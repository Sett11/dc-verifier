use crate::call_graph::{CallEdge, CallNode};
use crate::models::NodeId;
use petgraph::{Directed, Graph};

/// Call graph - main structure for representing relationships between code nodes
pub type CallGraph = Graph<CallNode, CallEdge, Directed, u32>;

/// Finds all nodes of a specific type
pub fn find_nodes<F>(graph: &CallGraph, predicate: F) -> Vec<NodeId>
where
    F: Fn(&CallNode) -> bool,
{
    graph
        .node_indices()
        .filter(|&idx| {
            graph
                .node_weight(idx)
                .map(|node| predicate(node))
                .unwrap_or(false)
        })
        .map(NodeId::from)
        .collect()
}

/// Finds node by function/class name
pub fn find_node_by_name(graph: &CallGraph, name: &str) -> Option<NodeId> {
    graph
        .node_indices()
        .find(|&idx| {
            graph.node_weight(idx).and_then(|node| match node {
                CallNode::Function { name: n, .. } => Some(n == name),
                CallNode::Class { name: n, .. } => Some(n == name),
                CallNode::Method { name: n, .. } => Some(n == name),
                CallNode::Route { .. } => None,
                CallNode::Module { .. } => None,
            }) == Some(true)
        })
        .map(NodeId::from)
}

/// Gets all incoming nodes (who calls this node)
pub fn incoming_nodes(graph: &CallGraph, node: NodeId) -> Vec<NodeId> {
    graph
        .neighbors_directed(*node, petgraph::Direction::Incoming)
        .map(NodeId::from)
        .collect()
}

/// Gets all outgoing nodes (whom this node calls)
pub fn outgoing_nodes(graph: &CallGraph, node: NodeId) -> Vec<NodeId> {
    graph
        .neighbors_directed(*node, petgraph::Direction::Outgoing)
        .map(NodeId::from)
        .collect()
}
