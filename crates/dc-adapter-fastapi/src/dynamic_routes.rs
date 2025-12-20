use anyhow::Result;
use dc_core::call_graph::{CallGraph, CallNode, HttpMethod};
use dc_core::models::{Location, NodeId};
use rustpython_parser::ast;
use rustpython_parser::{parse, Mode};
use std::path::Path;

/// Information about a dynamically generated endpoint
#[derive(Debug, Clone)]
pub struct DynamicEndpoint {
    pub path: String,
    pub method: HttpMethod,
    pub request_schema: Option<String>,
    pub response_schema: Option<String>,
}

/// Configuration for fastapi_users router generators
struct FastAPIUsersRouter {
    #[allow(dead_code)]
    method_name: &'static str,
    endpoints: Vec<DynamicEndpoint>,
    schema_params: Vec<&'static str>,
}

/// Analyzer for dynamically generated FastAPI routes (e.g., fastapi_users)
pub struct DynamicRoutesAnalyzer {
    verbose: bool,
}

impl DynamicRoutesAnalyzer {
    pub fn new(verbose: bool) -> Self {
        Self { verbose }
    }

    /// Analyzes main.py file for dynamic route generation
    /// Returns a list of dynamic endpoints found
    pub fn analyze_main_file(&self, main_file: &Path) -> Result<Vec<DynamicEndpoint>> {
        let source = std::fs::read_to_string(main_file)?;
        let ast = parse(&source, Mode::Module, main_file.to_string_lossy().as_ref())?;

        let mut endpoints = Vec::new();

        if let ast::Mod::Module(module) = ast {
            for stmt in &module.body {
                self.analyze_statement(stmt, main_file, &mut endpoints)?;
            }
        }

        Ok(endpoints)
    }

    fn analyze_statement(
        &self,
        stmt: &ast::Stmt,
        current_file: &Path,
        endpoints: &mut Vec<DynamicEndpoint>,
    ) -> Result<()> {
        match stmt {
            ast::Stmt::Expr(expr_stmt) => {
                if let ast::Expr::Call(call_expr) = expr_stmt.value.as_ref() {
                    self.analyze_call(call_expr, current_file, endpoints)?;
                }
            }
            ast::Stmt::Assign(assign_stmt) => {
                // Check for app.include_router(...) assignments
                for target in &assign_stmt.targets {
                    if let ast::Expr::Attribute(attr) = target {
                        if attr.attr.as_str() == "include_router" {
                            if let ast::Expr::Call(call_expr) = assign_stmt.value.as_ref() {
                                self.analyze_call(call_expr, current_file, endpoints)?;
                            }
                        }
                    }
                }
            }
            _ => {}
        }
        Ok(())
    }

    fn analyze_call(
        &self,
        call_expr: &ast::ExprCall,
        current_file: &Path,
        endpoints: &mut Vec<DynamicEndpoint>,
    ) -> Result<()> {
        // Check if this is app.include_router(...)
        if let ast::Expr::Attribute(attr) = call_expr.func.as_ref() {
            if attr.attr.as_str() == "include_router" {
                return self.analyze_include_router(call_expr, current_file, endpoints);
            }
        }

        // Check if this is a fastapi_users router generator call
        if let Some(router_info) = self.identify_router_generator(call_expr) {
            // Extract prefix from parent include_router call if available
            // For now, we'll use default prefixes
            let prefix = self.extract_prefix_from_context(call_expr);
            let mut router_endpoints = router_info.endpoints.clone();

            // Extract schemas from call arguments
            let schemas = self.extract_schemas_from_call(call_expr);

            // Apply prefix and schemas to endpoints
            for endpoint in &mut router_endpoints {
                endpoint.path = format!("{}{}", prefix, endpoint.path);

                // Map schemas to endpoints based on router configuration
                if !schemas.is_empty() {
                    if router_info.schema_params.contains(&"response_schema") && !schemas.is_empty()
                    {
                        endpoint.response_schema = Some(schemas[0].clone());
                    }
                    if router_info.schema_params.contains(&"request_schema") && schemas.len() > 1 {
                        endpoint.request_schema = Some(schemas[1].clone());
                    }
                }
            }

            endpoints.extend(router_endpoints);
        }

        Ok(())
    }

    fn analyze_include_router(
        &self,
        call_expr: &ast::ExprCall,
        _current_file: &Path,
        endpoints: &mut Vec<DynamicEndpoint>,
    ) -> Result<()> {
        // Extract router from first argument
        let router_call = call_expr.args.first();

        // Extract prefix from keyword arguments
        let mut prefix = String::new();
        for kw in &call_expr.keywords {
            if let Some(arg_name) = &kw.arg {
                if arg_name.as_str() == "prefix" {
                    if let Some(prefix_str) = self.extract_string_value(&kw.value) {
                        prefix = prefix_str;
                    }
                }
            }
        }

        // Analyze router call if it's a fastapi_users generator
        if let Some(router_expr) = router_call {
            // router_expr is &Box<Expr>, Box automatically dereferences through Deref
            // Use &*router_expr to get &Expr
            if let ast::Expr::Call(router_call_expr) = &*router_expr {
                if let Some(router_info) = self.identify_router_generator(&router_call_expr) {
                    // Extract schemas from router call arguments
                    let schemas = self.extract_schemas_from_call(&router_call_expr);

                    let mut router_endpoints = router_info.endpoints.clone();

                    // Apply prefix and schemas
                    for endpoint in &mut router_endpoints {
                        endpoint.path = format!("{}{}", prefix, endpoint.path);

                        // Map schemas based on router configuration
                        if !schemas.is_empty() {
                            if router_info.schema_params.contains(&"response_schema")
                                && !schemas.is_empty()
                            {
                                endpoint.response_schema = Some(schemas[0].clone());
                            }
                            if router_info.schema_params.contains(&"request_schema")
                                && schemas.len() > 1
                            {
                                endpoint.request_schema = Some(schemas[1].clone());
                            }
                        }
                    }

                    endpoints.extend(router_endpoints);
                }
            }
        }

        Ok(())
    }

    fn identify_router_generator(&self, call_expr: &ast::ExprCall) -> Option<FastAPIUsersRouter> {
        // Check if this is a fastapi_users router generator
        // call_expr.func is Box<Expr>, so we use as_ref() to get &Expr
        if let ast::Expr::Attribute(attr) = call_expr.func.as_ref() {
            let method_name = attr.attr.as_str();

            match method_name {
                "get_register_router" => Some(FastAPIUsersRouter {
                    method_name: "get_register_router",
                    endpoints: vec![DynamicEndpoint {
                        path: "/register".to_string(),
                        method: HttpMethod::Post,
                        request_schema: None,
                        response_schema: None,
                    }],
                    schema_params: vec!["response_schema", "request_schema"],
                }),
                "get_auth_router" => Some(FastAPIUsersRouter {
                    method_name: "get_auth_router",
                    endpoints: vec![
                        DynamicEndpoint {
                            path: "/login".to_string(),
                            method: HttpMethod::Post,
                            request_schema: None,
                            response_schema: Some("BearerResponse".to_string()),
                        },
                        DynamicEndpoint {
                            path: "/logout".to_string(),
                            method: HttpMethod::Post,
                            request_schema: None,
                            response_schema: None,
                        },
                    ],
                    schema_params: vec![],
                }),
                "get_reset_password_router" => Some(FastAPIUsersRouter {
                    method_name: "get_reset_password_router",
                    endpoints: vec![
                        DynamicEndpoint {
                            path: "/forgot-password".to_string(),
                            method: HttpMethod::Post,
                            request_schema: None,
                            response_schema: None,
                        },
                        DynamicEndpoint {
                            path: "/reset-password".to_string(),
                            method: HttpMethod::Post,
                            request_schema: None,
                            response_schema: None,
                        },
                    ],
                    schema_params: vec![],
                }),
                "get_verify_router" => Some(FastAPIUsersRouter {
                    method_name: "get_verify_router",
                    endpoints: vec![
                        DynamicEndpoint {
                            path: "/request-verify-token".to_string(),
                            method: HttpMethod::Post,
                            request_schema: None,
                            response_schema: None,
                        },
                        DynamicEndpoint {
                            path: "/verify".to_string(),
                            method: HttpMethod::Post,
                            request_schema: None,
                            response_schema: None,
                        },
                    ],
                    schema_params: vec!["response_schema"],
                }),
                "get_users_router" => Some(FastAPIUsersRouter {
                    method_name: "get_users_router",
                    endpoints: vec![
                        DynamicEndpoint {
                            path: "/me".to_string(),
                            method: HttpMethod::Get,
                            request_schema: None,
                            response_schema: None,
                        },
                        DynamicEndpoint {
                            path: "/{id}".to_string(),
                            method: HttpMethod::Get,
                            request_schema: None,
                            response_schema: None,
                        },
                        DynamicEndpoint {
                            path: "/{id}".to_string(),
                            method: HttpMethod::Patch,
                            request_schema: None,
                            response_schema: None,
                        },
                        DynamicEndpoint {
                            path: "/{id}".to_string(),
                            method: HttpMethod::Delete,
                            request_schema: None,
                            response_schema: None,
                        },
                    ],
                    schema_params: vec!["response_schema", "update_schema"],
                }),
                _ => None,
            }
        } else {
            None
        }
    }

    fn extract_schemas_from_call(&self, call_expr: &ast::ExprCall) -> Vec<String> {
        let mut schemas = Vec::new();

        for arg in &call_expr.args {
            // args is Vec<Box<Expr>>, Box automatically dereferences through Deref
            // Use &*arg to get &Expr
            if let ast::Expr::Name(name) = &*arg {
                schemas.push(name.id.to_string());
            } else if let ast::Expr::Attribute(attr) = &*arg {
                // Handle qualified names like app.schemas.UserRead
                let base = self.expr_to_string(attr.value.as_ref());
                let full_name = format!("{}.{}", base, attr.attr);
                schemas.push(full_name);
            }
        }

        schemas
    }

    fn extract_string_value(&self, expr: &ast::Expr) -> Option<String> {
        match expr {
            ast::Expr::Constant(constant) => match &constant.value {
                ast::Constant::Str(s) => Some(s.clone()),
                _ => None,
            },
            ast::Expr::JoinedStr(joined_str) => {
                // Handle f-strings - simplified version
                let mut result = String::new();
                for value in &joined_str.values {
                    // joined_str.values is Vec<Box<Expr>>, Box automatically dereferences through Deref
                    // Use &*value to get &Expr
                    if let ast::Expr::Constant(constant) = &*value {
                        if let ast::Constant::Str(s) = &constant.value {
                            result.push_str(s);
                        }
                    }
                }
                Some(result)
            }
            _ => None,
        }
    }

    fn extract_prefix_from_context(&self, _call_expr: &ast::ExprCall) -> String {
        // Default prefix - in real implementation, this would be extracted from parent include_router
        String::new()
    }

    fn expr_to_string(&self, expr: &ast::Expr) -> String {
        match expr {
            ast::Expr::Name(name) => name.id.to_string(),
            ast::Expr::Attribute(attr) => {
                format!("{}.{}", self.expr_to_string(attr.value.as_ref()), attr.attr)
            }
            _ => String::new(),
        }
    }

    /// Creates virtual Route nodes in the call graph for dynamic endpoints
    pub fn create_virtual_routes(
        &self,
        graph: &mut CallGraph,
        endpoints: &[DynamicEndpoint],
        main_file: &Path,
    ) -> Vec<NodeId> {
        let mut route_nodes = Vec::new();

        for endpoint in endpoints {
            let location = Location {
                file: main_file.to_string_lossy().to_string(),
                line: 0, // Will be set properly if we track line numbers
                column: None,
            };

            // Create a virtual handler node (placeholder)
            let handler_node = NodeId::from(graph.add_node(CallNode::Function {
                name: format!("{:?} {}", endpoint.method, endpoint.path),
                file: main_file.to_path_buf(),
                line: 0,
                parameters: vec![],
                return_type: None,
            }));

            let route_node = NodeId::from(graph.add_node(CallNode::Route {
                path: endpoint.path.clone(),
                method: endpoint.method,
                handler: handler_node,
                location: location.clone(),
            }));

            // Add edge from route to handler
            graph.add_edge(
                route_node.0,
                handler_node.0,
                dc_core::call_graph::CallEdge::Call {
                    caller: route_node,
                    callee: handler_node,
                    argument_mapping: Vec::new(),
                    location,
                },
            );

            route_nodes.push(route_node);

            if self.verbose {
                eprintln!(
                    "[DEBUG] Created virtual route: {} {}",
                    format!("{:?}", endpoint.method),
                    endpoint.path
                );
            }
        }

        route_nodes
    }
}
