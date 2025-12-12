use crate::call_graph::{CallEdge, CallGraph, CallNode};
use anyhow::Result;
use bincode;
use blake3;
use sled::Db;

/// Cache store for call graphs
pub struct CacheStore {
    db: Db,
}

impl CacheStore {
    /// Creates a new cache store
    pub fn new(path: &str) -> Result<Self> {
        let db = sled::open(path)?;
        Ok(Self { db })
    }

    /// Checks if the graph for a file has changed
    pub fn is_changed(&self, file_path: &str, content: &[u8]) -> Result<bool> {
        let key = format!("file:{}", file_path);
        let new_hash = blake3::hash(content);

        match self.db.get(&key)? {
            Some(old_hash) => Ok(old_hash.as_ref() != new_hash.as_bytes()),
            None => Ok(true), // File is new, so it has changed
        }
    }

    /// Saves file hash
    pub fn save_file_hash(&self, file_path: &str, content: &[u8]) -> Result<()> {
        let key = format!("file:{}", file_path);
        let hash = blake3::hash(content);
        self.db.insert(key, hash.as_bytes())?;
        Ok(())
    }

    /// Saves call graph
    pub fn save_graph(&self, graph_id: &str, graph: &CallGraph) -> Result<()> {
        // Serialize graph manually, as petgraph::Graph is not directly serializable
        // 1. Collect all nodes
        let mut nodes: Vec<(u32, CallNode)> = Vec::new();
        for node_idx in graph.node_indices() {
            if let Some(node) = graph.node_weight(node_idx) {
                nodes.push((node_idx.index() as u32, node.clone()));
            }
        }

        // 2. Collect all edges
        let mut edges: Vec<(u32, u32, CallEdge)> = Vec::new();
        for edge_idx in graph.edge_indices() {
            if let Some((source, target)) = graph.edge_endpoints(edge_idx) {
                if let Some(edge) = graph.edge_weight(edge_idx) {
                    edges.push((source.index() as u32, target.index() as u32, edge.clone()));
                }
            }
        }

        // 3. Create structure for serialization
        #[derive(serde::Serialize)]
        struct GraphData {
            nodes: Vec<(u32, CallNode)>,
            edges: Vec<(u32, u32, CallEdge)>,
        }

        let graph_data = GraphData { nodes, edges };

        // 4. Serialize via bincode
        let serialized = bincode::serialize(&graph_data)?;

        // 5. Save to sled
        let key = format!("graph:{}", graph_id);
        self.db.insert(key, serialized)?;

        Ok(())
    }

    /// Loads call graph
    pub fn load_graph(&self, graph_id: &str) -> Result<Option<CallGraph>> {
        let key = format!("graph:{}", graph_id);

        if let Some(data) = self.db.get(&key)? {
            // Deserialize structure
            #[derive(serde::Deserialize)]
            struct GraphData {
                nodes: Vec<(u32, CallNode)>,
                edges: Vec<(u32, u32, CallEdge)>,
            }

            let graph_data: GraphData = bincode::deserialize(data.as_ref())?;

            // Restore graph
            let mut graph = CallGraph::new();

            // Create mapping from old indices to new ones
            let mut index_map: std::collections::HashMap<u32, petgraph::graph::NodeIndex> =
                std::collections::HashMap::new();

            // Add nodes
            for (old_idx, node) in graph_data.nodes {
                let new_idx = graph.add_node(node);
                index_map.insert(old_idx, new_idx);
            }

            // Add edges
            for (source_old, target_old, edge) in graph_data.edges {
                let source_new = index_map.get(&source_old).copied().ok_or_else(|| {
                    anyhow::anyhow!(
                        "Corrupted cache: missing node {} while restoring edge ({} -> {})",
                        source_old,
                        source_old,
                        target_old
                    )
                })?;
                let target_new = index_map.get(&target_old).copied().ok_or_else(|| {
                    anyhow::anyhow!(
                        "Corrupted cache: missing node {} while restoring edge ({} -> {})",
                        target_old,
                        source_old,
                        target_old
                    )
                })?;

                graph.add_edge(source_new, target_new, edge);
            }

            Ok(Some(graph))
        } else {
            Ok(None)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{Location, NodeId};
    use petgraph::graph::NodeIndex;
    use tempfile::TempDir;

    #[test]
    fn fails_when_edge_references_missing_node() {
        let dir = TempDir::new().unwrap();
        let store = CacheStore::new(dir.path().to_str().unwrap()).unwrap();
        let key = "graph:test";

        #[derive(serde::Serialize)]
        struct GraphData {
            nodes: Vec<(u32, CallNode)>,
            edges: Vec<(u32, u32, CallEdge)>,
        }

        let edge = CallEdge::Call {
            caller: NodeId(NodeIndex::new(0)),
            callee: NodeId(NodeIndex::new(1)),
            argument_mapping: Vec::new(),
            location: Location {
                file: "file.py".into(),
                line: 1,
                column: Some(0),
            },
        };

        let data = GraphData {
            nodes: Vec::new(), // nodes are missing, but edge exists
            edges: vec![(0, 1, edge)],
        };

        let serialized = bincode::serialize(&data).unwrap();
        store.db.insert(key, serialized).unwrap();

        let err = store.load_graph("test").unwrap_err();
        assert!(
            err.to_string().contains("Corrupted cache"),
            "unexpected error: {err}"
        );
    }
}
