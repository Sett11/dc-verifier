use anyhow::{Context, Result};
use dc_core::call_graph::{CallEdge, CallGraph, CallNode, HttpMethod};
use dc_core::models::{Location, NodeId};
use dc_core::parsers::{Call, TypeScriptParser};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

/// TypeScript call graph builder
pub struct TypeScriptCallGraphBuilder {
    graph: CallGraph,
    src_paths: Vec<PathBuf>,
    parser: TypeScriptParser,
    processed_files: HashSet<PathBuf>,
    module_nodes: HashMap<PathBuf, NodeId>,
    function_nodes: HashMap<String, NodeId>,
    project_root: Option<PathBuf>,
    /// Maximum recursion depth (None = unlimited)
    max_depth: Option<usize>,
    /// Current recursion depth
    current_depth: usize,
    /// Enable verbose debug output
    verbose: bool,
}

impl TypeScriptCallGraphBuilder {
    /// Creates a new builder
    pub fn new(src_paths: Vec<PathBuf>) -> Self {
        Self {
            graph: CallGraph::new(),
            src_paths,
            parser: TypeScriptParser::new(),
            processed_files: HashSet::new(),
            module_nodes: HashMap::new(),
            function_nodes: HashMap::new(),
            project_root: None,
            max_depth: None,
            current_depth: 0,
            verbose: false,
        }
    }

    /// Sets the maximum recursion depth
    pub fn with_max_depth(mut self, max_depth: Option<usize>) -> Self {
        self.max_depth = max_depth;
        self
    }

    /// Sets the verbose flag for debug output
    pub fn with_verbose(mut self, verbose: bool) -> Self {
        self.verbose = verbose;
        self
    }

    /// Builds graph for TypeScript project
    pub fn build_graph(mut self) -> Result<CallGraph> {
        // 1. Find all .ts/.tsx files in src_paths
        let mut files = Vec::new();
        for src_path in &self.src_paths {
            self.find_ts_files(src_path, &mut files)?;
        }

        // 2. Determine project root
        if let Some(first_file) = files.first() {
            if let Some(parent) = first_file.parent() {
                self.project_root = Some(parent.to_path_buf());
            }
        }

        // 3. Parse and process each file
        for file in files {
            if let Err(err) = self.process_file(&file) {
                eprintln!("Error processing file {:?}: {}", file, err);
                // Continue processing other files
            }
        }

        Ok(self.graph)
    }

    /// Processes a single TypeScript file
    fn process_file(&mut self, file: &Path) -> Result<()> {
        let normalized = Self::normalize_path(file);

        if self.processed_files.contains(&normalized) {
            return Ok(()); // Already processed
        }

        // Check recursion depth limit
        if let Some(max_depth) = self.max_depth {
            if self.current_depth >= max_depth {
                return Err(anyhow::Error::from(
                    dc_core::error::GraphError::MaxDepthExceeded(max_depth),
                ));
            }
        }

        self.current_depth += 1;

        let result = (|| -> Result<()> {
            let (module, source, converter) = self
                .parser
                .parse_file(&normalized)
                .with_context(|| format!("Failed to parse {:?}", normalized))?;

            // Create module node
            let module_node = self.get_or_create_module_node(&normalized)?;
            self.processed_files.insert(normalized.clone());

            let file_path_str = normalized.to_string_lossy().to_string();

            // Extract imports
            let imports = self
                .parser
                .extract_imports(&module, &file_path_str, &converter);
            for import in imports {
                if let Err(err) = self.process_import(module_node, &import, &normalized) {
                    eprintln!(
                        "Error processing import '{}' from {:?}: {}",
                        import.path, normalized, err
                    );
                }
            }

            // Extract calls
            let calls = self
                .parser
                .extract_calls(&module, &file_path_str, &converter, &source);
            for call in &calls {
                if let Err(err) = self.process_call(module_node, call, &normalized) {
                    eprintln!(
                        "Error processing call '{}' from {:?}: {}",
                        call.name, normalized, err
                    );
                }
            }

            // Detect API calls and create Route nodes
            for call in calls {
                if let Some(api_call) = self.detect_api_call(&call) {
                    if let Err(err) =
                        self.create_route_from_api_call(api_call, &normalized, &file_path_str)
                    {
                        if self.verbose {
                            eprintln!(
                                "[DEBUG] Failed to create route from API call '{}': {}",
                                call.name, err
                            );
                        }
                    }
                }
            }

            // Extract functions and classes
            let functions_and_classes =
                self.parser
                    .extract_functions_and_classes(&module, &file_path_str, &converter);
            for item in functions_and_classes {
                match item {
                    dc_core::parsers::FunctionOrClass::Function {
                        name,
                        line,
                        parameters,
                        return_type,
                        is_async,
                        ..
                    } => {
                        let function_node = self.get_or_create_function_node_with_details(
                            &name,
                            &normalized,
                            line,
                            parameters,
                            return_type,
                            is_async,
                        );
                        self.graph.add_edge(
                            *module_node,
                            *function_node,
                            CallEdge::Call {
                                caller: module_node,
                                callee: function_node,
                                argument_mapping: Vec::new(),
                                location: dc_core::models::Location {
                                    file: file_path_str.clone(),
                                    line,
                                    column: None,
                                },
                            },
                        );
                    }
                    dc_core::parsers::FunctionOrClass::Class {
                        name,
                        line,
                        methods,
                        ..
                    } => {
                        let class_node = self.get_or_create_class_node(&name, &normalized, line);
                        self.graph.add_edge(
                            *module_node,
                            *class_node,
                            CallEdge::Call {
                                caller: module_node,
                                callee: class_node,
                                argument_mapping: Vec::new(),
                                location: dc_core::models::Location {
                                    file: file_path_str.clone(),
                                    line,
                                    column: None,
                                },
                            },
                        );

                        for method in methods {
                            let method_node = self.get_or_create_method_node(
                                &method.name,
                                class_node,
                                &normalized,
                                method.line,
                                method.parameters,
                                method.return_type,
                                method.is_async,
                                method.is_static,
                            );
                            self.graph.add_edge(
                                *class_node,
                                *method_node,
                                CallEdge::Call {
                                    caller: class_node,
                                    callee: method_node,
                                    argument_mapping: Vec::new(),
                                    location: dc_core::models::Location {
                                        file: file_path_str.clone(),
                                        line: method.line,
                                        column: None,
                                    },
                                },
                            );
                        }
                    }
                }
            }

            Ok(())
        })();

        self.current_depth -= 1;
        result
    }

    /// Processes an import
    fn process_import(
        &mut self,
        from: NodeId,
        import: &dc_core::parsers::Import,
        current_file: &Path,
    ) -> Result<NodeId> {
        let import_path = match self.resolve_import_path(&import.path, current_file) {
            Ok(path) => path,
            Err(err) => {
                if import.path.starts_with('.') {
                    return Err(err);
                }
                if !import.path.contains('/') || import.path.starts_with('@') {
                    return Ok(from);
                }
                return Err(err);
            }
        };

        let module_node = self.get_or_create_module_node(&import_path)?;

        self.graph.add_edge(
            *from,
            *module_node,
            CallEdge::Import {
                from,
                to: module_node,
                import_path: import.path.clone(),
                file: import_path.clone(),
            },
        );

        // Recursively process the imported module
        // Note: current_depth is managed inside process_file
        if !self.processed_files.contains(&import_path) {
            let _ = self.process_file(&import_path);
        }

        Ok(module_node)
    }

    /// Processes a function call
    fn process_call(
        &mut self,
        caller: NodeId,
        call: &dc_core::parsers::Call,
        current_file: &Path,
    ) -> Result<NodeId> {
        // Try to find function in current file or other processed files
        let callee_node = self
            .find_function_node(&call.name, current_file)
            .unwrap_or_else(|| {
                // If function not found, create virtual node
                self.get_or_create_function_node(&call.name, current_file)
            });

        let argument_mapping = call
            .arguments
            .iter()
            .enumerate()
            .map(|(idx, arg)| {
                let key = arg
                    .parameter_name
                    .clone()
                    .unwrap_or_else(|| format!("arg{}", idx));
                (key, arg.value.clone())
            })
            .collect();

        self.graph.add_edge(
            *caller,
            *callee_node,
            CallEdge::Call {
                caller,
                callee: callee_node,
                argument_mapping,
                location: call.location.clone(),
            },
        );

        Ok(callee_node)
    }

    /// Detects if a call is an API call (fetch, axios, React Query, etc.)
    fn detect_api_call(&self, call: &Call) -> Option<ApiCallInfo> {
        let name = &call.name;

        // Check for fetch(url, options)
        if name == "fetch" && !call.arguments.is_empty() {
            let url = call.arguments.first()?.value.clone();
            // Try to extract method from options (second argument)
            let method = if call.arguments.len() > 1 {
                self.extract_method_from_fetch_options(&call.arguments[1].value)
            } else {
                HttpMethod::Get
            };
            return Some(ApiCallInfo {
                path: url,
                method,
                location: call.location.clone(),
                request_type: None,
                response_type: None,
            });
        }

        // Check for axios.get/post/put/delete(url, ...)
        if name.starts_with("axios.") {
            let parts: Vec<&str> = name.split('.').collect();
            if parts.len() == 2 {
                if let Ok(method) = parts[1].parse::<HttpMethod>() {
                    let path = call.arguments.first()?.value.clone();
                    return Some(ApiCallInfo {
                        path,
                        method,
                        location: call.location.clone(),
                        request_type: None,
                        response_type: None,
                    });
                }
            }
        }

        // Check for api.get/post/put/delete(url, ...) or client.get/post/...
        let api_patterns = ["api.", "client.", "http.", "request."];
        for pattern in &api_patterns {
            if name.starts_with(pattern) {
                let parts: Vec<&str> = name.split('.').collect();
                if parts.len() >= 2 {
                    if let Ok(method) = parts[1].parse::<HttpMethod>() {
                        let path = call.arguments.first()?.value.clone();
                        return Some(ApiCallInfo {
                            path,
                            method,
                            location: call.location.clone(),
                            request_type: None,
                            response_type: None,
                        });
                    }
                }
            }
        }

        // Check for React Query hooks: useQuery, useMutation
        // These are typically used with query keys that contain URLs
        // But first check if it's Apollo Client (has gql in arguments)
        if name == "useQuery" || name == "useMutation" {
            // Check if it's Apollo Client call (has gql in arguments)
            if self.is_apollo_client_call(call) {
                // Extract types from GraphQL query
                let (request_type, response_type) = self.extract_types_from_graphql_query(call);
                // GraphQL usually uses POST
                let method = HttpMethod::Post;
                // Extract path from GraphQL operation
                let path = self.extract_path_from_graphql_query(call);
                return Some(ApiCallInfo {
                    path,
                    method,
                    location: call.location.clone(),
                    request_type,
                    response_type,
                });
            }

            // Otherwise, it's TanStack Query
            // Try to extract URL from query key (first argument)
            if let Some(first_arg) = call.arguments.first() {
                // Query keys are often arrays like ["users", id] or objects
                // For simplicity, we'll use the first argument as path
                let path = first_arg.value.clone();
                let method = if name == "useMutation" {
                    HttpMethod::Post
                } else {
                    HttpMethod::Get
                };

                // Extract request and response types from query hook generic parameters
                let (request_type, response_type) = self.extract_types_from_query_hook(call);

                return Some(ApiCallInfo {
                    path,
                    method,
                    location: call.location.clone(),
                    request_type,
                    response_type,
                });
            }
        }

        // Check for SWR hooks: useSWR, useSWRMutation
        if name == "useSWR" || name == "useSWRMutation" {
            // Try to extract URL from key (first argument)
            if let Some(first_arg) = call.arguments.first() {
                let path = first_arg.value.clone();
                let method = if name == "useSWRMutation" {
                    HttpMethod::Post
                } else {
                    HttpMethod::Get
                };

                // Extract request and response types from SWR hook generic parameters
                let (request_type, response_type) = self.extract_types_from_swr_hook(call);

                return Some(ApiCallInfo {
                    path,
                    method,
                    location: call.location.clone(),
                    request_type,
                    response_type,
                });
            }
        }

        // Check for RTK Query pattern: *.use*Query() or *.use*Mutation()
        if name.contains(".use") && (name.contains("Query") || name.contains("Mutation")) {
            let parts: Vec<&str> = name.split('.').collect();
            if parts.len() >= 2 {
                if let Some(hook_name) = parts.last() {
                    // Extract endpoint and method from hook name
                    let (endpoint, method) = self.extract_endpoint_from_rtk_hook(hook_name);
                    // Extract types
                    let (request_type, response_type) = self.extract_types_from_rtk_hook(call);
                    return Some(ApiCallInfo {
                        path: endpoint,
                        method,
                        location: call.location.clone(),
                        request_type,
                        response_type,
                    });
                }
            }
        }

        // Check for tRPC pattern: chain ending with .useQuery() or .useMutation()
        if name.ends_with(".useQuery") || name.ends_with(".useMutation") {
            // Extract path from chain (trpc.users.get.useQuery → /trpc/users.get)
            let path = self.extract_path_from_trpc_chain(name);
            let method = if name.ends_with(".useMutation") {
                HttpMethod::Post
            } else {
                HttpMethod::Get
            };
            // Extract types
            let (request_type, response_type) = self.extract_types_from_trpc_hook(call);
            return Some(ApiCallInfo {
                path,
                method,
                location: call.location.clone(),
                request_type,
                response_type,
            });
        }

        // Check for Next.js Server Actions pattern: actions.*()
        if name.starts_with("actions.") {
            // Extract path from action name
            let path = self.extract_path_from_action_name(name);
            // Extract types from server action
            let (request_type, response_type) = self.extract_types_from_server_action(call);
            return Some(ApiCallInfo {
                path,
                method: HttpMethod::Post, // Server Actions usually POST
                location: call.location.clone(),
                request_type,
                response_type,
            });
        }

        None
    }

    /// Extracts types from SWR hooks (useSWR, useSWRMutation)
    ///
    /// For useSWR<Data, Error>:
    /// - Data (1st) = response type
    /// - Error (2nd) = error type (ignored)
    ///
    /// For useSWRMutation<Data, Error, Key, Arg>:
    /// - Data (1st) = response type
    /// - Error (2nd) = error type (ignored)
    /// - Key (3rd) = key type (ignored)
    /// - Arg (4th) = request type
    fn extract_types_from_swr_hook(
        &self,
        call: &Call,
    ) -> (
        Option<dc_core::models::TypeInfo>,
        Option<dc_core::models::TypeInfo>,
    ) {
        let is_mutation = call.name == "useSWRMutation";

        match call.generic_params.len() {
            0 => (None, None),
            1 => {
                // useSWR<Data> - Data = response type
                let response = call.generic_params.first().cloned();
                (None, response)
            }
            2 => {
                // useSWR<Data, Error> - Data = response type
                let response = call.generic_params.first().cloned();
                (None, response)
            }
            4 => {
                // useSWRMutation<Data, Error, Key, Arg>
                // Data (1st) = response, Arg (4th) = request
                let response = call.generic_params.first().cloned();
                let request = if is_mutation {
                    call.generic_params.get(3).cloned()
                } else {
                    None
                };
                (request, response)
            }
            _ => {
                // Fallback: первый параметр = response
                let response = call.generic_params.first().cloned();
                (None, response)
            }
        }
    }

    /// Extracts types from TanStack Query hooks (useQuery, useMutation)
    ///
    /// For useQuery<TQueryFnData, TError, TData, TQueryKey>:
    /// - TQueryFnData (1st) = response type
    /// - TError (2nd) = error type (ignored)
    /// - TData (3rd) = transformed data type (usually same as TQueryFnData, ignored)
    /// - TQueryKey (4th) = query key type (ignored)
    ///
    /// For useMutation<TData, TError, TVariables, TContext>:
    /// - TData (1st) = response type
    /// - TError (2nd) = error type (ignored)
    /// - TVariables (3rd) = request type (variables)
    /// - TContext (4th) = context type (ignored)
    fn extract_types_from_query_hook(
        &self,
        call: &Call,
    ) -> (
        Option<dc_core::models::TypeInfo>,
        Option<dc_core::models::TypeInfo>,
    ) {
        let is_mutation = call.name == "useMutation";

        match call.generic_params.len() {
            0 => (None, None),
            1 => {
                // Only response type provided
                let response = call.generic_params.first().cloned();
                (None, response)
            }
            2 => {
                // useQuery<TQueryFnData, TError> or useMutation<TData, TError>
                let response = call.generic_params.first().cloned();
                // Error type (2nd) is ignored
                (None, response)
            }
            3 => {
                // useQuery<TQueryFnData, TError, TData> or useMutation<TData, TError, TVariables>
                let response = call.generic_params.first().cloned();
                if is_mutation {
                    // For useMutation, 3rd param is TVariables (request type)
                    let request = call.generic_params.get(2).cloned();
                    (request, response)
                } else {
                    // For useQuery, 3rd param is TData (transformed data, usually same as TQueryFnData)
                    // We use TQueryFnData (1st) as response, ignore TData
                    (None, response)
                }
            }
            4 => {
                // Full signature: useQuery<TQueryFnData, TError, TData, TQueryKey>
                //              or useMutation<TData, TError, TVariables, TContext>
                let response = call.generic_params.first().cloned();
                if is_mutation {
                    // For useMutation, 3rd param is TVariables (request type)
                    let request = call.generic_params.get(2).cloned();
                    (request, response)
                } else {
                    // For useQuery, 1st param is TQueryFnData (response type)
                    // GET requests typically don't have request body, so request is None
                    (None, response)
                }
            }
            _ => {
                // For 5+ parameters, use the same logic as 4 parameters
                // This handles extended signatures or custom hooks
                let response = call.generic_params.first().cloned();
                if is_mutation {
                    // For useMutation, 3rd param is typically TVariables
                    let request = call.generic_params.get(2).cloned();
                    (request, response)
                } else {
                    (None, response)
                }
            }
        }
    }

    /// Finds corresponding service file for a queries file
    /// For example: features/auth/api/authQueries.ts -> features/auth/api/authService.ts
    fn find_service_file(&self, queries_file: &Path) -> Option<PathBuf> {
        let file_stem = queries_file.file_stem()?.to_str()?;

        // Early return if file_stem doesn't end with "Queries"
        if !file_stem.ends_with("Queries") {
            return None;
        }

        // Build service name by replacing "Queries" suffix
        let service_name = file_stem.replace("Queries", "Service");

        // Search in src_paths
        for src_path in &self.src_paths {
            if src_path.is_dir() {
                // Directory: join with service filename
                let candidate = src_path.join(format!("{}.ts", service_name));
                if candidate.exists() {
                    return Some(candidate);
                }
                let candidate_tsx = src_path.join(format!("{}.tsx", service_name));
                if candidate_tsx.exists() {
                    return Some(candidate_tsx);
                }
            } else if src_path.is_file() {
                // File: compare stem and extension
                if let Some(src_stem) = src_path.file_stem().and_then(|s| s.to_str()) {
                    if src_stem == service_name {
                        if let Some(ext) = src_path.extension().and_then(|e| e.to_str()) {
                            if ext == "ts" || ext == "tsx" {
                                return Some(src_path.clone());
                            }
                        }
                    }
                }
            }
        }

        // Also try in the same directory as queries file
        if let Some(parent) = queries_file.parent() {
            let candidate = parent.join(format!("{}.ts", service_name));
            if candidate.exists() {
                return Some(candidate);
            }
            let candidate_tsx = parent.join(format!("{}.tsx", service_name));
            if candidate_tsx.exists() {
                return Some(candidate_tsx);
            }
        }

        None
    }

    /// Extracts request and response types from a service function
    ///
    /// Tries multiple strategies:
    /// 1. Find function by exact name
    /// 2. If not found, extract all functions and find the first one with types
    /// 3. If still not found, return None
    fn extract_service_types(
        &self,
        service_file: &Path,
        function_name: &str,
    ) -> Result<(
        Option<dc_core::models::TypeInfo>,
        Option<dc_core::models::TypeInfo>,
    )> {
        // 1. Parse the service file
        let (module, _source, converter) = self.parser.parse_file(service_file)?;
        let file_path_str = service_file.to_string_lossy().to_string();

        // 2. First try to find the function by exact name
        let function_info =
            self.parser
                .find_function_by_name(&module, function_name, &file_path_str, &converter);

        if let Some(info) = function_info {
            // 3. Extract types from found function
            // Request type = first parameter of the function
            let request_type = info.parameters.first().map(|param| param.type_info.clone());

            // Response type = return type (already handles Promise<T>)
            let response_type = info.return_type;

            return Ok((request_type, response_type));
        }

        // 4. If function not found by name, try to find any function with types
        // This handles cases where function name doesn't match exactly
        let all_functions =
            self.parser
                .extract_functions_and_classes(&module, &file_path_str, &converter);

        // Find first function with both request and response types, or at least response type
        for func_or_class in all_functions {
            if let dc_core::parsers::FunctionOrClass::Function {
                parameters,
                return_type,
                ..
            } = func_or_class
            {
                // Prefer functions with both types, but accept any with at least response type
                if return_type.is_some() || !parameters.is_empty() {
                    let request_type = parameters.first().map(|param| param.type_info.clone());
                    let response_type = return_type;

                    // If we found a function with types, use it
                    if request_type.is_some() || response_type.is_some() {
                        return Ok((request_type, response_type));
                    }
                }
            }
        }

        // 5. No suitable function found
        Ok((None, None))
    }

    /// Extracts HTTP method from fetch options object
    fn extract_method_from_fetch_options(&self, options_str: &str) -> HttpMethod {
        // Try to parse method from options string
        // This is a simple heuristic - in real code, we'd need to parse the object
        if options_str.contains("method") {
            if let Some(method_start) = options_str.find("method") {
                let after_method = &options_str[method_start..];
                if let Some(colon) = after_method.find(':') {
                    let method_part = &after_method[colon + 1..];
                    let method_clean = method_part
                        .trim()
                        .trim_matches('"')
                        .trim_matches('\'')
                        .to_uppercase();
                    if let Ok(method) = method_clean.parse::<HttpMethod>() {
                        return method;
                    }
                }
            }
        }
        HttpMethod::Get
    }

    /// Creates a Route node from an API call
    fn create_route_from_api_call(
        &mut self,
        api_call: ApiCallInfo,
        file_path: &Path,
        _file_path_str: &str,
    ) -> Result<()> {
        // Priority order for type extraction:
        // 1. Try to find corresponding service file and extract types from function
        // 2. Use types from generic parameters of API call (useQuery/useMutation)
        // 3. Fallback to default handler without types

        let (request_type, response_type) =
            if let Some(service_file) = self.find_service_file(file_path) {
                // Try to extract types from service file
                // Look for common service function names
                let function_names = ["api_handler", "handler", "service", "fetchData"];
                let mut found_types = (None, None);

                for func_name in &function_names {
                    if let Ok(types) = self.extract_service_types(&service_file, func_name) {
                        if types.0.is_some() || types.1.is_some() {
                            found_types = types;
                            break;
                        }
                    }
                }

                found_types
            } else {
                // No service file found, use types from API call generic params
                (api_call.request_type, api_call.response_type)
            };

        // Create handler with types if available
        let handler_node = match (request_type, response_type) {
            (Some(req_type), Some(resp_type)) => {
                // Both request and response types available
                self.get_or_create_function_node_with_details(
                    "api_handler",
                    file_path,
                    0,
                    vec![dc_core::call_graph::Parameter {
                        name: "request".to_string(),
                        type_info: req_type,
                        optional: false,
                        default_value: None,
                    }],
                    Some(resp_type),
                    false,
                )
            }
            (None, Some(resp_type)) => {
                // Only response type available
                self.get_or_create_function_node_with_details(
                    "api_handler",
                    file_path,
                    0,
                    Vec::new(),
                    Some(resp_type),
                    false,
                )
            }
            _ => {
                // Fallback to existing code
                self.get_or_create_function_node("api_handler", file_path)
            }
        };
        let location = api_call.location.clone();

        let route_node = NodeId::from(self.graph.add_node(CallNode::Route {
            path: api_call.path,
            method: api_call.method,
            handler: handler_node,
            location: location.clone(),
        }));

        // Link route to handler
        self.graph.add_edge(
            *route_node,
            *handler_node,
            CallEdge::Call {
                caller: route_node,
                callee: handler_node,
                argument_mapping: Vec::new(),
                location,
            },
        );

        Ok(())
    }

    /// Gets or creates a module node
    fn get_or_create_module_node(&mut self, path: &Path) -> Result<NodeId> {
        let normalized = Self::normalize_path(path);

        if let Some(node) = self.module_nodes.get(&normalized) {
            return Ok(*node);
        }

        let node = NodeId::from(self.graph.add_node(CallNode::Module {
            path: normalized.clone(),
        }));
        self.module_nodes.insert(normalized, node);
        Ok(node)
    }

    /// Gets or creates a function node
    fn get_or_create_function_node(&mut self, name: &str, file: &Path) -> NodeId {
        self.get_or_create_function_node_with_details(name, file, 0, Vec::new(), None, false)
    }

    /// Gets or creates a function node with details
    fn get_or_create_function_node_with_details(
        &mut self,
        name: &str,
        file: &Path,
        line: usize,
        parameters: Vec<dc_core::call_graph::Parameter>,
        return_type: Option<dc_core::models::TypeInfo>,
        _is_async: bool,
    ) -> NodeId {
        let key = Self::function_key(file, name);

        if let Some(node) = self.function_nodes.get(&key) {
            return *node;
        }

        let node = NodeId::from(self.graph.add_node(CallNode::Function {
            name: name.to_string(),
            file: file.to_path_buf(),
            line,
            parameters,
            return_type,
        }));
        self.function_nodes.insert(key, node);
        node
    }

    /// Gets or creates a class node
    fn get_or_create_class_node(&mut self, name: &str, file: &Path, _line: usize) -> NodeId {
        let _key = format!(
            "{}::class::{}",
            Self::normalize_path(file).to_string_lossy(),
            name
        );

        // Check if class node already exists
        for (node_idx, node) in self.graph.node_indices().zip(self.graph.node_weights()) {
            if let CallNode::Class {
                name: node_name, ..
            } = node
            {
                if node_name == name {
                    return NodeId::from(node_idx);
                }
            }
        }

        NodeId::from(self.graph.add_node(CallNode::Class {
            name: name.to_string(),
            file: file.to_path_buf(),
            methods: Vec::new(),
        }))
    }

    /// Gets or creates a method node
    #[allow(clippy::too_many_arguments)]
    fn get_or_create_method_node(
        &mut self,
        name: &str,
        class: NodeId,
        file: &Path,
        _line: usize,
        parameters: Vec<dc_core::call_graph::Parameter>,
        return_type: Option<dc_core::models::TypeInfo>,
        _is_async: bool,
        _is_static: bool,
    ) -> NodeId {
        let _key = format!(
            "{}::method::{}",
            Self::normalize_path(file).to_string_lossy(),
            name
        );

        // Check if method node already exists
        for (node_idx, node) in self.graph.node_indices().zip(self.graph.node_weights()) {
            if let CallNode::Method {
                name: node_name,
                class: node_class,
                ..
            } = node
            {
                if node_name == name && *node_class == class {
                    return NodeId::from(node_idx);
                }
            }
        }

        let node = NodeId::from(self.graph.add_node(CallNode::Method {
            name: name.to_string(),
            class,
            parameters,
            return_type,
        }));

        // Update class methods list
        if let Some(CallNode::Class { methods, .. }) = self.graph.node_weight_mut(*class) {
            methods.push(node);
        }

        node
    }

    /// Finds a function node
    fn find_function_node(&self, name: &str, current_file: &Path) -> Option<NodeId> {
        let normalized = Self::normalize_path(current_file);
        let direct_key = Self::function_key(&normalized, name);
        if let Some(node) = self.function_nodes.get(&direct_key) {
            return Some(*node);
        }

        // Search by name across all files
        self.function_nodes
            .iter()
            .find(|(key, _)| key.ends_with(&format!("::{}", name)))
            .map(|(_, node)| *node)
    }

    /// Resolves import path
    fn resolve_import_path(&self, import_path: &str, current_file: &Path) -> Result<PathBuf> {
        let normalized_current = Self::normalize_path(current_file);
        let base_dir = normalized_current
            .parent()
            .map(|p| p.to_path_buf())
            .or_else(|| self.project_root.clone())
            .unwrap_or_else(|| PathBuf::from("."));

        let candidate = if import_path.starts_with('.') {
            self.resolve_relative_import(import_path, &base_dir)
        } else {
            // Absolute imports - skip external modules for now
            return Err(anyhow::anyhow!("External module: {}", import_path));
        };

        if candidate.exists() {
            return Ok(candidate);
        }

        // Try adding extensions
        for ext in &["ts", "tsx", "js", "jsx"] {
            let mut with_ext = candidate.clone();
            with_ext.set_extension(ext);
            if with_ext.exists() {
                return Ok(with_ext);
            }
        }

        anyhow::bail!(
            "Cannot resolve import path {} from {:?}",
            import_path,
            current_file
        )
    }

    /// Resolves relative import
    fn resolve_relative_import(&self, import_path: &str, base_dir: &Path) -> PathBuf {
        let mut level = 0;
        for ch in import_path.chars() {
            if ch == '.' {
                level += 1;
            } else {
                break;
            }
        }

        let mut path = base_dir.to_path_buf();
        for _ in 1..level {
            if let Some(parent) = path.parent() {
                path = parent.to_path_buf();
            }
        }

        let remaining = import_path.trim_start_matches('.');
        if !remaining.is_empty() {
            let replaced = remaining.replace('/', std::path::MAIN_SEPARATOR_STR);
            path = path.join(replaced);
        }

        path
    }

    /// Normalizes path
    fn normalize_path(path: &Path) -> PathBuf {
        path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
    }

    /// Creates key for function
    fn function_key(path: &Path, name: &str) -> String {
        format!("{}::{}", Self::normalize_path(path).to_string_lossy(), name)
    }

    /// Extracts endpoint and HTTP method from RTK Query hook name
    ///
    /// Patterns:
    /// - useGet{Resource}Query → GET /{resource}
    /// - useCreate{Resource}Mutation → POST /{resource}
    /// - useUpdate{Resource}ByIdMutation → PUT /{resource}/:id
    /// - useDelete{Resource}Mutation → DELETE /{resource}/:id
    /// - useLazy{Resource}Query → GET /{resource}
    fn extract_endpoint_from_rtk_hook(&self, hook_name: &str) -> (String, HttpMethod) {
        let hook_lower = hook_name.to_lowercase();

        // Determine HTTP method
        let method = if hook_lower.contains("create") || hook_lower.contains("add") {
            HttpMethod::Post
        } else if hook_lower.contains("update")
            || hook_lower.contains("edit")
            || hook_lower.contains("patch")
        {
            HttpMethod::Put
        } else if hook_lower.contains("delete") || hook_lower.contains("remove") {
            HttpMethod::Delete
        } else {
            HttpMethod::Get
        };

        // Extract resource name
        let resource = hook_name
            .trim_start_matches("use")
            .trim_end_matches("Query")
            .trim_end_matches("Mutation")
            .trim_end_matches("ById")
            .trim_end_matches("Lazy");

        // Convert PascalCase to kebab-case
        let endpoint = self.pascal_to_kebab(resource);

        (format!("/{}", endpoint), method)
    }

    /// Converts PascalCase to kebab-case
    /// Example: "GetUsers" → "get-users"
    fn pascal_to_kebab(&self, input: &str) -> String {
        if input.is_empty() {
            return String::new();
        }

        let mut result = String::new();
        let chars: Vec<char> = input.chars().collect();

        for (i, ch) in chars.iter().enumerate() {
            if ch.is_uppercase() && i > 0 {
                result.push('-');
            }
            result.push(ch.to_lowercase().next().unwrap_or(*ch));
        }

        result
    }

    /// Extracts types from RTK Query hooks
    ///
    /// RTK Query hooks typically have types:
    /// - useGetUsersQuery<ResponseType>() - only response type
    /// - useCreateUserMutation<ResponseType, RequestType>() - response and request types
    fn extract_types_from_rtk_hook(
        &self,
        call: &Call,
    ) -> (
        Option<dc_core::models::TypeInfo>,
        Option<dc_core::models::TypeInfo>,
    ) {
        match call.generic_params.len() {
            0 => (None, None),
            1 => {
                // Only response type
                let response = call.generic_params.first().cloned();
                (None, response)
            }
            2 => {
                // Response and request types
                let response = call.generic_params.first().cloned();
                let request = call.generic_params.get(1).cloned();
                (request, response)
            }
            _ => {
                // Fallback: первый параметр = response
                let response = call.generic_params.first().cloned();
                (None, response)
            }
        }
    }

    /// Extracts path from tRPC call chain
    ///
    /// Examples:
    /// - trpc.users.get.useQuery → /trpc/users.get
    /// - api.users.list.useQuery → /trpc/users.list
    fn extract_path_from_trpc_chain(&self, chain: &str) -> String {
        // Remove .useQuery/.useMutation suffix
        let without_suffix = chain
            .trim_end_matches(".useQuery")
            .trim_end_matches(".useMutation");

        // Split by dots
        let parts: Vec<&str> = without_suffix.split('.').collect();

        if parts.is_empty() {
            return "/trpc".to_string();
        }

        // First part is usually "trpc" or "api", rest is the path
        let path_parts: Vec<&str> = if parts[0] == "trpc" || parts[0] == "api" {
            parts[1..].to_vec()
        } else {
            parts
        };

        // Join with dots (tRPC uses dot notation)
        let endpoint = path_parts.join(".");
        format!("/trpc/{}", endpoint)
    }

    /// Extracts types from tRPC hooks
    ///
    /// tRPC hooks typically have types:
    /// - trpc.users.get.useQuery<ResponseType>() - only response type
    /// - trpc.users.create.useMutation<ResponseType, RequestType>() - response and request types
    fn extract_types_from_trpc_hook(
        &self,
        call: &Call,
    ) -> (
        Option<dc_core::models::TypeInfo>,
        Option<dc_core::models::TypeInfo>,
    ) {
        match call.generic_params.len() {
            0 => (None, None),
            1 => {
                // Only response type
                let response = call.generic_params.first().cloned();
                (None, response)
            }
            2 => {
                // Response and request types
                let response = call.generic_params.first().cloned();
                let request = call.generic_params.get(1).cloned();
                (request, response)
            }
            _ => {
                // Fallback: первый параметр = response
                let response = call.generic_params.first().cloned();
                (None, response)
            }
        }
    }

    /// Checks if a call is an Apollo Client call
    ///
    /// Apollo Client calls typically have `gql` in arguments or GraphQL-related patterns
    fn is_apollo_client_call(&self, call: &Call) -> bool {
        // Check for gql in arguments
        let has_gql = call
            .arguments
            .iter()
            .any(|arg| arg.value.contains("gql") || arg.value.contains("graphql"));

        has_gql
    }

    /// Extracts path from GraphQL query
    ///
    /// GraphQL typically uses a single endpoint like /graphql or /api/graphql
    fn extract_path_from_graphql_query(&self, _call: &Call) -> String {
        // GraphQL typically uses a single endpoint
        // In practice, this could be /graphql, /api/graphql, etc.
        // For now, we'll use a default path
        "/graphql".to_string()
    }

    /// Extracts types from GraphQL query (Apollo Client)
    ///
    /// Apollo Client uses generic parameters:
    /// - useQuery<ResponseType, VariablesType>(...)
    /// - useMutation<ResponseType, VariablesType>(...)
    fn extract_types_from_graphql_query(
        &self,
        call: &Call,
    ) -> (
        Option<dc_core::models::TypeInfo>,
        Option<dc_core::models::TypeInfo>,
    ) {
        match call.generic_params.len() {
            0 => (None, None),
            1 => {
                // Only response type
                let response = call.generic_params.first().cloned();
                (None, response)
            }
            2 => {
                // Response and variables (request) types
                let response = call.generic_params.first().cloned();
                let request = call.generic_params.get(1).cloned();
                (request, response)
            }
            _ => {
                // Fallback: первый параметр = response
                let response = call.generic_params.first().cloned();
                (None, response)
            }
        }
    }

    /// Extracts path from Next.js Server Action name
    ///
    /// Examples:
    /// - actions.createUser → /api/createUser
    /// - actions.users.create → /api/users/create
    fn extract_path_from_action_name(&self, name: &str) -> String {
        let without_prefix = name.trim_start_matches("actions.");
        let parts: Vec<&str> = without_prefix.split('.').collect();
        format!("/api/{}", parts.join("/"))
    }

    /// Extracts types from Next.js Server Action
    ///
    /// Server Actions are functions, so we need to find the function definition
    /// and extract types from parameters (request) and return type (response).
    /// For now, we'll return None as this requires finding the function definition.
    fn extract_types_from_server_action(
        &self,
        _call: &Call,
    ) -> (
        Option<dc_core::models::TypeInfo>,
        Option<dc_core::models::TypeInfo>,
    ) {
        // TODO: Find function definition and extract types from parameters and return type
        // This requires searching the call graph for the function definition
        (None, None)
    }

    /// Detects NestJS route from function or class
    ///
    /// This is a placeholder for future NestJS support.
    /// For full support, we need to:
    /// 1. Extract decorators from TypeScript AST (@Get, @Post, @Controller, etc.)
    /// 2. Extract DTO classes with validation decorators
    /// 3. Extract types from @Body(), @Query(), @Param() decorators
    ///
    /// Currently, this function is not called but is prepared for future integration
    /// when TypeScript parser is extended to extract decorators.
    #[allow(dead_code)]
    fn detect_nestjs_route(
        &self,
        _func_or_class: &dc_core::parsers::FunctionOrClass,
    ) -> Option<ApiCallInfo> {
        // TODO: Implement when TypeScript parser supports decorator extraction
        // This requires:
        // 1. Extracting decorators from class and method definitions
        // 2. Parsing @Controller('path') and @Get('route') decorators
        // 3. Extracting DTO types from @Body(), @Query(), @Param()
        // 4. Building full path from controller and route decorators
        None
    }

    /// Extracts HTTP method from NestJS decorator name
    ///
    /// Examples: "Get" -> HttpMethod::Get, "Post" -> HttpMethod::Post
    #[allow(dead_code)]
    fn extract_http_method_from_nestjs_decorator(
        &self,
        decorator_name: &str,
    ) -> Option<HttpMethod> {
        match decorator_name {
            "Get" | "GET" => Some(HttpMethod::Get),
            "Post" | "POST" => Some(HttpMethod::Post),
            "Put" | "PUT" => Some(HttpMethod::Put),
            "Delete" | "DELETE" => Some(HttpMethod::Delete),
            "Patch" | "PATCH" => Some(HttpMethod::Patch),
            _ => None,
        }
    }

    /// Extracts path from NestJS decorators
    ///
    /// Combines @Controller('base') and @Get('route') into full path
    #[allow(dead_code)]
    fn extract_path_from_nestjs_decorators(
        &self,
        _controller_path: Option<&str>,
        _route_path: Option<&str>,
    ) -> String {
        // TODO: Combine controller and route paths
        // Example: @Controller('users') + @Get('profile') -> /users/profile
        "/".to_string()
    }

    /// Extracts types from NestJS handler function
    ///
    /// Extracts request type from @Body() parameter and response type from return type
    #[allow(dead_code)]
    fn extract_types_from_nestjs_handler(
        &self,
        _func: &dc_core::parsers::FunctionOrClass,
    ) -> (
        Option<dc_core::models::TypeInfo>,
        Option<dc_core::models::TypeInfo>,
    ) {
        // TODO: Extract types from function parameters and return type
        // This requires:
        // 1. Finding @Body() parameter for request type
        // 2. Using return type for response type
        (None, None)
    }

    #[allow(clippy::only_used_in_recursion)]
    fn find_ts_files(&self, dir: &PathBuf, files: &mut Vec<PathBuf>) -> Result<()> {
        if dir.is_file() {
            if let Some(ext) = dir.extension() {
                if ext == "ts" || ext == "tsx" {
                    files.push(dir.clone());
                }
            }
            return Ok(());
        }

        if dir.is_dir() {
            for entry in std::fs::read_dir(dir)? {
                let entry = entry?;
                let path = entry.path();
                self.find_ts_files(&path, files)?;
            }
        }

        Ok(())
    }
}

/// Information about an API call
struct ApiCallInfo {
    path: String,
    method: HttpMethod,
    location: Location,
    /// Optional request type (for future use with generic parameters)
    request_type: Option<dc_core::models::TypeInfo>,
    /// Optional response type (for future use with generic parameters)
    response_type: Option<dc_core::models::TypeInfo>,
}
