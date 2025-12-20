use anyhow::Result;
use dc_core::call_graph::{CallGraph, CallNode, HttpMethod};
use dc_core::models::{Location, NodeId};
use rustpython_parser::ast;
use rustpython_parser::{parse, Mode};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

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
    /// Cache of include_router calls with their prefixes (path -> prefix)
    include_router_prefixes: HashMap<PathBuf, Vec<(usize, String)>>,
}

impl DynamicRoutesAnalyzer {
    pub fn new(verbose: bool) -> Self {
        Self {
            verbose,
            include_router_prefixes: HashMap::new(),
        }
    }

    /// Analyzes main.py file for dynamic route generation
    /// Returns a list of dynamic endpoints found
    pub fn analyze_main_file(&mut self, main_file: &Path) -> Result<Vec<DynamicEndpoint>> {
        let source = std::fs::read_to_string(main_file)?;
        let ast = parse(&source, Mode::Module, main_file.to_string_lossy().as_ref())?;

        // First pass: collect all include_router calls with their prefixes and line numbers
        if let ast::Mod::Module(module) = &ast {
            for (idx, stmt) in module.body.iter().enumerate() {
                self.collect_include_router_prefixes(stmt, main_file, idx);
            }
        }

        let mut endpoints = Vec::new();

        // Second pass: analyze statements and use collected prefixes
        if let ast::Mod::Module(module) = ast {
            for stmt in &module.body {
                self.analyze_statement(stmt, main_file, &mut endpoints)?;
            }
        }

        Ok(endpoints)
    }

    /// Collects include_router calls with their prefixes for later use
    fn collect_include_router_prefixes(&mut self, stmt: &ast::Stmt, file: &Path, line_idx: usize) {
        if let ast::Stmt::Expr(expr_stmt) = stmt {
            if let ast::Expr::Call(call_expr) = expr_stmt.value.as_ref() {
                if let ast::Expr::Attribute(attr) = call_expr.func.as_ref() {
                    if attr.attr.as_str() == "include_router" {
                        // Extract prefix from keyword arguments
                        for kw in &call_expr.keywords {
                            if let Some(arg_name) = &kw.arg {
                                if arg_name.as_str() == "prefix" {
                                    if let Some(prefix_str) = self.extract_string_value(&kw.value) {
                                        self.include_router_prefixes
                                            .entry(file.to_path_buf())
                                            .or_default()
                                            .push((line_idx, prefix_str));
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    fn analyze_statement(
        &self,
        stmt: &ast::Stmt,
        current_file: &Path,
        endpoints: &mut Vec<DynamicEndpoint>,
    ) -> Result<()> {
        if let ast::Stmt::Expr(expr_stmt) = stmt {
            // Check for app.include_router(...) method calls
            if let ast::Expr::Call(call_expr) = expr_stmt.value.as_ref() {
                // Check if this is an include_router call
                if let ast::Expr::Attribute(attr) = call_expr.func.as_ref() {
                    if attr.attr.as_str() == "include_router" {
                        self.analyze_include_router(call_expr, current_file, endpoints)?;
                        return Ok(());
                    }
                }
                // Otherwise, analyze as regular call
                self.analyze_call(call_expr, current_file, endpoints)?;
            }
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
            let prefix = self.extract_prefix_from_context(call_expr, current_file);
            let mut router_endpoints = router_info.endpoints.clone();

            // Extract schemas from call arguments
            let schemas = self.extract_schemas_from_call(call_expr);

            // Apply prefix and schemas to endpoints
            for endpoint in &mut router_endpoints {
                endpoint.path = format!("{}{}", prefix, endpoint.path);
                self.apply_schemas_to_endpoint(endpoint, &router_info, &schemas);
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
            // router_expr is &Box<Expr>, which automatically coerces to &Expr via Deref
            // We can pattern match directly on router_expr since &Box<Expr> -> &Expr coercion happens
            // Use explicit coercion: &*router_expr gives &Box<Expr>, then * gives Box<Expr>, then & gives &Expr
            // Actually, just use router_expr directly - Rust will coerce &Box<Expr> to &Expr automatically
            let expr_ref: &ast::Expr = router_expr;
            if let ast::Expr::Call(ref router_call_expr) = expr_ref {
                if let Some(router_info) = self.identify_router_generator(router_call_expr) {
                    // Extract schemas from router call arguments
                    let schemas = self.extract_schemas_from_call(router_call_expr);

                    let mut router_endpoints = router_info.endpoints.clone();

                    // Apply prefix and schemas
                    for endpoint in &mut router_endpoints {
                        endpoint.path = format!("{}{}", prefix, endpoint.path);
                        self.apply_schemas_to_endpoint(endpoint, &router_info, &schemas);
                    }

                    endpoints.extend(router_endpoints);
                }
            }
        }

        Ok(())
    }

    /// Applies schemas to an endpoint based on router configuration
    fn apply_schemas_to_endpoint(
        &self,
        endpoint: &mut DynamicEndpoint,
        router_info: &FastAPIUsersRouter,
        schemas: &[String],
    ) {
        if schemas.is_empty() {
            return;
        }

        if router_info.schema_params.contains(&"response_schema") {
            endpoint.response_schema = Some(schemas[0].clone());
        }
        if router_info.schema_params.contains(&"request_schema") && schemas.len() > 1 {
            endpoint.request_schema = Some(schemas[1].clone());
        }
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
            // args is Vec<Box<Expr>>, arg is &Box<Expr>, which coerces to &Expr via Deref
            let expr_ref: &ast::Expr = arg;
            if let ast::Expr::Name(name) = expr_ref {
                schemas.push(name.id.to_string());
            } else if let ast::Expr::Attribute(attr) = expr_ref {
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
                    // joined_str.values is Vec<Box<Expr>>, value is &Box<Expr>, which coerces to &Expr via Deref
                    let expr_ref: &ast::Expr = value;
                    if let ast::Expr::Constant(constant) = expr_ref {
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

    /// Extracts prefix from context by finding the nearest include_router call
    /// This is a simplified implementation that looks for include_router calls in the same file
    fn extract_prefix_from_context(
        &self,
        _call_expr: &ast::ExprCall,
        current_file: &Path,
    ) -> String {
        // Try to find include_router calls in the same file
        if let Some(prefixes) = self.include_router_prefixes.get(current_file) {
            // For now, return the first prefix found
            // In a more sophisticated implementation, we would match by line number or AST position
            if let Some((_, prefix)) = prefixes.first() {
                return prefix.clone();
            }
        }
        // Default: no prefix
        String::new()
    }

    #[allow(clippy::only_used_in_recursion)]
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
                    "[DEBUG] Created virtual route: {:?} {}",
                    endpoint.method, endpoint.path
                );
            }
        }

        route_nodes
    }
}
