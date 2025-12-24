use crate::dynamic_routes::{DynamicRoutesAnalyzer, DynamicRoutesConfig};
use crate::pydantic::PydanticExtractor;
use anyhow::Result;
use dc_core::call_graph::{CallGraph, CallGraphBuilder, CallNode, HttpMethod};
use dc_core::models::{Location, NodeId};
use dc_core::openapi::{OpenAPILinker, OpenAPIParser, OpenAPISchema};
use std::path::{Path, PathBuf};
use tracing::{debug, warn};

/// Call graph builder for FastAPI application
pub struct FastApiCallGraphBuilder {
    core_builder: CallGraphBuilder,
    app_path: PathBuf,
    verbose: bool,
    openapi_schema: Option<OpenAPISchema>,
    openapi_linker: Option<OpenAPILinker>,
    dynamic_routes_config: Option<DynamicRoutesConfig>,
}

impl FastApiCallGraphBuilder {
    /// Creates a new builder
    pub fn new(app_path: PathBuf) -> Self {
        let extractor = Box::new(PydanticExtractor::new());
        Self {
            core_builder: CallGraphBuilder::new().with_schema_extractor(extractor),
            app_path,
            verbose: false,
            openapi_schema: None,
            openapi_linker: None,
            dynamic_routes_config: None,
        }
    }

    /// Enables or disables strict import resolution in the underlying core builder
    pub fn with_strict_imports(mut self, strict_imports: bool) -> Self {
        self.core_builder = self.core_builder.with_strict_imports(strict_imports);
        self
    }

    /// Sets the maximum recursion depth
    pub fn with_max_depth(mut self, max_depth: Option<usize>) -> Self {
        self.core_builder = self.core_builder.with_max_depth(max_depth);
        self
    }

    /// Sets the verbose flag for debug output
    pub fn with_verbose(mut self, verbose: bool) -> Self {
        self.verbose = verbose;
        self.core_builder = self.core_builder.with_verbose(verbose);
        self
    }

    /// Sets the OpenAPI schema path
    /// If provided, the builder will use OpenAPI schema to enhance route detection
    pub fn with_openapi_schema(mut self, openapi_path: Option<PathBuf>) -> Self {
        if let Some(path) = openapi_path {
            if let Ok(schema) = OpenAPIParser::parse_file(&path) {
                debug!(
                    openapi_path = ?path,
                    "Loaded OpenAPI schema"
                );
                self.openapi_schema = Some(schema.clone());
                self.openapi_linker = Some(OpenAPILinker::new(schema));
            } else {
                warn!(
                    openapi_path = ?path,
                    "Failed to parse OpenAPI schema"
                );
            }
        }
        self
    }

    /// Sets the dynamic routes configuration
    pub fn with_dynamic_routes_config(mut self, config: Option<DynamicRoutesConfig>) -> Self {
        self.dynamic_routes_config = config;
        self
    }

    /// Builds graph for FastAPI application
    /// Consumes self, as it calls into_graph() on core_builder
    pub fn build_graph(self) -> Result<CallGraph> {
        // Determine project root
        let project_root = Self::find_project_root(&self.app_path);

        // Find entry point
        let entry_point = if self.app_path.exists() && self.app_path.is_file() {
            // If app_path points to specific file, use it
            self.app_path.clone()
        } else {
            // Otherwise search for standard entry point
            self.core_builder.find_entry_point(&project_root)?
        };

        // Build call graph from entry point
        // CallGraphBuilder will automatically handle:
        // - Imports
        // - Functions and classes
        // - Function calls
        // - FastAPI decorators (@app.get, @app.post, etc.)
        let mut core_builder = self.core_builder;
        core_builder.build_from_entry(&entry_point)?;

        // Store verbose and openapi_linker before moving self
        let verbose = self.verbose;
        let openapi_linker = self.openapi_linker;

        // Analyze dynamic routes (fastapi_users, etc.)
        let mut dynamic_analyzer =
            DynamicRoutesAnalyzer::new().with_config(self.dynamic_routes_config.clone());

        // Get the graph (before dynamic routes processing)
        let mut graph =
            if let Ok(dynamic_endpoints) = dynamic_analyzer.analyze_main_file(&entry_point) {
                if !dynamic_endpoints.is_empty() {
                    debug!(
                        endpoint_count = dynamic_endpoints.len(),
                        entry_point = ?entry_point,
                        "Found dynamic endpoints"
                    );

                    // Create virtual route nodes in the graph
                    let mut graph = core_builder.into_graph();
                    dynamic_analyzer.create_virtual_routes(
                        &mut graph,
                        &dynamic_endpoints,
                        &entry_point,
                    );
                    graph
                } else {
                    core_builder.into_graph()
                }
            } else {
                core_builder.into_graph()
            };

        // Enhance routes with OpenAPI information
        if let Some(linker) = openapi_linker {
            if let Err(err) = Self::enhance_routes_with_openapi_static(&linker, &mut graph, verbose)
            {
                warn!(
                    error = %err,
                    "Failed to enhance routes with OpenAPI"
                );
            }
        }

        Ok(graph)
    }

    /// Finds project root by going up from app_path and searching for project markers
    fn find_project_root(app_path: &Path) -> PathBuf {
        let markers = ["pyproject.toml", "setup.py", "requirements.txt", ".git"];
        let mut current = app_path.to_path_buf();

        // If app_path is a file, start from its parent
        if current.is_file() {
            if let Some(parent) = current.parent() {
                current = parent.to_path_buf();
            }
        }

        // Go up until marker is found
        while let Some(parent) = current.parent() {
            // Check for markers
            for marker in &markers {
                let marker_path = parent.join(marker);
                // Handle access errors gracefully
                if marker_path.exists() {
                    return parent.to_path_buf();
                }
            }
            current = parent.to_path_buf();
        }

        // Fallback: return parent of app_path or app_path itself
        app_path
            .parent()
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| app_path.to_path_buf())
    }

    /// Enhances routes in the graph with OpenAPI information
    fn enhance_routes_with_openapi_static(
        linker: &OpenAPILinker,
        graph: &mut CallGraph,
        verbose: bool,
    ) -> Result<()> {
        // Find all Route nodes in the graph
        let route_nodes: Vec<(NodeId, String, HttpMethod)> = graph
            .node_indices()
            .filter_map(|node_id| {
                if let Some(CallNode::Route { path, method, .. }) = graph.node_weight(node_id) {
                    Some((NodeId::from(node_id), path.clone(), *method))
                } else {
                    None
                }
            })
            .collect();

        debug!(
            route_count = route_nodes.len(),
            "Found route nodes to enhance with OpenAPI"
        );

        // For each route, find corresponding OpenAPI endpoint and enrich
        for (node_id, path, method) in &route_nodes {
            if let Some(endpoint) = linker.match_route_to_endpoint(path, *method) {
                debug!(
                    http_method = ?method,
                    route_path = %path,
                    operation_id = ?endpoint.operation_id,
                    "Matched route to OpenAPI endpoint"
                );

                // Try to enrich handler function with OpenAPI schema information
                if let Some(CallNode::Route { handler, .. }) = graph.node_weight(node_id.0) {
                    // If OpenAPI has response schema, enrich handler's return type
                    if let Some(ref response_schema_name) = endpoint.response_schema {
                        if let Some(_schema) = linker.get_schema(response_schema_name) {
                            // Get handler node to enrich its return type
                            if let Some(handler_node) = graph.node_weight_mut(handler.0) {
                                // Create SchemaReference for OpenAPI schema
                                let schema_ref = dc_core::models::SchemaReference {
                                    name: response_schema_name.clone(),
                                    schema_type: dc_core::models::SchemaType::OpenAPI,
                                    location: dc_core::models::Location {
                                        file: format!("openapi://{}", response_schema_name),
                                        line: 0,
                                        column: None,
                                    },
                                    metadata: std::collections::HashMap::new(),
                                };

                                // Create TypeInfo with OpenAPI schema
                                let return_type = Some(dc_core::models::TypeInfo {
                                    base_type: dc_core::models::BaseType::Object,
                                    schema_ref: Some(schema_ref),
                                    constraints: Vec::new(),
                                    optional: false,
                                });

                                // Update handler node's return_type
                                match handler_node {
                                    CallNode::Function {
                                        return_type: rt, ..
                                    } => {
                                        *rt = return_type;
                                    }
                                    CallNode::Method {
                                        return_type: rt, ..
                                    } => {
                                        *rt = return_type;
                                    }
                                    _ => {
                                        // Handler is not a function or method, skip enrichment
                                    }
                                }

                                debug!(
                                    response_schema_name = %response_schema_name,
                                    http_method = ?method,
                                    route_path = %path,
                                    "Enriched handler with response schema"
                                );
                            }
                        }
                    }
                }
            } else {
                debug!(
                    http_method = ?method,
                    route_path = %path,
                    "Route not found in OpenAPI schema"
                );
            }
        }

        // Find OpenAPI endpoints not discovered in code and create virtual routes
        let discovered_routes: Vec<_> = route_nodes
            .iter()
            .map(|(_, path, method)| (path.clone(), *method))
            .collect();
        let (missing_in_openapi, missing_in_code) = linker.validate_routes(&discovered_routes);

        if verbose {
            if !missing_in_openapi.is_empty() {
                debug!(
                    route_count = missing_in_openapi.len(),
                    "Routes found in code but not in OpenAPI"
                );
            }
            if !missing_in_code.is_empty() {
                debug!(
                    endpoint_count = missing_in_code.len(),
                    "Endpoints found in OpenAPI but not in code (creating virtual routes)"
                );
            }
        }

        // Create virtual Route nodes for OpenAPI endpoints not found in code
        for endpoint in missing_in_code {
            // Try to find handler by operation_id
            let handler_name_opt = endpoint.operation_id.as_ref().map(|op_id| {
                // Convert operation_id to function name
                // e.g., "items:read_item" -> "read_item"
                op_id.split(':').next_back().unwrap_or(op_id).to_string()
            });

            if let Some(handler_name) = handler_name_opt {
                // Try to find function node by name (exact match only)
                let handler_node = graph
                    .node_indices()
                    .find(|&node_id| {
                        if let Some(CallNode::Function { name, .. }) = graph.node_weight(node_id) {
                            name == &handler_name
                        } else if let Some(CallNode::Method { name, .. }) =
                            graph.node_weight(node_id)
                        {
                            name == &handler_name
                        } else {
                            false
                        }
                    })
                    .map(NodeId::from);

                let method = match endpoint.method.to_lowercase().as_str() {
                    "get" => HttpMethod::Get,
                    "post" => HttpMethod::Post,
                    "put" => HttpMethod::Put,
                    "patch" => HttpMethod::Patch,
                    "delete" => HttpMethod::Delete,
                    "options" => HttpMethod::Options,
                    "head" => HttpMethod::Head,
                    _ => continue,
                };

                // Create placeholder function node if handler not found
                let handler_node_id = handler_node.unwrap_or_else(|| {
                    let placeholder_id = graph.add_node(CallNode::Function {
                        name: handler_name.clone(),
                        file: PathBuf::from("openapi-virtual"),
                        line: 0,
                        parameters: vec![],
                        return_type: None,
                    });
                    NodeId::from(placeholder_id)
                });

                // Create virtual route node
                let _route_node_id = graph.add_node(CallNode::Route {
                    path: endpoint.path.clone(),
                    method,
                    handler: handler_node_id,
                    location: Location {
                        file: "openapi-virtual".to_string(),
                        line: 0,
                        column: None,
                    },
                    request_schema: None,
                    response_schema: None,
                });

                debug!(
                    http_method = ?method,
                    route_path = %endpoint.path,
                    "Created virtual route from OpenAPI"
                );
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn test_find_project_root_with_pyproject() {
        let temp_dir = TempDir::new().unwrap();
        let project_root = temp_dir.path();
        let app_path = project_root.join("src").join("app.py");

        // Create project structure
        fs::create_dir_all(app_path.parent().unwrap()).unwrap();
        fs::write(project_root.join("pyproject.toml"), "[project]").unwrap();
        fs::write(&app_path, "from fastapi import FastAPI").unwrap();

        let found_root = FastApiCallGraphBuilder::find_project_root(&app_path);
        assert_eq!(found_root, project_root);
    }

    #[test]
    fn test_find_project_root_with_git() {
        let temp_dir = TempDir::new().unwrap();
        let project_root = temp_dir.path();
        let app_path = project_root.join("backend").join("api").join("main.py");

        fs::create_dir_all(app_path.parent().unwrap()).unwrap();
        fs::create_dir_all(project_root.join(".git")).unwrap();
        fs::write(&app_path, "from fastapi import FastAPI").unwrap();

        let found_root = FastApiCallGraphBuilder::find_project_root(&app_path);
        assert_eq!(found_root, project_root);
    }

    #[test]
    fn test_find_project_root_fallback() {
        let temp_dir = TempDir::new().unwrap();
        let app_path = temp_dir.path().join("app.py");
        fs::write(&app_path, "from fastapi import FastAPI").unwrap();

        let found_root = FastApiCallGraphBuilder::find_project_root(&app_path);
        // Should return parent of app_path
        assert_eq!(found_root, app_path.parent().unwrap());
    }
}
