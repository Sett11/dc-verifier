use anyhow::Result;
use dc_core::call_graph::{CallGraph, CallNode, DynamicEndpoint, HttpMethod, RouterGenerator};
use dc_core::models::{Location, NodeId};
use rustpython_parser::ast;
use rustpython_parser::{parse, Mode};
use serde::Deserialize;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use tracing::debug;

use crate::utils::parse_http_method;

/// Configuration for dynamic route generators
/// This mirrors the structure in dc-cli/src/config.rs
#[derive(Debug, Deserialize, Clone)]
pub struct DynamicRoutesConfig {
    /// List of router generator configurations
    pub generators: Vec<RouterGeneratorConfig>,
}

/// Configuration for a single router generator
#[derive(Debug, Deserialize, Clone)]
pub struct RouterGeneratorConfig {
    /// Module path (e.g., "fastapi_users")
    pub module: String,
    /// Method name (e.g., "get_register_router")
    pub method: String,
    /// List of endpoints this generator creates
    pub endpoints: Vec<EndpointConfig>,
    /// Schema parameter mapping (which argument is request/response schema)
    pub schema_params: Vec<String>,
}

/// Configuration for a single endpoint
#[derive(Debug, Deserialize, Clone)]
pub struct EndpointConfig {
    /// Endpoint path (e.g., "/register")
    pub path: String,
    /// HTTP method (e.g., "GET", "POST")
    pub method: String,
    /// Index of argument for request schema (if any)
    pub request_schema_param: Option<usize>,
    /// Index of argument for response schema (if any)
    pub response_schema_param: Option<usize>,
}

/// Configuration for fastapi_users router generators
struct FastAPIUsersRouter {
    #[allow(dead_code)]
    method_name: String,
    endpoints: Vec<DynamicEndpoint>,
    schema_params: Vec<String>,
}

/// Analyzer for dynamically generated FastAPI routes (e.g., fastapi_users)
pub struct DynamicRoutesAnalyzer {
    /// Cache of include_router calls with their prefixes (path -> prefix)
    include_router_prefixes: HashMap<PathBuf, Vec<(usize, String)>>,
    /// Configurable router generators from config file
    router_generators: Vec<RouterGeneratorConfig>,
    /// Cache of parsed ASTs for variable resolution
    file_asts: HashMap<PathBuf, ast::Mod>,
    /// Registered router generators
    generators: Vec<Box<dyn RouterGenerator>>,
}

impl DynamicRoutesAnalyzer {
    pub fn new() -> Self {
        let mut analyzer = Self {
            include_router_prefixes: HashMap::new(),
            router_generators: Vec::new(),
            file_asts: HashMap::new(),
            generators: Vec::new(),
        };

        // Регистрировать встроенные генераторы
        analyzer.register_default_generators();
        analyzer
    }

    fn register_default_generators(&mut self) {
        self.generators.push(Box::new(FastAPIUsersRouterGenerator));
    }

    /// Register a custom router generator
    pub fn register_generator(&mut self, generator: Box<dyn RouterGenerator>) {
        self.generators.push(generator);
    }

    /// Sets the dynamic routes configuration
    pub fn with_config(mut self, config: Option<DynamicRoutesConfig>) -> Self {
        if let Some(dynamic_routes_config) = config {
            self.router_generators = dynamic_routes_config.generators;
        }
        self
    }

    /// Analyzes main.py file for dynamic route generation
    /// Returns a list of dynamic endpoints found
    pub fn analyze_main_file(&mut self, main_file: &Path) -> Result<Vec<DynamicEndpoint>> {
        let source = std::fs::read_to_string(main_file)?;
        let ast = parse(&source, Mode::Module, main_file.to_string_lossy().as_ref())?;

        // Кэшировать AST для разрешения переменных
        self.file_asts.insert(main_file.to_path_buf(), ast.clone());

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
        // Получить AST из кэша
        let file_ast = self.file_asts.get(file);

        if let ast::Stmt::Expr(expr_stmt) = stmt {
            if let ast::Expr::Call(call_expr) = expr_stmt.value.as_ref() {
                if let ast::Expr::Attribute(attr) = call_expr.func.as_ref() {
                    if attr.attr.as_str() == "include_router" {
                        // Extract prefix from keyword arguments
                        for kw in &call_expr.keywords {
                            if let Some(arg_name) = &kw.arg {
                                if arg_name.as_str() == "prefix" {
                                    if let Some(prefix_str) =
                                        self.extract_string_value(&kw.value, file_ast)
                                    {
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

        // Получить AST из кэша
        let file_ast = self.file_asts.get(current_file);

        // Проверить зарегистрированные генераторы
        for generator in &self.generators {
            if generator.can_handle(call_expr) {
                let mut router_endpoints =
                    generator.analyze_call(call_expr, current_file, file_ast)?;
                let schemas = generator.extract_schemas(call_expr);
                let prefix = self.extract_prefix_from_context(call_expr, current_file);

                // Применить prefix и schemas
                for endpoint in &mut router_endpoints {
                    endpoint.path = format!("{}{}", prefix, endpoint.path);
                    self.apply_schemas_to_endpoints(endpoint, &schemas);
                }

                endpoints.extend(router_endpoints);
                return Ok(());
            }
        }

        // Fallback на существующую логику (для обратной совместимости)
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
        current_file: &Path,
        endpoints: &mut Vec<DynamicEndpoint>,
    ) -> Result<()> {
        // Получить AST из кэша
        let file_ast = self.file_asts.get(current_file);

        // Extract router from first argument
        let router_call = call_expr.args.first();

        // Extract prefix from keyword arguments
        let mut prefix = String::new();
        for kw in &call_expr.keywords {
            if let Some(arg_name) = &kw.arg {
                if arg_name.as_str() == "prefix" {
                    if let Some(prefix_str) = self.extract_string_value(&kw.value, file_ast) {
                        prefix = prefix_str;
                    }
                }
            }
        }

        // Analyze router call if it's a router generator
        if let Some(router_expr) = router_call {
            let expr_ref: &ast::Expr = router_expr;
            if let ast::Expr::Call(ref router_call_expr) = expr_ref {
                // Получить AST из кэша
                let file_ast = self.file_asts.get(current_file);

                // Сначала проверить зарегистрированные генераторы
                let mut handled = false;
                for generator in &self.generators {
                    if generator.can_handle(router_call_expr) {
                        let mut router_endpoints =
                            generator.analyze_call(router_call_expr, current_file, file_ast)?;
                        let schemas = generator.extract_schemas(router_call_expr);

                        // Apply prefix and schemas
                        for endpoint in &mut router_endpoints {
                            endpoint.path = format!("{}{}", prefix, endpoint.path);
                            self.apply_schemas_to_endpoints(endpoint, &schemas);
                        }

                        endpoints.extend(router_endpoints);
                        handled = true;
                        break;
                    }
                }

                // Fallback на существующую логику
                if !handled {
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
        }

        Ok(())
    }

    /// Applies schemas to an endpoint using index-based approach
    fn apply_schemas_to_endpoints(&self, endpoint: &mut DynamicEndpoint, schemas: &[String]) {
        if schemas.is_empty() {
            return;
        }

        // Use index-based approach if available
        if let Some(idx) = endpoint.response_schema_param_index {
            if idx < schemas.len() {
                endpoint.response_schema = Some(schemas[idx].clone());
            }
        }

        if let Some(idx) = endpoint.request_schema_param_index {
            if idx < schemas.len() {
                endpoint.request_schema = Some(schemas[idx].clone());
            }
        }
    }

    /// Applies schemas to an endpoint based on router configuration (legacy method)
    fn apply_schemas_to_endpoint(
        &self,
        endpoint: &mut DynamicEndpoint,
        router_info: &FastAPIUsersRouter,
        schemas: &[String],
    ) {
        if schemas.is_empty() {
            return;
        }

        // Use index-based approach if available
        if let Some(idx) = endpoint.response_schema_param_index {
            if idx < schemas.len() {
                endpoint.response_schema = Some(schemas[idx].clone());
            }
        } else if router_info
            .schema_params
            .contains(&"response_schema".to_string())
        {
            // Fallback to old behavior
            endpoint.response_schema = Some(schemas[0].clone());
        }

        if let Some(idx) = endpoint.request_schema_param_index {
            if idx < schemas.len() {
                endpoint.request_schema = Some(schemas[idx].clone());
            }
        } else if router_info
            .schema_params
            .contains(&"request_schema".to_string())
            && schemas.len() > 1
        {
            // Fallback to old behavior
            endpoint.request_schema = Some(schemas[1].clone());
        }
    }

    fn identify_router_generator(&self, call_expr: &ast::ExprCall) -> Option<FastAPIUsersRouter> {
        // Check if this is a fastapi_users router generator
        // call_expr.func is Box<Expr>, so we use as_ref() to get &Expr
        if let ast::Expr::Attribute(attr) = call_expr.func.as_ref() {
            let method_name = attr.attr.as_str();
            let base_name = self.expr_to_string(attr.value.as_ref());

            // First, check configurable generators
            for generator_config in &self.router_generators {
                if base_name.contains(&generator_config.module)
                    && method_name == generator_config.method
                {
                    return Some(self.create_router_from_config(generator_config));
                }
            }

            // Fallback: check hardcoded generators
            // Check if this is a known fastapi_users router generator method
            // We accept it if:
            // 1. The method name matches known patterns (get_*_router)
            // 2. OR the base name contains fastapi_users
            let is_fastapi_users_call = base_name.contains("fastapi_users")
                || base_name.contains("fastapi")
                || method_name.starts_with("get_") && method_name.ends_with("_router");

            if !is_fastapi_users_call {
                return None;
            }

            match method_name {
                "get_register_router" => Some(FastAPIUsersRouter {
                    method_name: "get_register_router".to_string(),
                    endpoints: vec![DynamicEndpoint {
                        path: "/register".to_string(),
                        method: HttpMethod::Post,
                        request_schema: None,
                        response_schema: None,
                        request_schema_param_index: Some(1),
                        response_schema_param_index: Some(0),
                    }],
                    schema_params: vec![
                        "response_schema".to_string(),
                        "request_schema".to_string(),
                    ],
                }),
                "get_auth_router" => Some(FastAPIUsersRouter {
                    method_name: "get_auth_router".to_string(),
                    endpoints: vec![
                        DynamicEndpoint {
                            path: "/login".to_string(),
                            method: HttpMethod::Post,
                            request_schema: None,
                            response_schema: Some("BearerResponse".to_string()),
                            request_schema_param_index: None,
                            response_schema_param_index: None,
                        },
                        DynamicEndpoint {
                            path: "/logout".to_string(),
                            method: HttpMethod::Post,
                            request_schema: None,
                            response_schema: None,
                            request_schema_param_index: None,
                            response_schema_param_index: None,
                        },
                    ],
                    schema_params: vec![],
                }),
                "get_reset_password_router" => Some(FastAPIUsersRouter {
                    method_name: "get_reset_password_router".to_string(),
                    endpoints: vec![
                        DynamicEndpoint {
                            path: "/forgot-password".to_string(),
                            method: HttpMethod::Post,
                            request_schema: None,
                            response_schema: None,
                            request_schema_param_index: None,
                            response_schema_param_index: None,
                        },
                        DynamicEndpoint {
                            path: "/reset-password".to_string(),
                            method: HttpMethod::Post,
                            request_schema: None,
                            response_schema: None,
                            request_schema_param_index: None,
                            response_schema_param_index: None,
                        },
                    ],
                    schema_params: vec![],
                }),
                "get_verify_router" => Some(FastAPIUsersRouter {
                    method_name: "get_verify_router".to_string(),
                    endpoints: vec![
                        DynamicEndpoint {
                            path: "/request-verify-token".to_string(),
                            method: HttpMethod::Post,
                            request_schema: None,
                            response_schema: None,
                            request_schema_param_index: None,
                            response_schema_param_index: Some(0),
                        },
                        DynamicEndpoint {
                            path: "/verify".to_string(),
                            method: HttpMethod::Post,
                            request_schema: None,
                            response_schema: None,
                            request_schema_param_index: None,
                            response_schema_param_index: Some(0),
                        },
                    ],
                    schema_params: vec!["response_schema".to_string()],
                }),
                "get_users_router" => Some(FastAPIUsersRouter {
                    method_name: "get_users_router".to_string(),
                    endpoints: vec![
                        DynamicEndpoint {
                            path: "/me".to_string(),
                            method: HttpMethod::Get,
                            request_schema: None,
                            response_schema: None,
                            request_schema_param_index: None,
                            response_schema_param_index: Some(0),
                        },
                        DynamicEndpoint {
                            path: "/{id}".to_string(),
                            method: HttpMethod::Get,
                            request_schema: None,
                            response_schema: None,
                            request_schema_param_index: None,
                            response_schema_param_index: Some(0),
                        },
                        DynamicEndpoint {
                            path: "/{id}".to_string(),
                            method: HttpMethod::Patch,
                            request_schema: None,
                            response_schema: None,
                            request_schema_param_index: Some(1),
                            response_schema_param_index: Some(0),
                        },
                        DynamicEndpoint {
                            path: "/{id}".to_string(),
                            method: HttpMethod::Delete,
                            request_schema: None,
                            response_schema: None,
                            request_schema_param_index: None,
                            response_schema_param_index: None,
                        },
                    ],
                    schema_params: vec!["response_schema".to_string(), "update_schema".to_string()],
                }),
                _ => None,
            }
        } else {
            None
        }
    }

    /// Creates a FastAPIUsersRouter from configuration
    fn create_router_from_config(&self, config: &RouterGeneratorConfig) -> FastAPIUsersRouter {
        let mut endpoints = Vec::new();

        for endpoint_config in &config.endpoints {
            let method = parse_http_method(&endpoint_config.method);
            endpoints.push(DynamicEndpoint {
                path: endpoint_config.path.clone(),
                method,
                request_schema: None,
                response_schema: None,
                request_schema_param_index: endpoint_config.request_schema_param,
                response_schema_param_index: endpoint_config.response_schema_param,
            });
        }

        FastAPIUsersRouter {
            method_name: config.method.clone(),
            endpoints,
            schema_params: config.schema_params.clone(),
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

    fn extract_string_value(
        &self,
        expr: &ast::Expr,
        file_ast: Option<&ast::Mod>,
    ) -> Option<String> {
        match expr {
            ast::Expr::Constant(constant) => match &constant.value {
                ast::Constant::Str(s) => Some(s.clone()),
                _ => None,
            },
            ast::Expr::JoinedStr(joined_str) => {
                // Handle f-strings - improved version that handles variables
                let mut result = String::new();
                for value in &joined_str.values {
                    // joined_str.values is Vec<Box<Expr>>, value is &Box<Expr>, which coerces to &Expr via Deref
                    let expr_ref: &ast::Expr = value;
                    match expr_ref {
                        ast::Expr::Constant(constant) => {
                            if let ast::Constant::Str(s) = &constant.value {
                                result.push_str(s);
                            }
                        }
                        ast::Expr::FormattedValue(formatted) => {
                            // Handle formatted values in f-strings (e.g., {variable})
                            // Try to extract the variable name
                            let var_name = self.expr_to_string(formatted.value.as_ref());
                            if !var_name.is_empty() {
                                // Попытаться разрешить из контекста
                                let mut visited = std::collections::HashSet::new();
                                if let Some(resolved_value) = file_ast.and_then(|ast| {
                                    self.resolve_variable_from_context(&var_name, ast, &mut visited)
                                }) {
                                    result.push_str(&resolved_value);
                                } else {
                                    // Fallback: известные константы
                                    result.push_str(&self.resolve_known_constant(&var_name));
                                }
                            }
                        }
                        _ => {
                            // Unknown expression type in f-string, skip or use placeholder
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

    /// Resolves a variable from the file's AST context
    fn resolve_variable_from_context(
        &self,
        var_name: &str,
        file_ast: &ast::Mod,
        visited: &mut std::collections::HashSet<String>,
    ) -> Option<String> {
        // Предотвратить циклические ссылки
        if visited.contains(var_name) {
            return None;
        }
        visited.insert(var_name.to_string());

        if let ast::Mod::Module(module) = file_ast {
            // Поиск в прямом порядке (сверху вниз)
            for stmt in &module.body {
                // Обработка Stmt::Assign
                if let ast::Stmt::Assign(assign) = stmt {
                    for target in &assign.targets {
                        // target is Box<Expr>, use as_ref() to get &Expr
                        let target_expr: &ast::Expr = target;
                        if let ast::Expr::Name(name) = target_expr {
                            if name.id.as_str() == var_name {
                                // Рекурсивно извлечь значение
                                return self.extract_string_value(&assign.value, Some(file_ast));
                            }
                        }
                    }
                }

                // Обработка Stmt::AnnAssign (типизированное присваивание)
                if let ast::Stmt::AnnAssign(ann_assign) = stmt {
                    // ann_assign.target is Box<Expr>
                    let target_expr: &ast::Expr = &ann_assign.target;
                    if let ast::Expr::Name(name) = target_expr {
                        if name.id.as_str() == var_name {
                            if let Some(value) = &ann_assign.value {
                                return self.extract_string_value(value, Some(file_ast));
                            }
                        }
                    }
                }
            }
        }
        None
    }

    /// Resolves known constants from fastapi_users and other libraries
    fn resolve_known_constant(&self, var_name: &str) -> String {
        // Известные константы из fastapi_users и других библиотек
        match var_name {
            "AUTH_URL_PATH" => "/auth".to_string(),
            "USERS_URL_PATH" => "/users".to_string(),
            "REGISTER_URL_PATH" => "/register".to_string(),
            "VERIFY_URL_PATH" => "/verify".to_string(),
            "RESET_PASSWORD_URL_PATH" => "/reset-password".to_string(),
            _ => format!("{{{}}}", var_name), // Плейсхолдер как fallback
        }
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
                request_schema: None,
                response_schema: None,
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

            debug!(
                http_method = ?endpoint.method,
                route_path = %endpoint.path,
                "Created virtual route"
            );
        }

        route_nodes
    }
}

/// Router generator for fastapi_users library
pub struct FastAPIUsersRouterGenerator;

impl FastAPIUsersRouterGenerator {
    /// Helper method to convert expression to string
    fn expr_to_string(&self, expr: &ast::Expr) -> String {
        match expr {
            ast::Expr::Name(name) => name.id.to_string(),
            ast::Expr::Attribute(attr) => {
                format!("{}.{}", self.expr_to_string(attr.value.as_ref()), attr.attr)
            }
            _ => String::new(),
        }
    }
}

impl RouterGenerator for FastAPIUsersRouterGenerator {
    fn module_name(&self) -> &str {
        "fastapi_users"
    }

    fn can_handle(&self, call_expr: &ast::ExprCall) -> bool {
        if let ast::Expr::Attribute(attr) = call_expr.func.as_ref() {
            let method_name = attr.attr.as_str();
            let base_name = self.expr_to_string(attr.value.as_ref());

            // More specific matching: require both fastapi_users module AND specific method pattern
            (base_name == "fastapi_users" || base_name.ends_with(".fastapi_users"))
                && (method_name.starts_with("get_") && method_name.ends_with("_router"))
        } else {
            false
        }
    }

    fn analyze_call(
        &self,
        call_expr: &ast::ExprCall,
        _current_file: &Path,
        _file_ast: Option<&ast::Mod>,
    ) -> Result<Vec<DynamicEndpoint>> {
        if let ast::Expr::Attribute(attr) = call_expr.func.as_ref() {
            let method_name = attr.attr.as_str();

            let endpoints = match method_name {
                "get_register_router" => vec![DynamicEndpoint {
                    path: "/register".to_string(),
                    method: HttpMethod::Post,
                    request_schema: None,
                    response_schema: None,
                    request_schema_param_index: Some(1),
                    response_schema_param_index: Some(0),
                }],
                "get_auth_router" => vec![
                    DynamicEndpoint {
                        path: "/login".to_string(),
                        method: HttpMethod::Post,
                        request_schema: None,
                        response_schema: Some("BearerResponse".to_string()),
                        request_schema_param_index: None,
                        response_schema_param_index: None,
                    },
                    DynamicEndpoint {
                        path: "/logout".to_string(),
                        method: HttpMethod::Post,
                        request_schema: None,
                        response_schema: None,
                        request_schema_param_index: None,
                        response_schema_param_index: None,
                    },
                ],
                "get_reset_password_router" => vec![
                    DynamicEndpoint {
                        path: "/forgot-password".to_string(),
                        method: HttpMethod::Post,
                        request_schema: None,
                        response_schema: None,
                        request_schema_param_index: None,
                        response_schema_param_index: None,
                    },
                    DynamicEndpoint {
                        path: "/reset-password".to_string(),
                        method: HttpMethod::Post,
                        request_schema: None,
                        response_schema: None,
                        request_schema_param_index: None,
                        response_schema_param_index: None,
                    },
                ],
                "get_verify_router" => vec![
                    DynamicEndpoint {
                        path: "/request-verify-token".to_string(),
                        method: HttpMethod::Post,
                        request_schema: None,
                        response_schema: None,
                        request_schema_param_index: None,
                        response_schema_param_index: Some(0),
                    },
                    DynamicEndpoint {
                        path: "/verify".to_string(),
                        method: HttpMethod::Post,
                        request_schema: None,
                        response_schema: None,
                        request_schema_param_index: None,
                        response_schema_param_index: Some(0),
                    },
                ],
                "get_users_router" => vec![
                    DynamicEndpoint {
                        path: "/me".to_string(),
                        method: HttpMethod::Get,
                        request_schema: None,
                        response_schema: None,
                        request_schema_param_index: None,
                        response_schema_param_index: Some(0),
                    },
                    DynamicEndpoint {
                        path: "/{id}".to_string(),
                        method: HttpMethod::Get,
                        request_schema: None,
                        response_schema: None,
                        request_schema_param_index: None,
                        response_schema_param_index: Some(0),
                    },
                    DynamicEndpoint {
                        path: "/{id}".to_string(),
                        method: HttpMethod::Patch,
                        request_schema: None,
                        response_schema: None,
                        request_schema_param_index: Some(1),
                        response_schema_param_index: Some(0),
                    },
                    DynamicEndpoint {
                        path: "/{id}".to_string(),
                        method: HttpMethod::Delete,
                        request_schema: None,
                        response_schema: None,
                        request_schema_param_index: None,
                        response_schema_param_index: None,
                    },
                ],
                _ => return Ok(Vec::new()),
            };

            Ok(endpoints)
        } else {
            Ok(Vec::new())
        }
    }

    fn extract_schemas(&self, call_expr: &ast::ExprCall) -> Vec<String> {
        let mut schemas = Vec::new();

        for arg in &call_expr.args {
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
}
