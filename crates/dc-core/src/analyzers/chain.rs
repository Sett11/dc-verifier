use crate::call_graph::{CallGraph, CallNode, Parameter};
use crate::data_flow::DataFlowTracker;
use crate::models::{
    BaseType, ChainDirection, ChainType, Contract, DataChain, Link, LinkType, Location, NodeId,
    SchemaReference, SchemaType, Severity, TypeInfo,
};
use anyhow::{anyhow, bail, Result};
use petgraph::graph::NodeIndex;
use std::collections::{HashMap, HashSet};
use std::path::Path;

/// Data chain builder from call graph
pub struct ChainBuilder<'a> {
    /// Call graph
    graph: &'a CallGraph,
    /// Data flow tracker
    #[allow(dead_code)]
    data_flow: &'a DataFlowTracker<'a>,
    /// Enable verbose debug output
    verbose: bool,
}

impl<'a> ChainBuilder<'a> {
    /// Creates a new chain builder
    pub fn new(graph: &'a CallGraph, data_flow: &'a DataFlowTracker<'a>, verbose: bool) -> Self {
        Self {
            graph,
            data_flow,
            verbose,
        }
    }

    /// Builds chain from entry point to end point
    pub fn build_chain(&self, entry: NodeId, direction: ChainDirection) -> Result<DataChain> {
        match direction {
            ChainDirection::FrontendToBackend => self.build_forward_chain(entry),
            ChainDirection::BackendToFrontend => self.build_reverse_chain(entry),
        }
    }

    /// Finds all chains in project
    pub fn find_all_chains(&self) -> Result<Vec<DataChain>> {
        let mut chains = Vec::new();

        // Find all routes (API entry points)
        let routes =
            crate::call_graph::find_nodes(self.graph, |n| matches!(n, CallNode::Route { .. }));

        if self.verbose {
            eprintln!(
                "[DEBUG] Found {} route nodes in graph (total nodes: {})",
                routes.len(),
                self.graph.node_count()
            );
        }

        for (idx, route) in routes.iter().enumerate() {
            let route_index: NodeIndex<u32> = (*route).into();
            let route_node = self
                .graph
                .node_weight(route_index)
                .ok_or_else(|| anyhow!("Route node not found: {:?}", route))?;

            if self.verbose {
                if let CallNode::Route { path, method, .. } = route_node {
                    eprintln!(
                        "[DEBUG] Processing route {}: {} {} (node {:?})",
                        idx + 1,
                        format!("{:?}", method).to_uppercase(),
                        path,
                        route.0.index()
                    );
                }
            }

            // Build chain Frontend → Backend → Database
            match self.build_forward_chain(*route) {
                Ok(forward_chain) => {
                    if self.verbose {
                        eprintln!(
                            "[DEBUG] Successfully built forward chain from route {:?} ({} links)",
                            route.0.index(),
                            forward_chain.links.len()
                        );
                    }
                    chains.push(forward_chain);
                }
                Err(e) => {
                    if self.verbose {
                        eprintln!(
                            "[DEBUG] Failed to build forward chain from route {:?}: {}",
                            route.0.index(),
                            e
                        );
                    }
                }
            }

            // Build chain Database → Backend → Frontend
            match self.build_reverse_chain(*route) {
                Ok(reverse_chain) => {
                    if self.verbose {
                        eprintln!(
                            "[DEBUG] Successfully built reverse chain from route {:?} ({} links)",
                            route.0.index(),
                            reverse_chain.links.len()
                        );
                    }
                    chains.push(reverse_chain);
                }
                Err(e) => {
                    if self.verbose {
                        eprintln!(
                            "[DEBUG] Failed to build reverse chain from route {:?}: {}",
                            route.0.index(),
                            e
                        );
                    }
                }
            }
        }

        if self.verbose {
            eprintln!(
                "[DEBUG] Total chains built: {} (from {} routes)",
                chains.len(),
                routes.len()
            );
        }

        Ok(chains)
    }

    /// Builds chain Frontend → Backend → Database
    pub fn build_forward_chain(&self, start: NodeId) -> Result<DataChain> {
        self.ensure_node_exists(start)?;
        let path = self.collect_path(start, |node| {
            crate::call_graph::outgoing_nodes(self.graph, node)
        });

        if path.is_empty() {
            bail!("Failed to build forward chain: empty path");
        }

        let links = self.create_links_from_nodes(&path, ChainDirection::FrontendToBackend)?;
        let contracts = self.build_contracts(&links);
        let chain_type = self.determine_chain_type(&links);

        Ok(DataChain {
            id: format!("chain-{}", start.index()),
            name: self.generate_chain_name(start)?,
            links,
            contracts,
            direction: ChainDirection::FrontendToBackend,
            chain_type,
        })
    }

    /// Builds chain Database → Backend → Frontend
    pub fn build_reverse_chain(&self, start: NodeId) -> Result<DataChain> {
        self.ensure_node_exists(start)?;
        let mut path = self.collect_path(start, |node| {
            crate::call_graph::incoming_nodes(self.graph, node)
        });
        if path.is_empty() {
            bail!("Failed to build reverse chain: empty path");
        }
        path.reverse();

        let links = self.create_links_from_nodes(&path, ChainDirection::BackendToFrontend)?;
        let contracts = self.build_contracts(&links);
        let chain_type = self.determine_chain_type(&links);

        Ok(DataChain {
            id: format!("chain-reverse-{}", start.index()),
            name: format!("{} (reverse)", self.generate_chain_name(start)?),
            links,
            contracts,
            direction: ChainDirection::BackendToFrontend,
            chain_type,
        })
    }

    fn ensure_node_exists(&self, node_id: NodeId) -> Result<()> {
        if self.graph.node_weight(*node_id).is_some() {
            Ok(())
        } else {
            bail!("Node {:?} not found in graph", node_id);
        }
    }

    fn collect_path<F>(&self, start: NodeId, get_neighbors: F) -> Vec<NodeId>
    where
        F: Fn(NodeId) -> Vec<NodeId>,
    {
        let mut order = Vec::new();
        let mut current = start;
        let mut visited = HashSet::new();

        loop {
            if visited.contains(&current) {
                break;
            }
            visited.insert(current);
            order.push(current);

            let next = get_neighbors(current)
                .into_iter()
                .find(|candidate| !visited.contains(candidate));

            match next {
                Some(next_node) => current = next_node,
                None => break,
            }
        }

        order
    }

    fn create_links_from_nodes(
        &self,
        nodes: &[NodeId],
        _direction: ChainDirection,
    ) -> Result<Vec<Link>> {
        let total = nodes.len();
        nodes
            .iter()
            .enumerate()
            .map(|(idx, node_id)| {
                let mut link_type = self.determine_link_type(*node_id);
                // Simplified logic without duplication by direction
                if total == 1 || idx == 0 {
                    link_type = LinkType::Source;
                } else if idx == total - 1 {
                    link_type = LinkType::Sink;
                }
                // Otherwise use link_type from determine_link_type
                self.create_link_from_node(*node_id, link_type)
            })
            .collect()
    }

    fn build_contracts(&self, links: &[Link]) -> Vec<Contract> {
        let mut contracts = Vec::new();
        for window in links.windows(2) {
            if let [from, to] = window {
                contracts.push(self.create_contract(from, to));
            }
        }
        contracts
    }

    fn create_contract(&self, from: &Link, to: &Link) -> Contract {
        let mut contract = Contract {
            from_link_id: from.id.clone(),
            to_link_id: to.id.clone(),
            from_schema: from.schema_ref.clone(),
            to_schema: to.schema_ref.clone(),
            mismatches: Vec::new(),
            severity: Severity::Info,
        };

        // Check for missing schemas and set severity
        let has_missing_schema = contract.from_schema.metadata.contains_key("missing_schema")
            || contract.to_schema.metadata.contains_key("missing_schema");

        if has_missing_schema {
            contract.severity = Severity::Warning;
        }

        contract
    }

    fn create_link_from_node(&self, node_id: NodeId, link_type: LinkType) -> Result<Link> {
        let node = self
            .graph
            .node_weight(*node_id)
            .ok_or_else(|| anyhow!("Node not found: {:?}", node_id))?
            .clone();

        let (id, location, schema_ref) = match node {
            CallNode::Route { path, location, .. } => {
                let schema = self.extract_route_schema(node_id)?;
                (
                    format!("route-{}-{}", path.replace('/', "-"), node_id.index()),
                    location,
                    schema,
                )
            }
            CallNode::Function {
                name,
                file,
                line,
                parameters,
                ..
            } => {
                let location = self.location_from_path(&file, line);
                let schema = self.extract_function_schema(&parameters, &name, &location);
                (
                    format!("func-{}-{}", name, node_id.index()),
                    location,
                    schema,
                )
            }
            CallNode::Method {
                name,
                class,
                parameters,
                ..
            } => {
                let (file_path, line) = self.method_location(class)?;
                let location = self.location_from_path(&file_path, line);
                let schema = self.extract_function_schema(&parameters, &name, &location);
                (
                    format!("method-{}-{}", name, node_id.index()),
                    location,
                    schema,
                )
            }
            CallNode::Class { name, file, .. } => {
                let location = self.location_from_path(&file, 0);
                let schema = self.extract_class_schema(&name, &location);
                (
                    format!("class-{}-{}", name, node_id.index()),
                    location,
                    schema,
                )
            }
            CallNode::Module { path } => {
                bail!("Cannot create chain link from module: {:?}", path.display());
            }
        };

        Ok(Link {
            id,
            link_type,
            location,
            node_id,
            schema_ref,
        })
    }

    fn extract_function_schema(
        &self,
        parameters: &[Parameter],
        fallback_name: &str,
        location: &Location,
    ) -> SchemaReference {
        for param in parameters {
            if let Some(schema) = self.schema_from_type_info(&param.type_info) {
                return schema;
            }
        }

        self.unknown_schema(fallback_name, location.clone())
    }

    fn extract_route_schema(&self, route_node_id: NodeId) -> Result<SchemaReference> {
        let route_node = self
            .graph
            .node_weight(*route_node_id)
            .ok_or_else(|| anyhow!("Route node not found: {:?}", route_node_id))?;

        if let CallNode::Route { handler, .. } = route_node {
            if let Some(CallNode::Function {
                name,
                parameters,
                file,
                line,
                ..
            }) = self.graph.node_weight(handler.0).cloned()
            {
                let location = self.location_from_path(&file, line);
                return Ok(self.extract_function_schema(&parameters, &name, &location));
            }
        }

        Ok(self.unknown_schema(
            "RouteRequest",
            Location {
                file: String::new(),
                line: 0,
                column: None,
            },
        ))
    }

    fn extract_class_schema(&self, name: &str, location: &Location) -> SchemaReference {
        SchemaReference {
            name: name.to_string(),
            schema_type: SchemaType::Pydantic,
            location: location.clone(),
            metadata: HashMap::new(),
        }
    }

    fn method_location(&self, class_node: NodeId) -> Result<(std::path::PathBuf, usize)> {
        let node = self
            .graph
            .node_weight(*class_node)
            .ok_or_else(|| anyhow!("Class for method not found: {:?}", class_node))?;

        if let CallNode::Class { file, .. } = node {
            Ok((file.clone(), 0))
        } else {
            bail!("Node {:?} is not a class", class_node);
        }
    }

    fn schema_from_type_info(&self, type_info: &TypeInfo) -> Option<SchemaReference> {
        if let Some(schema) = &type_info.schema_ref {
            return Some(schema.clone());
        }

        match type_info.base_type {
            BaseType::Object | BaseType::Array => {
                let mut metadata = HashMap::new();
                metadata.insert(
                    "base_type".to_string(),
                    format!("{:?}", type_info.base_type),
                );
                Some(SchemaReference {
                    name: format!("{:?}", type_info.base_type),
                    schema_type: SchemaType::JsonSchema,
                    location: Location {
                        file: String::new(),
                        line: 0,
                        column: None,
                    },
                    metadata,
                })
            }
            _ => None,
        }
    }

    fn unknown_schema(&self, name: &str, location: Location) -> SchemaReference {
        SchemaReference {
            name: name.to_string(),
            schema_type: SchemaType::JsonSchema,
            location,
            metadata: HashMap::new(),
        }
    }

    fn determine_link_type(&self, node_id: NodeId) -> LinkType {
        self.graph
            .node_weight(*node_id)
            .map(|node| match node {
                CallNode::Route { .. } => LinkType::Source,
                CallNode::Class { .. } => LinkType::Sink,
                _ => LinkType::Transformer,
            })
            .unwrap_or(LinkType::Transformer)
    }

    /// Determines the type of chain based on its links
    ///
    /// - Full: Contains Route nodes (API endpoints) - spans multiple layers
    /// - FrontendInternal: All nodes are from TypeScript files (.ts/.tsx)
    /// - BackendInternal: All nodes are from Python files (.py)
    fn determine_chain_type(&self, links: &[Link]) -> ChainType {
        if links.is_empty() {
            return ChainType::Full;
        }

        // Check if chain contains Route nodes (API endpoints)
        let has_route = links.iter().any(|link| {
            self.graph
                .node_weight(*link.node_id)
                .map(|node| matches!(node, CallNode::Route { .. }))
                .unwrap_or(false)
        });

        if has_route {
            return ChainType::Full;
        }

        // Check file extensions to determine if all nodes are from same layer
        let mut has_frontend = false;
        let mut has_backend = false;

        for link in links {
            let file_ext = Path::new(&link.location.file)
                .extension()
                .and_then(|e| e.to_str());

            match file_ext {
                Some("ts") | Some("tsx") => has_frontend = true,
                Some("py") => has_backend = true,
                _ => {}
            }
        }

        // Determine chain type
        if has_frontend && !has_backend {
            ChainType::FrontendInternal
        } else if has_backend && !has_frontend {
            ChainType::BackendInternal
        } else {
            // Mixed or unknown - treat as full chain
            ChainType::Full
        }
    }

    fn location_from_path(&self, path: &Path, line: usize) -> Location {
        Location {
            file: path.to_string_lossy().to_string(),
            line,
            column: None,
        }
    }

    fn generate_chain_name(&self, start: NodeId) -> Result<String> {
        let node = self
            .graph
            .node_weight(*start)
            .ok_or_else(|| anyhow!("Node not found: {:?}", start))?;

        Ok(match node {
            CallNode::Route { path, method, .. } => {
                let method_str = format!("{:?}", method).to_uppercase();
                format!("{} {}", method_str, path)
            }
            CallNode::Function { name, .. } => format!("Function {}", name),
            CallNode::Class { name, .. } => format!("Class {}", name),
            CallNode::Method { name, .. } => format!("Method {}", name),
            CallNode::Module { path } => {
                format!(
                    "Module {}",
                    path.file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or_default()
                )
            }
        })
    }
}
