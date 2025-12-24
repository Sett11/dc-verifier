use crate::path_resolver;
use anyhow::{Context, Result};
use dc_core::call_graph::{CallEdge, CallGraph, CallNode, HttpMethod};
use dc_core::models::{Location, NodeId};
use dc_core::openapi::{OpenAPILinker, OpenAPIParser, OpenAPISchema};
use dc_core::parsers::{Call, TypeScriptParser};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use swc_ecma_ast;
use tracing::{debug, error, warn};

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
    /// Cache for SDK function analysis results (function_name -> ApiCallInfo)
    sdk_function_cache: HashMap<String, Option<ApiCallInfo>>,
    /// Map of imported functions to their source files (function_name -> source_file_path)
    imported_functions: HashMap<String, PathBuf>,
    /// TypeScript path resolver for handling path mappings
    path_resolver: path_resolver::TypeScriptPathResolver,
    /// OpenAPI schema for linking TypeScript types to Backend
    /// TODO: Integrate into detect_api_call to link API calls with OpenAPI endpoints
    openapi_schema: Option<OpenAPISchema>,
    /// OpenAPI linker for schema matching
    /// TODO: Use in detect_api_call to match discovered API calls with OpenAPI endpoints
    openapi_linker: Option<OpenAPILinker>,
    /// Zod extractor for finding schema usages
    zod_extractor: crate::zod::ZodExtractor,
}

impl TypeScriptCallGraphBuilder {
    /// Determines project root by finding common ancestor of src_paths and searching for marker files
    fn determine_project_root(src_paths: &[PathBuf]) -> PathBuf {
        if src_paths.is_empty() {
            return PathBuf::from(".");
        }

        // Find common ancestor of all src_paths
        let mut common_ancestor = src_paths[0].clone();
        for path in src_paths.iter().skip(1) {
            common_ancestor = Self::find_common_ancestor(&common_ancestor, path);
        }

        // Search upward for marker files (package.json or tsconfig.json)
        let mut current = common_ancestor.clone();
        loop {
            // Check for marker files
            if current.join("package.json").exists() || current.join("tsconfig.json").exists() {
                return current;
            }

            // Move up one directory
            if let Some(parent) = current.parent() {
                current = parent.to_path_buf();
            } else {
                // Reached root, return common ancestor
                break;
            }
        }

        // Fallback to common ancestor or first src_path's parent
        common_ancestor
            .parent()
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| PathBuf::from("."))
    }

    /// Finds common ancestor of two paths
    fn find_common_ancestor(path1: &Path, path2: &Path) -> PathBuf {
        let components1: Vec<_> = path1.components().collect();
        let components2: Vec<_> = path2.components().collect();

        let mut common = PathBuf::new();
        let min_len = components1.len().min(components2.len());

        for i in 0..min_len {
            if components1[i] == components2[i] {
                common.push(components1[i]);
            } else {
                break;
            }
        }

        common
    }

    /// Creates a new builder
    pub fn new(src_paths: Vec<PathBuf>) -> Self {
        // Determine project root using helper function
        let project_root = Self::determine_project_root(&src_paths);

        Self {
            graph: CallGraph::new(),
            src_paths,
            parser: TypeScriptParser::new(),
            processed_files: HashSet::new(),
            module_nodes: HashMap::new(),
            function_nodes: HashMap::new(),
            project_root: Some(project_root.clone()),
            max_depth: None,
            current_depth: 0,
            verbose: false,
            sdk_function_cache: HashMap::new(),
            imported_functions: HashMap::new(),
            path_resolver: path_resolver::TypeScriptPathResolver::new(&project_root),
            openapi_schema: None,
            openapi_linker: None,
            zod_extractor: crate::zod::ZodExtractor::new(),
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

    /// Sets the OpenAPI schema path
    /// If provided, the builder will use OpenAPI schema to link TypeScript API calls with Backend routes
    pub fn with_openapi_schema(mut self, openapi_path: Option<PathBuf>) -> Self {
        if let Some(path) = openapi_path {
            if let Ok(schema) = OpenAPIParser::parse_file(&path) {
                if self.verbose {
                    debug!(
                        openapi_path = ?path,
                        "Loaded OpenAPI schema"
                    );
                }
                self.openapi_schema = Some(schema.clone());
                self.openapi_linker = Some(OpenAPILinker::new(schema));
            } else if self.verbose {
                warn!(
                    openapi_path = ?path,
                    "Failed to parse OpenAPI schema"
                );
            }
        }
        self
    }

    /// Builds graph for TypeScript project
    pub fn build_graph(mut self) -> Result<CallGraph> {
        // 1. Find all .ts/.tsx files in src_paths
        let mut files = Vec::new();
        for src_path in &self.src_paths {
            self.find_ts_files(src_path, &mut files)?;
        }

        // 2. Determine project root using helper function (only if not already set or if it differs)
        let discovered_root = Self::determine_project_root(&self.src_paths);
        if self.project_root.as_ref().is_none()
            || self.project_root.as_ref() != Some(&discovered_root)
        {
            self.project_root = Some(discovered_root.clone());
            // Reinitialize path resolver with correct project root
            self.path_resolver = path_resolver::TypeScriptPathResolver::new(&discovered_root);
        }

        // 3. Parse and process each file
        for file in files {
            if let Err(err) = self.process_file(&file) {
                error!(
                    file_path = ?file,
                    error = %err,
                    "Error processing file"
                );
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
            for import in &imports {
                if let Err(err) = self.process_import(module_node, import, &normalized) {
                    warn!(
                        import_path = %import.path,
                        file_path = ?normalized,
                        error = %err,
                        "Error processing import"
                    );
                }
            }

            // Track imported functions for SDK detection
            // Handle both direct imports and export * from patterns
            for import in &imports {
                if let Ok(import_path) = self.resolve_import_path(&import.path, &normalized) {
                    if self.is_sdk_file(&import_path) {
                        // If import has specific names, track them
                        if !import.names.is_empty() {
                            for name in &import.names {
                                self.imported_functions
                                    .insert(name.clone(), import_path.clone());
                            }
                        }
                    }
                    // Also check if the imported file re-exports from SDK files
                    self.track_reexported_sdk_functions(&import_path, &normalized, 0);
                }
            }

            // Also check for export * from patterns in the current file
            // and track exported functions from SDK files
            // Continue even if import resolution fails
            self.track_export_all_sdk_functions(&module, &normalized, 0);

            // Extract calls
            let calls = self
                .parser
                .extract_calls(&module, &file_path_str, &converter, &source);
            for call in &calls {
                if let Err(err) = self.process_call(module_node, call, &normalized) {
                    warn!(
                        call_name = %call.name,
                        file_path = ?normalized,
                        error = %err,
                        "Error processing call"
                    );
                }
            }

            // Detect API calls and create Route nodes
            for call in calls {
                if let Some(api_call) = self.detect_api_call(&call) {
                    if let Err(err) =
                        self.create_route_from_api_call(api_call, &normalized, &file_path_str)
                    {
                        debug!(
                            call_name = %call.name,
                            error = %err,
                            "Failed to create route from API call"
                        );
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

            // Extract Zod schemas and add them to graph
            let zod_schemas = self
                .parser
                .extract_zod_schemas(&module, &file_path_str, &converter);
            for zod_schema in &zod_schemas {
                let schema_node_id = NodeId::from(self.graph.add_node(CallNode::Schema {
                    schema: zod_schema.clone(),
                }));

                // Add edge from module to schema
                self.graph.add_edge(
                    *module_node,
                    schema_node_id.0,
                    CallEdge::Call {
                        caller: module_node,
                        callee: schema_node_id,
                        argument_mapping: Vec::new(),
                        location: zod_schema.location.clone(),
                    },
                );
            }

            // Link Zod schemas to their usage and API calls
            if !zod_schemas.is_empty() {
                if let Err(e) = self.link_zod_schemas_to_usage(&normalized, &zod_schemas) {
                    if self.verbose {
                        debug!(
                            error = %e,
                            "Failed to link Zod schemas to usage"
                        );
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

    /// Checks if a file is an SDK file (openapi-client, api-client, sdk, etc.)
    fn is_sdk_file(&self, file_path: &Path) -> bool {
        let _path_str = file_path.to_string_lossy().to_lowercase();
        let file_name = file_path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("")
            .to_lowercase();
        let file_stem = file_path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_lowercase();

        // Explicit check for sdk.gen.ts (most common pattern)
        if file_name == "sdk.gen.ts" || file_name == "sdk.gen.tsx" {
            return true;
        }

        // Check for explicit SDK patterns in path (segment-aware)
        // Check if any path segment is exactly "openapi-client" or "api-client"
        let has_openapi_client = file_path.components().any(|c| {
            let segment = c.as_os_str().to_string_lossy().to_lowercase();
            segment == "openapi-client" || segment == "api-client"
        });
        if has_openapi_client {
            return true;
        }

        // Check for explicit file name patterns
        if file_name == "openapi-client" || file_name == "api-client" {
            return true;
        }

        // Check for .gen.ts/.gen.tsx files (generated files)
        if file_name.ends_with(".gen.ts") || file_name.ends_with(".gen.tsx") {
            return true;
        }

        // Check for whole path segments (not substrings)
        let has_sdk_segment = file_path
            .components()
            .any(|c| c.as_os_str().to_string_lossy().to_lowercase() == "sdk");
        if has_sdk_segment {
            return true;
        }

        // Check for file stem exactly matching "sdk"
        if file_stem == "sdk" {
            return true;
        }

        // Check for explicit file name suffixes/prefixes
        if file_name.ends_with(".sdk.ts")
            || file_name.ends_with(".sdk.tsx")
            || file_name.ends_with("-client.ts")
            || file_name.ends_with("-client.tsx")
            || file_name.ends_with("_client.ts")
            || file_name.ends_with("_client.tsx")
        {
            return true;
        }

        false
    }

    /// Checks if a function call is from an SDK file
    fn is_sdk_function_call(&self, function_name: &str) -> bool {
        self.imported_functions.contains_key(function_name)
    }

    /// Analyzes SDK function body to extract API call information
    fn analyze_sdk_function(
        &mut self,
        function_name: &str,
        source_file: &Path,
    ) -> Option<ApiCallInfo> {
        // Check cache first
        if let Some(cached) = self.sdk_function_cache.get(function_name) {
            return cached.clone();
        }

        // Parse the source file
        let (module, source, converter) = match self.parser.parse_file(source_file) {
            Ok(result) => result,
            Err(_) => {
                debug!(
                    source_file = ?source_file,
                    "Failed to parse SDK file"
                );
                self.sdk_function_cache
                    .insert(function_name.to_string(), None);
                return None;
            }
        };

        // Find the function definition
        let file_path_str = source_file.to_string_lossy().to_string();
        let function_info = match self.parser.find_function_by_name(
            &module,
            function_name,
            &file_path_str,
            &converter,
        ) {
            Some(info) => info,
            None => {
                if self.verbose {
                    debug!(
                        function_name = %function_name,
                        source_file = ?source_file,
                        "Function not found"
                    );
                }
                self.sdk_function_cache
                    .insert(function_name.to_string(), None);
                return None;
            }
        };

        // If we have an OpenAPI linker, try to resolve the API call purely by operationId
        // (function name) before doing any heuristic source scanning. This helps when
        // the URL or HTTP method are not easily recoverable from generated SDK code.
        if let Some(linker) = &self.openapi_linker {
            if let Some(endpoint) = linker.find_endpoint_by_operation_id(function_name) {
                if let Some(method) = Self::http_method_from_str(&endpoint.method) {
                    let api_info = Some(ApiCallInfo {
                        path: endpoint.path.clone(),
                        method,
                        location: function_info.location.clone(),
                        request_type: None,
                        response_type: None,
                    });

                    self.sdk_function_cache
                        .insert(function_name.to_string(), api_info.clone());
                    return api_info;
                }
            }
        }

        // Analyze function body to find client.get/post/delete calls
        // We need to parse the source code to find the pattern:
        // return (options?.client ?? client).get/post/delete({ url: "...", ... })
        let api_info = self.extract_api_info_from_sdk_function(&source, &function_info.location);

        // Cache the result
        self.sdk_function_cache
            .insert(function_name.to_string(), api_info.clone());

        api_info
    }

    /// Converts lowercase HTTP method string to HttpMethod
    fn http_method_from_str(method: &str) -> Option<HttpMethod> {
        match method.to_lowercase().as_str() {
            "get" => Some(HttpMethod::Get),
            "post" => Some(HttpMethod::Post),
            "put" => Some(HttpMethod::Put),
            "patch" => Some(HttpMethod::Patch),
            "delete" => Some(HttpMethod::Delete),
            "options" => Some(HttpMethod::Options),
            "head" => Some(HttpMethod::Head),
            _ => None,
        }
    }

    /// Extracts API call information from SDK function source code
    fn extract_api_info_from_sdk_function(
        &self,
        source: &str,
        function_location: &Location,
    ) -> Option<ApiCallInfo> {
        // Find the function body in source code
        let start_line = function_location.line;
        let lines: Vec<&str> = source.lines().collect();

        // Look for patterns like:
        // - client.get({ url: "/items/" })
        // - (options?.client ?? client).post({ url: "/items/" })
        // - client.delete({ url: "/items/{id}" })

        // Search for client method calls in the function body
        let mut found_method: Option<HttpMethod> = None;
        let mut found_url: Option<String> = None;

        // Look for lines containing client.get, client.post, etc.
        for (idx, line) in lines.iter().enumerate() {
            if idx < start_line.saturating_sub(1) {
                continue;
            }

            let line_lower = line.to_lowercase();

            // Check for HTTP method patterns
            if line_lower.contains("client.get")
                || line_lower.contains("?.get")
                || line_lower.contains("?? client).get")
            {
                found_method = Some(HttpMethod::Get);
            } else if line_lower.contains("client.post")
                || line_lower.contains("?.post")
                || line_lower.contains("?? client).post")
            {
                found_method = Some(HttpMethod::Post);
            } else if line_lower.contains("client.put")
                || line_lower.contains("?.put")
                || line_lower.contains("?? client).put")
            {
                found_method = Some(HttpMethod::Put);
            } else if line_lower.contains("client.delete")
                || line_lower.contains("?.delete")
                || line_lower.contains("?? client).delete")
            {
                found_method = Some(HttpMethod::Delete);
            } else if line_lower.contains("client.patch")
                || line_lower.contains("?.patch")
                || line_lower.contains("?? client).patch")
            {
                found_method = Some(HttpMethod::Patch);
            }

            // Extract URL from url: "..."
            if let Some(url_start) = line_lower.find("url:") {
                let after_url = &line[url_start + 4..];
                // Try to extract string value
                if let Some(quote_start) = after_url.find('"') {
                    let url_part = &after_url[quote_start + 1..];
                    if let Some(quote_end) = url_part.find('"') {
                        found_url = Some(url_part[..quote_end].to_string());
                    }
                } else if let Some(quote_start) = after_url.find('\'') {
                    let url_part = &after_url[quote_start + 1..];
                    if let Some(quote_end) = url_part.find('\'') {
                        found_url = Some(url_part[..quote_end].to_string());
                    }
                }
            }

            // If we found both, we can return
            if found_method.is_some() && found_url.is_some() {
                break;
            }
        }

        if let (Some(method), Some(url)) = (found_method, found_url) {
            Some(ApiCallInfo {
                path: url,
                method,
                location: function_location.clone(),
                request_type: None,
                response_type: None,
            })
        } else {
            None
        }
    }

    /// Extracts URL from call arguments
    /// Tries to extract from object with "url" property, falls back to first argument as string
    fn extract_url_from_call_arguments(&self, call: &Call) -> String {
        if let Some(first_arg) = call.arguments.first() {
            // Try to parse as object with "url" property
            // Pattern: { url: "/items/" } or { url: "/items/", ... }
            let arg_value = &first_arg.value;
            if let Some(url_start) = arg_value.find("url:") {
                let after_url = &arg_value[url_start + 4..];
                // Try to extract string value
                if let Some(quote_start) = after_url.find('"') {
                    let url_part = &after_url[quote_start + 1..];
                    if let Some(quote_end) = url_part.find('"') {
                        return url_part[..quote_end].to_string();
                    }
                } else if let Some(quote_start) = after_url.find('\'') {
                    let url_part = &after_url[quote_start + 1..];
                    if let Some(quote_end) = url_part.find('\'') {
                        return url_part[..quote_end].to_string();
                    }
                }
            }
            // Fallback: use first argument as-is (might be a string literal)
            first_arg.value.clone()
        } else {
            "/".to_string()
        }
    }

    /// Detects if a call is an API call (fetch, axios, React Query, etc.)
    fn detect_api_call(&mut self, call: &Call) -> Option<ApiCallInfo> {
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
        // Also supports optional chaining: (options?.client ?? client).get()
        let api_patterns = ["api.", "client.", "http.", "request."];
        for pattern in &api_patterns {
            if name.starts_with(pattern) {
                let parts: Vec<&str> = name.split('.').collect();
                if parts.len() >= 2 {
                    if let Ok(method) = parts[1].parse::<HttpMethod>() {
                        // Try to extract URL from first argument (could be a string or object)
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

        // Check for client API calls using AST information (preferred) or string fallback
        // AST approach: check if base_object is exactly "client" and property is an HTTP method
        let is_client_call = if let Some(ref base) = call.base_object {
            base == "client"
                && call
                    .property
                    .as_ref()
                    .and_then(|p| p.parse::<HttpMethod>().ok())
                    .is_some()
        } else {
            false
        };

        if is_client_call {
            // Use AST information - base is "client", property is HTTP method
            if let Some(property) = &call.property {
                if let Ok(method) = property.parse::<HttpMethod>() {
                    // Try to extract URL from arguments
                    let path = self.extract_url_from_call_arguments(call);
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

        // Fallback: string-based detection for cases where AST info is not available
        // Use stricter patterns to avoid false positives
        if name.starts_with("client.") {
            let parts: Vec<&str> = name.split('.').collect();
            if parts.len() == 2 {
                if let Ok(method) = parts[1].parse::<HttpMethod>() {
                    let path = self.extract_url_from_call_arguments(call);
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

        // Check for SDK function calls (OpenAPI-generated clients)
        // Need to clone source_file to avoid borrow checker issues
        if self.is_sdk_function_call(name) {
            let source_file = self.imported_functions.get(name).cloned();
            if let Some(source_file) = source_file {
                if let Some(api_info) = self.analyze_sdk_function(name, &source_file) {
                    return Some(api_info);
                }
            }
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

        // Create initial Route node for this API call
        let route_node = NodeId::from(self.graph.add_node(CallNode::Route {
            path: api_call.path.clone(),
            method: api_call.method,
            handler: handler_node,
            location: location.clone(),
            request_schema: None,
            response_schema: None,
        }));

        // If we have an OpenAPI linker, try to match this route to an OpenAPI endpoint
        // and enrich Route node with request/response schemas from OpenAPI.
        if let Some(linker) = &self.openapi_linker {
            if let Some(endpoint) = linker.match_route_to_endpoint(&api_call.path, api_call.method)
            {
                // Update existing Route node in the graph with OpenAPI schema references
                if let Some(CallNode::Route {
                    request_schema,
                    response_schema,
                    ..
                }) = self.graph.node_weight_mut(route_node.0)
                {
                    // Request schema from OpenAPI (if any)
                    if let Some(ref schema_name) = endpoint.request_schema {
                        *request_schema = Some(dc_core::models::SchemaReference {
                            name: schema_name.clone(),
                            schema_type: dc_core::models::SchemaType::OpenAPI,
                            location: Location {
                                file: format!("openapi://{}", schema_name),
                                line: 0,
                                column: None,
                            },
                            metadata: std::collections::HashMap::new(),
                        });
                    }

                    // Response schema from OpenAPI (if any)
                    if let Some(ref schema_name) = endpoint.response_schema {
                        *response_schema = Some(dc_core::models::SchemaReference {
                            name: schema_name.clone(),
                            schema_type: dc_core::models::SchemaType::OpenAPI,
                            location: Location {
                                file: format!("openapi://{}", schema_name),
                                line: 0,
                                column: None,
                            },
                            metadata: std::collections::HashMap::new(),
                        });
                    }
                }
            }
        }

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

        // Try adding .gen.ts/.gen.tsx extensions for generated files
        // This handles imports like "./client" -> "./client.gen.ts"
        for gen_ext in &["gen.ts", "gen.tsx"] {
            let mut with_gen_ext = candidate.clone();
            // Get the file stem and add .gen extension
            if let Some(stem) = candidate.file_stem().and_then(|s| s.to_str()) {
                with_gen_ext.set_file_name(format!("{}.{}", stem, gen_ext));
                if with_gen_ext.exists() {
                    debug!(
                        import_path = %import_path,
                        resolved_path = ?with_gen_ext,
                        "Resolved import to generated file"
                    );
                    return Ok(with_gen_ext);
                }
            }
        }

        // Try index files in directory
        if candidate.is_dir() || candidate.extension().is_none() {
            for index_file in &["index.ts", "index.tsx", "index.js", "index.jsx"] {
                let index_path = candidate.join(index_file);
                if index_path.exists() {
                    return Ok(index_path);
                }
            }
            // Also try index.gen.ts/index.gen.tsx
            for gen_index in &["index.gen.ts", "index.gen.tsx"] {
                let gen_index_path = candidate.join(gen_index);
                if gen_index_path.exists() {
                    debug!(
                        import_path = %import_path,
                        resolved_path = ?gen_index_path,
                        "Resolved import to generated index file"
                    );
                    return Ok(gen_index_path);
                }
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

    /// Maximum depth for re-export traversal to prevent infinite recursion
    const MAX_REEXPORT_DEPTH: usize = 10;

    /// Tracks re-exported SDK functions from a file (handles export * from patterns)
    fn track_reexported_sdk_functions(
        &mut self,
        file_path: &Path,
        current_file: &Path,
        depth: usize,
    ) {
        // Check recursion depth limit
        if depth >= Self::MAX_REEXPORT_DEPTH {
            debug!(
                file_path = ?file_path,
                "Max re-export depth reached"
            );
            return;
        }

        // Check if file was already processed to avoid infinite recursion
        let normalized = Self::normalize_path(file_path);
        if self.processed_files.contains(&normalized) {
            return;
        }

        // Mark file as being processed to prevent cycles
        self.processed_files.insert(normalized.clone());

        // Try to parse the file and find export * from patterns
        if let Ok((module, _, _)) = self.parser.parse_file(&normalized) {
            self.track_export_all_sdk_functions(&module, current_file, depth + 1);
        }
    }

    /// Tracks exported functions from SDK files via export * from
    fn track_export_all_sdk_functions(
        &mut self,
        module: &swc_ecma_ast::Module,
        current_file: &Path,
        depth: usize,
    ) {
        // Check recursion depth limit
        if depth >= Self::MAX_REEXPORT_DEPTH {
            if self.verbose {
                debug!("Max re-export depth reached in track_export_all_sdk_functions");
            }
            return;
        }

        for item in &module.body {
            if let swc_ecma_ast::ModuleItem::ModuleDecl(swc_ecma_ast::ModuleDecl::ExportAll(
                export_all,
            )) = item
            {
                let export_path = export_all
                    .src
                    .value
                    .as_str()
                    .unwrap_or_else(|| {
                        warn!(
                            file_path = ?current_file,
                            span = ?export_all.src.span,
                            "Failed to extract export path, using empty string"
                        );
                        ""
                    })
                    .to_string();
                if let Ok(export_file_path) = self.resolve_import_path(&export_path, current_file) {
                    if self.is_sdk_file(&export_file_path) {
                        // Find all exported functions from this SDK file
                        if let Ok((target_module, _, _)) = self.parser.parse_file(&export_file_path)
                        {
                            let file_path_str = export_file_path.to_string_lossy().to_string();
                            let converter = dc_core::parsers::LocationConverter::new(String::new());
                            let functions = self.parser.extract_functions_and_classes(
                                &target_module,
                                &file_path_str,
                                &converter,
                            );
                            for func in functions {
                                if let dc_core::parsers::FunctionOrClass::Function {
                                    name, ..
                                } = func
                                {
                                    let name_clone = name.clone();
                                    self.imported_functions
                                        .insert(name, export_file_path.clone());
                                    debug!(
                                        function_name = %name_clone,
                                        export_file_path = ?export_file_path,
                                        "Tracked SDK function"
                                    );
                                }
                            }
                        }
                    } else {
                        // Not an SDK file, but might re-export from SDK files
                        self.track_reexported_sdk_functions(
                            &export_file_path,
                            current_file,
                            depth + 1,
                        );
                    }
                }
            }
        }
    }

    /// Finds API call that follows Zod schema usage
    /// Looks for API calls in the same function or nearby statements
    fn find_api_call_after_zod_usage(
        &mut self,
        zod_usage: &dc_core::models::ZodUsage,
        calls: &[Call],
    ) -> Option<ApiCallInfo> {
        // Find calls that are in the same file and after the Zod usage
        let mut candidate_calls = Vec::new();

        for call in calls {
            // Check if call is in the same file
            if call.location.file == zod_usage.location.file {
                // Check if call is after Zod usage (same or later line)
                if call.location.line >= zod_usage.location.line {
                    // Check if this is an API call by examining the call name and structure
                    // We'll do a simple check here instead of calling detect_api_call
                    // to avoid mutable borrow issues
                    let is_api_call = call.name == "fetch"
                        || call.name.starts_with("axios.")
                        || call.name.starts_with("api.")
                        || call.name.starts_with("client.")
                        || call.name.starts_with("http.")
                        || call.name.starts_with("request.")
                        || call.name.contains(".useQuery")
                        || call.name.contains(".useMutation")
                        || call.name.starts_with("actions.");

                    if is_api_call {
                        // Try to extract API call info
                        if let Some(api_call) = self.detect_api_call(call) {
                            candidate_calls.push((call.location.line, api_call));
                        }
                    }
                }
            }
        }

        // Return the closest API call (smallest line difference)
        candidate_calls
            .into_iter()
            .min_by_key(|(line, _)| line - zod_usage.location.line)
            .map(|(_, api_call)| api_call)
    }

    /// Finds schema node by name and type
    fn find_schema_node(
        &self,
        schema_name: &str,
        schema_type: dc_core::models::SchemaType,
    ) -> Option<NodeId> {
        self.graph
            .node_indices()
            .find(|&node_id| {
                if let Some(CallNode::Schema { schema: schema_ref }) =
                    self.graph.node_weight(node_id)
                {
                    return schema_ref.name == schema_name && schema_ref.schema_type == schema_type;
                }
                false
            })
            .map(NodeId::from)
    }

    /// Links Zod schemas to their usage and API calls
    fn link_zod_schemas_to_usage(
        &mut self,
        file_path: &Path,
        zod_schemas: &[dc_core::models::SchemaReference],
    ) -> Result<()> {
        let file_path_str = file_path.to_string_lossy().to_string();

        // Parse the file to get module
        let (module, source, converter) = self.parser.parse_file(file_path)?;

        // Extract all calls from the file
        let calls = self
            .parser
            .extract_calls(&module, &file_path_str, &converter, &source);

        // For each Zod schema, find its usage
        for zod_schema in zod_schemas {
            let usages = self.zod_extractor.find_zod_schema_usage(
                &zod_schema.name,
                &module,
                &file_path_str,
                &converter,
            );

            for mut usage in usages {
                // Try to find associated API call
                if let Some(api_call) = self.find_api_call_after_zod_usage(&usage, &calls) {
                    usage.api_call_location = Some(api_call.location.clone());

                    // Store usage in metadata
                    if let Some(zod_node_id) =
                        self.find_schema_node(&zod_schema.name, dc_core::models::SchemaType::Zod)
                    {
                        // Get the schema node and update it
                        if let Some(CallNode::Schema {
                            schema: mut schema_ref,
                        }) = self.graph.node_weight(zod_node_id.0).cloned()
                        {
                            // Store usages as JSON in metadata
                            let existing_usages: Vec<dc_core::models::ZodUsage> = schema_ref
                                .metadata
                                .get("usages")
                                .and_then(|s| serde_json::from_str(s).ok())
                                .unwrap_or_default();

                            let mut updated_usages = existing_usages;
                            updated_usages.push(usage);

                            if let Ok(usages_json) = serde_json::to_string(&updated_usages) {
                                schema_ref
                                    .metadata
                                    .insert("usages".to_string(), usages_json);

                                // Replace the node with updated schema
                                *self.graph.node_weight_mut(zod_node_id.0).ok_or_else(|| {
                                    anyhow::anyhow!("Zod schema node not found")
                                })? = CallNode::Schema { schema: schema_ref };
                            }
                        }
                    }
                }
            }
        }

        Ok(())
    }
}

/// Information about an API call
#[derive(Clone)]
struct ApiCallInfo {
    path: String,
    method: HttpMethod,
    location: Location,
    /// Optional request type (for future use with generic parameters)
    request_type: Option<dc_core::models::TypeInfo>,
    /// Optional response type (for future use with generic parameters)
    response_type: Option<dc_core::models::TypeInfo>,
}
