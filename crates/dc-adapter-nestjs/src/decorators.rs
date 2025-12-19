use crate::extractor::ParameterExtractor;
use anyhow::Result;
use dc_core::call_graph::HttpMethod;
use dc_core::call_graph::{CallEdge, CallGraph, CallNode};
use dc_core::models::{Location, NodeId};
use dc_core::parsers::{DecoratorTarget, TypeScriptDecorator};
use std::collections::HashMap;

/// Processor for NestJS decorators
pub struct NestJSDecoratorProcessor {
    graph: CallGraph,
    controller_paths: HashMap<String, String>, // class_name -> path
    parameter_extractor: Option<ParameterExtractor>,
    verbose: bool,
}

impl NestJSDecoratorProcessor {
    /// Creates a new decorator processor
    pub fn new(graph: CallGraph, verbose: bool) -> Self {
        Self {
            graph,
            controller_paths: HashMap::new(),
            parameter_extractor: None,
            verbose,
        }
    }

    /// Sets parameter extractor for extracting request/response types
    pub fn with_parameter_extractor(mut self, extractor: ParameterExtractor) -> Self {
        self.parameter_extractor = Some(extractor);
        self
    }

    /// Processes decorators and creates Route nodes
    pub fn process_decorators(&mut self, decorators: Vec<TypeScriptDecorator>) -> Result<()> {
        // Group decorators by target objects
        let mut class_decorators: HashMap<String, Vec<&TypeScriptDecorator>> = HashMap::new();
        let mut method_decorators: HashMap<(String, String), Vec<&TypeScriptDecorator>> =
            HashMap::new();
        let mut parameter_decorators: HashMap<(String, String, String), &TypeScriptDecorator> =
            HashMap::new();

        // Group decorators by target
        for decorator in &decorators {
            match &decorator.target {
                DecoratorTarget::Class(class_name) => {
                    class_decorators
                        .entry(class_name.clone())
                        .or_default()
                        .push(decorator);
                }
                DecoratorTarget::Method { class, method } => {
                    let key = (class.clone(), method.clone());
                    method_decorators
                        .entry(key)
                        .or_default()
                        .push(decorator);
                }
                DecoratorTarget::Parameter {
                    class,
                    method,
                    parameter,
                } => {
                    let key = (class.clone(), method.clone(), parameter.clone());
                    parameter_decorators.insert(key, decorator);
                }
            }
        }

        // Process controller decorators
        for (class_name, decorators) in &class_decorators {
            for decorator in decorators {
                if decorator.name == "Controller" {
                    self.process_controller_decorator(decorator, class_name)?;
                }
            }
        }

        // Process method decorators and create Route nodes
        for ((class_name, method_name), decorators) in &method_decorators {
            for decorator in decorators {
                if Self::extract_http_method(&decorator.name).is_some() {
                    if let Some(route_info) = self.process_method_decorator(
                        decorator,
                        class_name,
                        method_name,
                        &parameter_decorators,
                    )? {
                        // Extract request/response types if parameter extractor is available
                        // First, get method node and parameters (without mutable borrow)
                        let (request_type, response_type) = if self.parameter_extractor.is_some() {
                            if let Some(method_node) =
                                self.find_method_node(class_name, method_name)
                            {
                                // Get method parameters from graph
                                let method_params = self.get_method_parameters(method_node)?;

                                // Get method decorators and parameter decorators for this method
                                let method_decs: Vec<&TypeScriptDecorator> = decorators.to_vec();
                                let param_decs: Vec<&TypeScriptDecorator> = parameter_decorators
                                    .iter()
                                    .filter(|(k, _)| k.0 == *class_name && k.1 == *method_name)
                                    .map(|(_, d)| *d)
                                    .collect();

                                // Now use extractor (mutable borrow)
                                if let Some(ref mut extractor) = self.parameter_extractor {
                                    extractor.extract_route_parameters(
                                        &self.graph,
                                        method_node,
                                        &method_decs,
                                        &param_decs,
                                        &method_params,
                                    )?
                                } else {
                                    (None, None)
                                }
                            } else {
                                (None, None)
                            }
                        } else {
                            (None, None)
                        };

                        // Create Route node
                        let route_node_id = NodeId::from(self.graph.add_node(CallNode::Route {
                            path: route_info.path.clone(),
                            method: route_info.method,
                            handler: route_info.handler,
                            location: route_info.location.clone(),
                        }));

                        // Create edge from Route to handler
                        self.graph.add_edge(
                            route_node_id.0,
                            route_info.handler.0,
                            CallEdge::Call {
                                caller: route_node_id,
                                callee: route_info.handler,
                                argument_mapping: Vec::new(),
                                location: route_info.location,
                            },
                        );

                        if self.verbose {
                            eprintln!(
                                "[DEBUG] Created Route node: {:?} {} -> handler {:?} (request: {:?}, response: {:?})",
                                route_info.method,
                                route_info.path,
                                route_info.handler.0.index(),
                                request_type.is_some(),
                                response_type.is_some()
                            );
                        }
                    }
                }
            }
        }

        Ok(())
    }

    /// Processes controller decorator (@Controller)
    fn process_controller_decorator(
        &mut self,
        decorator: &TypeScriptDecorator,
        class_name: &str,
    ) -> Result<()> {
        // Extract path from @Controller('path') or @Controller()
        let path = if let Some(first_arg) = decorator.arguments.first() {
            // Remove quotes if present
            first_arg.trim_matches('"').trim_matches('\'').to_string()
        } else {
            String::new() // Empty path for @Controller()
        };

        self.controller_paths.insert(class_name.to_string(), path);

        if self.verbose {
            eprintln!(
                "[DEBUG] Controller '{}' has path: '{}'",
                class_name,
                self.controller_paths
                    .get(class_name)
                    .unwrap_or(&String::new())
            );
        }

        Ok(())
    }

    /// Processes method decorator (@Get, @Post, etc.)
    fn process_method_decorator(
        &mut self,
        decorator: &TypeScriptDecorator,
        class_name: &str,
        method_name: &str,
        _parameter_decorators: &HashMap<(String, String, String), &TypeScriptDecorator>,
    ) -> Result<Option<RouteInfo>> {
        // Extract HTTP method
        let http_method = match Self::extract_http_method(&decorator.name) {
            Some(method) => method,
            None => return Ok(None),
        };

        // Extract method path from decorator arguments
        let method_path = if let Some(first_arg) = decorator.arguments.first() {
            first_arg.trim_matches('"').trim_matches('\'').to_string()
        } else {
            String::new()
        };

        // Get controller path
        let controller_path = self
            .controller_paths
            .get(class_name)
            .cloned()
            .unwrap_or_default();

        // Combine paths
        let full_path = Self::combine_paths(&controller_path, &method_path);

        // Find handler method in graph
        let handler = match self.find_method_node(class_name, method_name) {
            Some(node) => node,
            None => {
                if self.verbose {
                    eprintln!(
                        "[DEBUG] Failed to find method node for {}.{}",
                        class_name, method_name
                    );
                }
                return Ok(None);
            }
        };

        Ok(Some(RouteInfo {
            path: full_path,
            method: http_method,
            handler,
            location: decorator.location.clone(),
        }))
    }

    /// Processes parameter decorator (@Body, @Query, @Param)
    #[allow(dead_code)] // Will be used in parameter extraction
    fn process_parameter_decorator(
        &self,
        decorator: &TypeScriptDecorator,
        parameter_name: &str,
    ) -> ParameterInfo {
        let source = match decorator.name.as_str() {
            "Body" => ParameterSource::Body,
            "Query" => ParameterSource::Query,
            "Param" => {
                // Extract parameter name from @Param('id')
                let param_name = decorator
                    .arguments
                    .first()
                    .map(|s| s.trim_matches('"').trim_matches('\'').to_string())
                    .unwrap_or_else(|| parameter_name.to_string());
                ParameterSource::Param(param_name)
            }
            "Headers" => ParameterSource::Headers,
            _ => ParameterSource::Body, // Default
        };

        ParameterInfo {
            name: parameter_name.to_string(),
            source,
            type_info: None, // Will be filled later from method parameters
        }
    }

    /// Extracts HTTP method from decorator name
    fn extract_http_method(decorator_name: &str) -> Option<HttpMethod> {
        match decorator_name {
            "Get" | "GET" => Some(HttpMethod::Get),
            "Post" | "POST" => Some(HttpMethod::Post),
            "Put" | "PUT" => Some(HttpMethod::Put),
            "Delete" | "DELETE" => Some(HttpMethod::Delete),
            "Patch" | "PATCH" => Some(HttpMethod::Patch),
            "Options" | "OPTIONS" => Some(HttpMethod::Options),
            "Head" | "HEAD" => Some(HttpMethod::Head),
            _ => None,
        }
    }

    /// Combines controller path and method path
    fn combine_paths(controller_path: &str, method_path: &str) -> String {
        let controller = controller_path.trim_matches('/');
        let method = method_path.trim_matches('/');

        if controller.is_empty() && method.is_empty() {
            return "/".to_string();
        }

        if controller.is_empty() {
            return format!("/{}", method);
        }

        if method.is_empty() {
            return format!("/{}", controller);
        }

        format!("/{}/{}", controller, method)
    }

    /// Finds method node in graph by class name and method name
    fn find_method_node(&self, class_name: &str, method_name: &str) -> Option<NodeId> {
        // First, find the class node
        let class_node = self.graph.node_indices().find(|&idx| {
            if let Some(CallNode::Class { name, .. }) = self.graph.node_weight(idx) {
                name == class_name
            } else {
                false
            }
        })?;

        // Then, find the method node that belongs to this class
        self.graph
            .node_indices()
            .find(|&idx| {
                if let Some(CallNode::Method { name, class, .. }) = self.graph.node_weight(idx) {
                    name == method_name && *class == NodeId::from(class_node)
                } else {
                    false
                }
            })
            .map(NodeId::from)
    }

    /// Gets method parameters from graph
    fn get_method_parameters(
        &self,
        method_node: NodeId,
    ) -> Result<Vec<dc_core::call_graph::Parameter>> {
        if let Some(CallNode::Method { parameters, .. }) = self.graph.node_weight(method_node.0) {
            Ok(parameters.clone())
        } else {
            Ok(Vec::new())
        }
    }

    /// Returns the graph (for use after processing)
    pub fn into_graph(self) -> CallGraph {
        self.graph
    }
}

/// Information about a route
#[allow(dead_code)] // Will be used in implementation
pub struct RouteInfo {
    pub path: String,
    pub method: HttpMethod,
    pub handler: NodeId,
    pub location: Location,
}

/// Information about a parameter
#[allow(dead_code)] // Will be used in implementation
pub struct ParameterInfo {
    pub name: String,
    pub source: ParameterSource,
    pub type_info: Option<dc_core::models::TypeInfo>,
}

/// Source of a parameter
#[allow(dead_code)] // Will be used in implementation
pub enum ParameterSource {
    Body,
    Query,
    Param(String), // path parameter name
    Headers,
}
