use anyhow::{Context, Result};
use rustpython_parser::ast::Ranged;
use rustpython_parser::{ast, parse, Mode};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use tracing::{debug, warn};

use crate::call_graph::decorator::Decorator;
use crate::call_graph::extractor::PydanticSchemaExtractor;
use crate::call_graph::{CallEdge, CallGraph, CallNode, HttpMethod, Parameter};
use crate::models::{BaseType, Location, NodeId, SchemaReference, SchemaType, TypeInfo};
use crate::parsers::{Call, Import, LocationConverter, PythonParser};

/// Call graph builder - main class for creating call graphs from code
pub struct CallGraphBuilder {
    /// Call graph
    graph: CallGraph,
    /// Entry points in the application
    entry_points: Vec<PathBuf>,
    /// Processed files (to avoid cycles)
    processed_files: HashSet<PathBuf>,
    /// Source code parser
    parser: PythonParser,
    /// Cache of module nodes
    module_nodes: HashMap<PathBuf, NodeId>,
    /// Cache of functions/methods (key: file + name)
    function_nodes: HashMap<String, NodeId>,
    /// Cache of Pydantic models (class name -> SchemaReference)
    pydantic_models: HashMap<String, SchemaReference>,
    /// Cache of ORM models (class name -> SchemaReference)
    orm_models: HashMap<String, SchemaReference>,
    /// Optional Pydantic schema extractor for JSON schema extraction
    schema_extractor: Option<Box<dyn PydanticSchemaExtractor>>,
    /// Project root
    project_root: Option<PathBuf>,
    /// Maximum recursion depth (None = unlimited)
    max_depth: Option<usize>,
    /// Current recursion depth
    current_depth: usize,
    /// Enable verbose debug output
    verbose: bool,
    /// Strict import resolution: fail on unresolved imports when true
    strict_imports: bool,
    /// Import information: file path -> (imported name -> module path)
    /// Stores which names are imported from which modules in each file
    file_imports: HashMap<PathBuf, HashMap<String, String>>,
}

impl CallGraphBuilder {
    /// Creates a new call graph builder
    ///
    /// # Example
    /// ```
    /// use dc_core::call_graph::CallGraphBuilder;
    /// let builder = CallGraphBuilder::new();
    /// ```
    pub fn new() -> Self {
        Self::with_parser(PythonParser::new(), false)
    }

    /// Creates a call graph builder with a custom parser (for tests)
    pub fn with_parser(parser: PythonParser, strict_imports: bool) -> Self {
        Self {
            graph: CallGraph::new(),
            entry_points: Vec::new(),
            processed_files: HashSet::new(),
            parser,
            module_nodes: HashMap::new(),
            function_nodes: HashMap::new(),
            pydantic_models: HashMap::new(),
            orm_models: HashMap::new(),
            schema_extractor: None,
            project_root: None,
            max_depth: None,
            current_depth: 0,
            verbose: false,
            strict_imports,
            file_imports: HashMap::new(),
        }
    }

    /// Sets the schema extractor for JSON schema extraction
    pub fn with_schema_extractor(mut self, extractor: Box<dyn PydanticSchemaExtractor>) -> Self {
        self.schema_extractor = Some(extractor);
        self
    }

    /// Sets the verbose flag for debug output
    pub fn with_verbose(mut self, verbose: bool) -> Self {
        self.verbose = verbose;
        self
    }

    /// Sets strict import resolution mode
    pub fn with_strict_imports(mut self, strict_imports: bool) -> Self {
        self.strict_imports = strict_imports;
        self
    }

    /// Sets the maximum recursion depth
    pub fn with_max_depth(mut self, max_depth: Option<usize>) -> Self {
        self.max_depth = max_depth;
        self
    }

    /// Finds the entry point (main.py, app.py) in the project
    pub fn find_entry_point(&self, project_root: &Path) -> Result<PathBuf> {
        let candidates = ["main.py", "app.py", "__main__.py"];

        for candidate in &candidates {
            let path = project_root.join(candidate);
            if path.exists() {
                return Ok(path);
            }
        }

        anyhow::bail!("Entry point not found in {:?}", project_root)
    }

    /// Builds the graph from an entry point
    pub fn build_from_entry(&mut self, entry: &Path) -> Result<()> {
        let normalized_entry = Self::normalize_path(entry);

        if self.processed_files.contains(&normalized_entry) {
            return Ok(()); // Already processed
        }

        // Check recursion depth limit
        if let Some(max_depth) = self.max_depth {
            if self.current_depth >= max_depth {
                return Err(anyhow::Error::from(
                    crate::error::GraphError::MaxDepthExceeded(max_depth),
                ));
            }
        }

        self.current_depth += 1;

        if self.project_root.is_none() {
            if let Some(parent) = normalized_entry.parent() {
                self.project_root = Some(parent.to_path_buf());
            }
        }

        let source = fs::read_to_string(&normalized_entry)
            .with_context(|| format!("Failed to read {:?}", normalized_entry))?;
        let ast = parse(
            &source,
            Mode::Module,
            normalized_entry.to_string_lossy().as_ref(),
        )
        .with_context(|| format!("Failed to parse {:?}", normalized_entry))?;

        // Create LocationConverter for accurate byte offset conversion
        let converter = LocationConverter::new(source);

        let module_node = self.get_or_create_module_node(&normalized_entry)?;

        self.processed_files.insert(normalized_entry.clone());
        self.entry_points.push(normalized_entry.clone());

        self.process_imports(&ast, module_node, &normalized_entry, &converter)?;
        self.extract_functions_and_classes(&ast, &normalized_entry, &converter)?;
        self.process_calls(&ast, module_node, &normalized_entry, &converter)?;
        self.process_decorators(&ast, &normalized_entry, &converter)?;

        self.current_depth -= 1;
        Ok(())
    }

    /// Resolves import according to strict_imports configuration.
    /// In non-strict mode, unresolved imports are logged (if verbose) and treated as no-op (Ok(None)).
    /// In strict mode, unresolved imports result in an error.
    fn resolve_import_with_config(
        &mut self,
        import: &Import,
        current_file: &Path,
    ) -> Result<Option<PathBuf>> {
        use crate::models::ImportError;

        let project_root = self
            .project_root
            .as_deref()
            .unwrap_or_else(|| Path::new("."));

        match self
            .parser
            .resolve_import_cached(&import.path, project_root)
        {
            Ok(Some(path)) => {
                debug!(
                    import_path = %import.path,
                    resolved_path = ?path,
                    "Resolved import"
                );
                Ok(Some(path))
            }
            Ok(None) => {
                let msg = format!(
                    "Import '{}' from {:?} could not be resolved (treated as local/missing)",
                    import.path, current_file
                );
                if self.strict_imports {
                    anyhow::bail!("[STRICT IMPORTS] {}", msg);
                } else {
                    debug!(
                        import_path = %import.path,
                        current_file = ?current_file,
                        "Import could not be resolved, treated as local/missing"
                    );
                    Ok(None)
                }
            }
            Err(ImportError::ExternalDependency { module, suggestion }) => {
                let msg = format!(
                    "External dependency not resolved: {} ({}).",
                    module, suggestion
                );
                if self.strict_imports {
                    anyhow::bail!("[STRICT IMPORTS] {}", msg);
                } else {
                    warn!(
                        import_path = %import.path,
                        current_file = ?current_file,
                        module = %module,
                        suggestion = %suggestion,
                        "External dependency not resolved, skipping import"
                    );
                    Ok(None)
                }
            }
            Err(ImportError::ResolutionFailed { import, reason }) => {
                let msg = format!(
                    "Failed to resolve import '{}' from {:?}: {}",
                    import, current_file, reason
                );
                if self.strict_imports {
                    anyhow::bail!("[STRICT IMPORTS] {}", msg);
                } else {
                    warn!(
                        import = %import,
                        current_file = ?current_file,
                        reason = %reason,
                        "Failed to resolve import, continuing"
                    );
                    Ok(None)
                }
            }
        }
    }

    /// Processes an import: adds a node and an edge
    pub fn process_import(
        &mut self,
        from: NodeId,
        import: &Import,
        current_file: &Path,
    ) -> Result<NodeId> {
        let import_path = match self.resolve_import_with_config(import, current_file)? {
            Some(path) => path,
            None => return Ok(from),
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

        // Recursively build graph for the imported module
        debug!(
            import_path = ?import_path,
            "Recursively building graph for imported module"
        );
        if let Err(err) = self.build_from_entry(&import_path) {
            warn!(
                import_path = ?import_path,
                error = %err,
                "Failed to recursively build graph for imported module"
            );
        }

        // Extract and cache Pydantic models from imported file
        if let Err(err) = self.extract_and_cache_pydantic_models(&import_path) {
            debug!(
                import_path = ?import_path,
                error = %err,
                "Failed to extract Pydantic models"
            );
        }

        // If import has specific names (e.g., "from api.routers import auth"),
        // also try to resolve and process those submodules
        if !import.names.is_empty() {
            let import_dir = if import_path.is_dir() {
                &import_path
            } else {
                import_path.parent().unwrap_or_else(|| {
                    warn!(
                        import_path = ?import_path,
                        "Import path has no parent, using current directory"
                    );
                    Path::new(".")
                })
            };

            for name in &import.names {
                // Try to find submodule: api/routers/auth.py
                let submodule_candidates = vec![
                    import_dir.join(format!("{}.py", name)),
                    import_dir.join(name).join("__init__.py"),
                ];

                for candidate in submodule_candidates {
                    if candidate.exists() {
                        debug!(
                            submodule_name = %name,
                            import_path = %import.path,
                            candidate_path = ?candidate,
                            "Found submodule from import"
                        );
                        let _ = self.build_from_entry(&candidate);
                        break; // Found it, no need to try other candidates
                    }
                }
            }
        }

        Ok(module_node)
    }

    /// Processes a function call: adds an edge
    pub fn process_call(
        &mut self,
        caller: NodeId,
        call: &Call,
        current_file: &Path,
    ) -> Result<NodeId> {
        // Check if this is a Pydantic transformation method
        if let Some(transform_info) = self.detect_pydantic_transformation(call) {
            return self.process_pydantic_transformation(
                caller,
                call,
                current_file,
                transform_info,
            );
        }

        let Some(callee_node) = self.find_function_node(&call.name, current_file) else {
            // Function not found, return caller without creating edge
            return Ok(caller);
        };

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

        if let Some(file) = self.node_file_path(callee_node) {
            let normalized = Self::normalize_path(&file);
            if !self.processed_files.contains(&normalized) {
                let _ = self.build_from_entry(&normalized);
            }
        }

        Ok(callee_node)
    }

    /// Processes a FastAPI decorator (@app.post)
    pub fn process_decorator(&mut self, decorator: &Decorator, current_file: &Path) -> Result<()> {
        if !self.is_route_decorator(&decorator.name) {
            debug!(
                decorator_name = %decorator.name,
                file_path = ?current_file,
                "Decorator is not a route decorator"
            );
            return Ok(());
        }

        let handler_name = match &decorator.target_function {
            Some(name) => name,
            None => {
                debug!(
                    decorator_name = %decorator.name,
                    file_path = ?current_file,
                    "Route decorator has no target function"
                );
                return Ok(());
            }
        };

        debug!(
            decorator_name = %decorator.name,
            handler_name = %handler_name,
            file_path = ?current_file,
            "Processing route decorator"
        );

        // Try to find handler node - handle qualified names (ClassName.method)
        let handler_node = if handler_name.contains('.') {
            // Try qualified name first, then simple name
            let parts: Vec<&str> = handler_name.split('.').collect();
            let simple_name = parts.last().copied().unwrap_or(handler_name);

            debug!(
                handler_name = %handler_name,
                simple_name = %simple_name,
                "Handler name contains '.', trying qualified then simple name"
            );

            // First try qualified name as-is
            if let Some(node) = self.find_function_node(handler_name, current_file) {
                debug!(
                    handler_name = %handler_name,
                    "Found handler using qualified name"
                );
                Some(node)
            } else {
                // Fall back to simple name
                debug!(
                    simple_name = %simple_name,
                    "Qualified name failed, trying simple name"
                );
                self.find_function_node(simple_name, current_file)
            }
        } else {
            self.find_function_node(handler_name, current_file)
        };

        let Some(handler_node) = handler_node else {
            debug!(
                handler_name = %handler_name,
                file_path = ?current_file,
                "Failed to find function node for handler"
            );
            return Ok(());
        };

        let http_method = self
            .extract_http_method(&decorator.name)
            .unwrap_or(HttpMethod::Get);
        let route_path = decorator
            .arguments
            .first()
            .cloned()
            .unwrap_or_else(|| "/".to_string());

        let mut location = decorator.location.clone();
        if location.file.is_empty() {
            location.file = current_file.to_string_lossy().to_string();
        }

        // Extract request body schema from handler function parameters
        let request_body_schema =
            self.graph
                .node_weight(handler_node.0)
                .and_then(|node| match node {
                    CallNode::Function { parameters, .. } | CallNode::Method { parameters, .. } => {
                        // Find first parameter that is a request body
                        parameters.iter().find_map(|param| {
                            // Check if this is a request body parameter
                            if !self.is_request_body_parameter(param) {
                                return None;
                            }

                            // Extract schema from parameter
                            // 1. Check if it's Annotated[T, Body()]
                            if let Some(ref schema_ref) = &param.type_info.schema_ref {
                                let type_name = &schema_ref.name;

                                // Extract from Annotated[T, Body()]
                                if type_name.starts_with("Annotated[") {
                                    let inner_type = self.extract_annotated_inner_type(type_name);
                                    if !inner_type.is_empty() {
                                        // Check if inner type is a Pydantic model
                                        let simple_name = inner_type
                                            .rsplit('.')
                                            .next()
                                            .unwrap_or(&inner_type)
                                            .trim()
                                            .to_string();
                                        if let Some(pydantic_model) =
                                            self.pydantic_models.get(&simple_name)
                                        {
                                            return Some(pydantic_model.clone());
                                        }
                                    }
                                }
                            }

                            // 2. Check if parameter has a Pydantic schema reference
                            if let Some(schema_ref) = &param.type_info.schema_ref {
                                if schema_ref.schema_type == SchemaType::Pydantic {
                                    return Some(schema_ref.clone());
                                }
                            }

                            // 3. Check if type name matches a Pydantic model in cache
                            if let Some(schema_ref) = &param.type_info.schema_ref {
                                let simple_name = schema_ref
                                    .name
                                    .rsplit('.')
                                    .next()
                                    .unwrap_or(&schema_ref.name)
                                    .trim()
                                    .to_string();
                                if let Some(pydantic_model) = self.pydantic_models.get(&simple_name)
                                {
                                    return Some(pydantic_model.clone());
                                }
                            }

                            // 4. Check parameter name (for parameters without annotations)
                            // In FastAPI, if parameter name matches a Pydantic model, it's treated as body
                            if param.type_info.schema_ref.is_none() {
                                // Try to find Pydantic model by parameter name
                                let param_name_capitalized = if !param.name.is_empty() {
                                    let mut chars = param.name.chars();
                                    if let Some(first) = chars.next() {
                                        format!("{}{}", first.to_uppercase(), chars.as_str())
                                    } else {
                                        param.name.clone()
                                    }
                                } else {
                                    param.name.clone()
                                };

                                if let Some(pydantic_model) =
                                    self.pydantic_models.get(&param_name_capitalized)
                                {
                                    return Some(pydantic_model.clone());
                                }
                            }

                            None
                        })
                    }
                    _ => None,
                });

        // Check for response_model in decorator keyword arguments
        let response_model_type = decorator
            .keyword_arguments
            .get("response_model")
            .map(|s| s.trim().to_string());

        // Try to resolve response_model from imports if not found in cache
        if let Some(ref response_model_str) = response_model_type {
            // Extract base model name (handle generic types like Page[ItemRead] -> ItemRead)
            let base_model_name = self
                .parser
                .extract_base_model_from_response_model(response_model_str);
            let response_model_name = base_model_name
                .rsplit('.')
                .next()
                .unwrap_or(&base_model_name)
                .trim()
                .to_string();

            // Check if already in cache
            if !self.pydantic_models.contains_key(&response_model_name) {
                // Try to resolve from imports
                if let Err(err) =
                    self.resolve_schema_from_imports(&response_model_name, current_file)
                {
                    if self.verbose {
                        debug!(
                            schema_name = %response_model_name,
                            file_path = ?current_file,
                            error = %err,
                            "Failed to resolve schema from imports"
                        );
                    }
                }
            }

            // Ensure schema is enriched with JSON schema if available
            if let Some(model) = self.pydantic_models.get_mut(&response_model_name) {
                if let Some(ref extractor) = self.schema_extractor {
                    // Check if schema already has JSON schema in metadata
                    if !model.metadata.contains_key("json_schema") {
                        if let Err(err) = extractor.enrich_schema(model) {
                            debug!(
                                schema_name = %response_model_name,
                                error = %err,
                                "Failed to enrich schema"
                            );
                        }
                    }
                }
            }

            // Apply response_model to handler node's return_type
            if let Some(handler_node) = self.graph.node_weight_mut(handler_node.0) {
                // Get the Pydantic model schema reference
                if let Some(pydantic_model) = self.pydantic_models.get(&response_model_name) {
                    let return_type = Some(TypeInfo {
                        base_type: BaseType::Object,
                        schema_ref: Some(pydantic_model.clone()),
                        constraints: Vec::new(),
                        optional: false,
                    });

                    // Update handler node's return_type
                    match handler_node {
                        CallNode::Function {
                            return_type: rt, ..
                        } => {
                            *rt = return_type;
                            debug!(
                                schema_name = %response_model_name,
                                "Applied response_model to handler function return_type"
                            );
                        }
                        CallNode::Method {
                            return_type: rt, ..
                        } => {
                            *rt = return_type;
                            debug!(
                                schema_name = %response_model_name,
                                "Applied response_model to handler method return_type"
                            );
                        }
                        _ => {
                            // Handler is not a function or method, skip
                        }
                    }
                }
            }
        }

        // Check if handler function has a return type
        let handler_returns_data =
            self.graph
                .node_weight(handler_node.0)
                .and_then(|node| match node {
                    CallNode::Function { return_type, .. } => return_type.as_ref(),
                    CallNode::Method { return_type, .. } => return_type.as_ref(),
                    _ => None,
                });

        // Check response_model and return type correspondence
        if let Some(return_type_info) = handler_returns_data {
            if let Some(ref response_model_str) = response_model_type {
                // Extract base model name (handle generic types like Page[ItemRead] -> ItemRead)
                let base_model_name = self
                    .parser
                    .extract_base_model_from_response_model(response_model_str);
                let response_model_name = base_model_name
                    .rsplit('.')
                    .next()
                    .unwrap_or(&base_model_name)
                    .trim()
                    .to_string();

                // Compare with return type name
                if let Some(schema_ref) = &return_type_info.schema_ref {
                    let return_type_name = schema_ref.name.trim();

                    // Check if names match (case-insensitive, ignoring qualified names)
                    let return_type_simple = return_type_name
                        .rsplit('.')
                        .next()
                        .unwrap_or(return_type_name);

                    if !return_type_simple.eq_ignore_ascii_case(&response_model_name) {
                        // Type mismatch: response_model doesn't match return type
                        // This will be checked by contract rules later
                        // For now, we can add metadata to indicate potential mismatch
                    }
                } else if return_type_info.base_type == crate::models::BaseType::Object {
                    // Return type is Object but response_model is specified
                    // This suggests the return type annotation might be missing or incorrect
                }
            } else {
                // No response_model specified
                // Check if return type is a missing schema (dict[...] or any)
                // This will be checked by MissingSchemaRule which looks for missing_schema flag
                // The schema_ref already has the missing_schema flag set in resolve_type_annotation
                // if it's a dict[str, Any] or any type
            }
        }

        // Store request body schema in route metadata if found
        // Note: Route nodes don't have metadata field, so we'll store it in handler node's metadata
        // via the schema reference in parameters, which is already done
        // For now, we just log it if verbose
        if let Some(ref req_schema) = request_body_schema {
            debug!(
                schema_name = %req_schema.name,
                http_method = ?http_method,
                route_path = %route_path,
                "Found request body schema for route"
            );
        }

        // Get response_model_schema for Route node
        let response_model_schema = if let Some(ref response_model_str) = response_model_type {
            // Extract base model name (handle generic types like Page[ItemRead] -> ItemRead)
            let base_model_name = self
                .parser
                .extract_base_model_from_response_model(response_model_str);
            let response_model_name = base_model_name
                .rsplit('.')
                .next()
                .unwrap_or(&base_model_name)
                .trim()
                .to_string();

            // Get schema from cache
            self.pydantic_models.get(&response_model_name).cloned()
        } else {
            // Try to get from handler's return_type
            handler_returns_data.and_then(|rt| rt.schema_ref.clone())
        };

        let route_node = NodeId::from(self.graph.add_node(CallNode::Route {
            path: route_path.clone(),
            method: http_method,
            handler: handler_node,
            location: location.clone(),
            request_schema: request_body_schema.clone(),
            response_schema: response_model_schema.clone(),
        }));

        self.graph.add_edge(
            route_node.0,
            handler_node.0,
            CallEdge::Call {
                caller: route_node,
                callee: handler_node,
                argument_mapping: Vec::new(),
                location,
            },
        );

        debug!(
            http_method = ?http_method,
            route_path = %route_path,
            handler_node_index = handler_node.0.index(),
            file_path = ?current_file,
            "Created route node"
        );

        Ok(())
    }

    /// Links Pydantic models with SQLAlchemy models based on from_attributes
    /// This should be called after the graph is built to ensure all classes are available
    pub fn link_pydantic_to_sqlalchemy(&mut self) {
        // Find all Pydantic models with from_attributes=True
        let pydantic_with_from_attrs: Vec<(String, SchemaReference)> = self
            .pydantic_models
            .iter()
            .filter(|(_, model)| {
                model
                    .metadata
                    .get("from_attributes")
                    .map(|v| v == "true" || v == "True")
                    .unwrap_or(false)
            })
            .map(|(name, model)| (name.clone(), model.clone()))
            .collect();

        if pydantic_with_from_attrs.is_empty() {
            return;
        }

        // Find all SQLAlchemy models from cache (instead of searching in graph)
        let sqlalchemy_models: Vec<(String, SchemaReference)> = self
            .orm_models
            .iter()
            .map(|(name, schema_ref)| (name.clone(), schema_ref.clone()))
            .collect();

        // Link Pydantic models to SQLAlchemy models
        for (pydantic_name, mut pydantic_model) in pydantic_with_from_attrs {
            // Extract Pydantic fields from metadata
            let pydantic_fields: Vec<crate::models::PydanticFieldInfo> =
                if let Some(fields_json) = pydantic_model.metadata.get("fields") {
                    serde_json::from_str(fields_json).unwrap_or_default()
                } else {
                    Vec::new()
                };

            // Try to find matching SQLAlchemy model
            // Strategy 1: Exact name match (e.g., ItemRead -> Item)
            // Strategy 2: Remove common suffixes (Read, Create, Update, etc.)
            // Strategy 3: Field-based matching (NEW)
            let base_name = pydantic_name
                .trim_end_matches("Read")
                .trim_end_matches("Create")
                .trim_end_matches("Update")
                .trim_end_matches("Delete")
                .trim_end_matches("Response")
                .trim_end_matches("Request")
                .trim_end_matches("Schema")
                .trim_end_matches("Model")
                .to_string();

            let mut best_match: Option<(String, SchemaReference, f64)> = None;

            for (sql_name, sql_schema) in &sqlalchemy_models {
                let mut match_score = 0.0;

                // Strategy 1: Exact name match
                if sql_name == &pydantic_name {
                    match_score = 1.0;
                }
                // Strategy 2: Base name match (remove suffixes)
                else if sql_name == &base_name {
                    match_score = 0.9;
                }
                // Strategy 3: Field-based matching
                else if !pydantic_fields.is_empty() {
                    // Extract SQLAlchemy fields from metadata
                    let sql_fields: Vec<crate::models::SQLAlchemyField> = sql_schema
                        .metadata
                        .get("fields")
                        .and_then(|json| serde_json::from_str(json).ok())
                        .unwrap_or_default();

                    let field_match = self.match_fields(&pydantic_fields, &sql_fields);
                    if field_match > 0.7 {
                        match_score = field_match;
                    }
                }

                // Update best match if this is better
                if match_score > 0.0 {
                    if let Some((_, _, best_score)) = &best_match {
                        if match_score > *best_score {
                            best_match = Some((sql_name.clone(), sql_schema.clone(), match_score));
                        }
                    } else {
                        best_match = Some((sql_name.clone(), sql_schema.clone(), match_score));
                    }
                }
            }

            // Use best match if found
            if let Some((sql_name, sql_schema, match_score)) = best_match {
                // Find the SQLAlchemy node in the graph for metadata
                let sql_node_id = self.graph.node_indices().find_map(|idx| {
                    if let Some(CallNode::Class { name, .. }) = self.graph.node_weight(idx) {
                        if name == &sql_name {
                            return Some(NodeId::from(idx));
                        }
                    }
                    None
                });

                let sql_file = PathBuf::from(&sql_schema.location.file);

                // Store the link in metadata
                pydantic_model
                    .metadata
                    .insert("sqlalchemy_model".to_string(), sql_name.clone());
                pydantic_model.metadata.insert(
                    "sqlalchemy_location".to_string(),
                    format!("{}:{}", sql_schema.location.file, sql_schema.location.line),
                );
                if let Some(node_id) = sql_node_id {
                    pydantic_model.metadata.insert(
                        "sqlalchemy_node_id".to_string(),
                        format!("{}", node_id.0.index()),
                    );
                }
                // Also store match score for debugging
                pydantic_model.metadata.insert(
                    "sqlalchemy_match_score".to_string(),
                    format!("{:.2}", match_score),
                );

                debug!(
                    pydantic_name = %pydantic_name,
                    sql_name = %sql_name,
                    sql_file = ?sql_file,
                    match_score = match_score,
                    "Linked Pydantic model to SQLAlchemy model"
                );

                // Update the cached model
                self.pydantic_models
                    .insert(pydantic_name.clone(), pydantic_model.clone());

                // Create DataFlow edge in graph if both nodes exist
                if let Some(pydantic_node_id) = self.find_class_node_by_name(&pydantic_name) {
                    if let Some(sql_node_id) = sql_node_id {
                        // Create bidirectional edge: Pydantic ↔ ORM
                        // Edge from Pydantic to ORM (transformation: from_attributes)
                        self.graph.add_edge(
                            pydantic_node_id.0,
                            sql_node_id.0,
                            CallEdge::DataFlow {
                                from: pydantic_node_id,
                                to: sql_node_id,
                                from_schema: pydantic_model.clone(),
                                to_schema: Box::new(sql_schema.clone()),
                                location: pydantic_model.location.clone(),
                                transformation: Some(
                                    crate::models::TransformationType::FromAttributes,
                                ),
                            },
                        );

                        // Create reverse edge (ORM → Pydantic) for bidirectional flow
                        // This represents the reverse transformation (ORM → Pydantic)
                        self.graph.add_edge(
                            sql_node_id.0,
                            pydantic_node_id.0,
                            CallEdge::DataFlow {
                                from: sql_node_id,
                                to: pydantic_node_id,
                                from_schema: sql_schema.clone(),
                                to_schema: Box::new(pydantic_model.clone()),
                                location: sql_schema.location.clone(),
                                transformation: Some(
                                    crate::models::TransformationType::OrmToPydantic,
                                ),
                            },
                        );

                        debug!(
                            pydantic_name = %pydantic_name,
                            sql_name = %sql_name,
                            "Created DataFlow edge: Pydantic ↔ ORM"
                        );
                    }
                }
            }
        }
    }

    /// Checks if a class is a SQLAlchemy model by analyzing its AST
    fn is_sqlalchemy_model(ast: &ast::Mod, class_name: &str) -> bool {
        if let ast::Mod::Module(module) = ast {
            for stmt in &module.body {
                if let ast::Stmt::ClassDef(class_def) = stmt {
                    if class_def.name.to_string() == class_name {
                        // Check if class inherits from common SQLAlchemy base classes
                        for base in &class_def.bases {
                            let base_str = Self::expr_to_string_static(base);
                            let last_segment = base_str
                                .rsplit('.')
                                .next()
                                .or_else(|| base_str.rsplit("::").next())
                                .unwrap_or(&base_str);

                            // Common SQLAlchemy base class names
                            if last_segment == "Base"
                                || last_segment == "declarative_base"
                                || base_str.contains("SQLAlchemyBase")
                                || base_str.contains("db.Model")
                                || base_str.contains("Model")
                            {
                                // Additional check: look for Column attributes in class body
                                for body_stmt in &class_def.body {
                                    if let ast::Stmt::AnnAssign(ann_assign) = body_stmt {
                                        // Check if annotation suggests SQLAlchemy (Column, relationship, etc.)
                                        let annotation_str = Self::expr_to_string_static(
                                            ann_assign.annotation.as_ref(),
                                        );
                                        if annotation_str.contains("Column")
                                            || annotation_str.contains("relationship")
                                            || annotation_str.contains("Mapped")
                                        {
                                            return true;
                                        }
                                    }
                                }
                                // If it inherits from Base/Model, it's likely SQLAlchemy
                                return true;
                            }
                        }
                    }
                }
            }
        }
        false
    }

    /// Helper function to convert expression to string (static version for use in is_sqlalchemy_model)
    fn expr_to_string_static(expr: &ast::Expr) -> String {
        match expr {
            ast::Expr::Name(name) => name.id.to_string(),
            ast::Expr::Attribute(attr) => {
                format!(
                    "{}.{}",
                    Self::expr_to_string_static(attr.value.as_ref()),
                    attr.attr
                )
            }
            ast::Expr::Subscript(sub) => {
                let base = Self::expr_to_string_static(sub.value.as_ref());
                let slice = Self::expr_to_string_static(sub.slice.as_ref());
                format!("{}[{}]", base, slice)
            }
            _ => String::new(),
        }
    }

    /// Gets the built graph
    pub fn into_graph(mut self) -> CallGraph {
        // Link Pydantic models to SQLAlchemy models before returning the graph
        self.link_pydantic_to_sqlalchemy();
        self.graph
    }

    /// Gets a reference to the graph
    pub fn graph(&self) -> &CallGraph {
        &self.graph
    }

    /// Gets a mutable reference to the graph
    pub fn graph_mut(&mut self) -> &mut CallGraph {
        &mut self.graph
    }
}

impl Default for CallGraphBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl CallGraphBuilder {
    fn process_imports(
        &mut self,
        module_ast: &ast::Mod,
        module_node: NodeId,
        file_path: &Path,
        converter: &LocationConverter,
    ) -> Result<()> {
        let file_path_str = file_path.to_string_lossy().to_string();
        let imports = self
            .parser
            .extract_imports(module_ast, &file_path_str, converter);

        // Store import information for schema resolution
        let normalized_file = Self::normalize_path(file_path);
        let mut file_imports_map = HashMap::new();

        for import in &imports {
            if let Err(err) = self.process_import(module_node, import, file_path) {
                warn!(
                    import_path = %import.path,
                    file_path = ?file_path,
                    error = %err,
                    "Failed to process import"
                );
            }

            // Store import mapping: imported name -> module path
            if !import.names.is_empty() {
                // from module import name1, name2
                for name in &import.names {
                    file_imports_map.insert(name.clone(), import.path.clone());
                }
            } else {
                // import module or import module as alias
                // For simple imports, we store the module path itself
                // This will be used when we need to resolve names from that module
                file_imports_map.insert(import.path.clone(), import.path.clone());
            }
        }

        // Store the import map after processing all imports
        if !file_imports_map.is_empty() {
            self.file_imports.insert(normalized_file, file_imports_map);
        }

        Ok(())
    }

    fn extract_functions_and_classes(
        &mut self,
        module_ast: &ast::Mod,
        file_path: &Path,
        converter: &LocationConverter,
    ) -> Result<()> {
        if let ast::Mod::Module(module) = module_ast {
            for stmt in &module.body {
                self.handle_definition(stmt, file_path, None, converter)?;
            }
        }
        Ok(())
    }

    fn handle_definition(
        &mut self,
        stmt: &ast::Stmt,
        file_path: &Path,
        class_context: Option<(String, NodeId)>,
        converter: &LocationConverter,
    ) -> Result<()> {
        match stmt {
            ast::Stmt::FunctionDef(func_def) => {
                if let Some((class_name, class_node)) = class_context {
                    let method_id = self.add_method_node(
                        &class_name,
                        class_node,
                        func_def,
                        file_path,
                        converter,
                    )?;
                    if let Some(CallNode::Class { methods, .. }) =
                        self.graph.node_weight_mut(*class_node)
                    {
                        if !methods.contains(&method_id) {
                            methods.push(method_id);
                        }
                    }
                } else {
                    self.add_function_node(func_def, file_path, converter)?;
                }
            }
            ast::Stmt::AsyncFunctionDef(func_def) => {
                if let Some((class_name, class_node)) = class_context {
                    let method_id = self.add_async_method_node(
                        &class_name,
                        class_node,
                        func_def,
                        file_path,
                        converter,
                    )?;
                    if let Some(CallNode::Class { methods, .. }) =
                        self.graph.node_weight_mut(*class_node)
                    {
                        if !methods.contains(&method_id) {
                            methods.push(method_id);
                        }
                    }
                } else {
                    self.add_async_function_node(func_def, file_path, converter)?;
                }
            }
            ast::Stmt::ClassDef(class_def) => {
                let class_node = self.add_class_node(class_def, file_path, converter)?;
                let class_name = class_def.name.to_string();
                for body_stmt in &class_def.body {
                    self.handle_definition(
                        body_stmt,
                        file_path,
                        Some((class_name.clone(), class_node)),
                        converter,
                    )?;
                }
            }
            _ => {}
        }
        Ok(())
    }

    fn add_function_node(
        &mut self,
        func_def: &ast::StmtFunctionDef,
        file_path: &Path,
        converter: &LocationConverter,
    ) -> Result<NodeId> {
        // Get location from AST
        let range = func_def.range();
        let (line, _column) = converter.byte_offset_to_location(range.start().into());

        let parameters = self.convert_parameters(&func_def.args, file_path, line);

        // Extract return type annotation if present
        let return_type = func_def
            .returns
            .as_ref()
            .map(|ret_ann| self.resolve_type_annotation(ret_ann, file_path, line));

        let node_id = NodeId::from(self.graph.add_node(CallNode::Function {
            name: func_def.name.to_string(),
            file: file_path.to_path_buf(),
            line,
            parameters,
            return_type,
        }));

        let key = Self::function_key(file_path, &func_def.name);
        self.function_nodes.insert(key, node_id);

        Ok(node_id)
    }

    fn add_async_function_node(
        &mut self,
        func_def: &ast::StmtAsyncFunctionDef,
        file_path: &Path,
        converter: &LocationConverter,
    ) -> Result<NodeId> {
        // Get location from AST
        let range = func_def.range();
        let (line, _column) = converter.byte_offset_to_location(range.start().into());

        let parameters = self.convert_parameters(&func_def.args, file_path, line);

        // Extract return type annotation if present
        let return_type = func_def
            .returns
            .as_ref()
            .map(|ret_ann| self.resolve_type_annotation(ret_ann, file_path, line));

        let node_id = NodeId::from(self.graph.add_node(CallNode::Function {
            name: func_def.name.to_string(),
            file: file_path.to_path_buf(),
            line,
            parameters,
            return_type,
        }));

        let key = Self::function_key(file_path, &func_def.name);
        self.function_nodes.insert(key, node_id);

        Ok(node_id)
    }

    fn add_method_node(
        &mut self,
        class_name: &str,
        class_node: NodeId,
        func_def: &ast::StmtFunctionDef,
        file_path: &Path,
        converter: &LocationConverter,
    ) -> Result<NodeId> {
        let range = func_def.range();
        let (line, _column) = converter.byte_offset_to_location(range.start().into());

        let mut parameters = self.convert_parameters(&func_def.args, file_path, line);
        // Check decorators before removing the first parameter
        let has_staticmethod = self.has_decorator(&func_def.decorator_list, "staticmethod");
        if !has_staticmethod && !parameters.is_empty() {
            // If there's no @staticmethod, remove the first parameter (self or cls)
            // For @classmethod we can remove cls, for regular methods - self
            parameters.remove(0);
        }

        // Extract return type annotation if present
        let return_type = func_def
            .returns
            .as_ref()
            .map(|ret_ann| self.resolve_type_annotation(ret_ann, file_path, line));

        let node_id = NodeId::from(self.graph.add_node(CallNode::Method {
            name: func_def.name.to_string(),
            class: class_node,
            parameters,
            return_type,
        }));

        let key = Self::function_key(file_path, &format!("{}.{}", class_name, func_def.name));
        self.function_nodes.insert(key, node_id);

        Ok(node_id)
    }

    fn add_async_method_node(
        &mut self,
        class_name: &str,
        class_node: NodeId,
        func_def: &ast::StmtAsyncFunctionDef,
        file_path: &Path,
        converter: &LocationConverter,
    ) -> Result<NodeId> {
        let range = func_def.range();
        let (line, _column) = converter.byte_offset_to_location(range.start().into());

        let mut parameters = self.convert_parameters(&func_def.args, file_path, line);
        // Check decorators before removing the first parameter
        let has_staticmethod = self.has_decorator(&func_def.decorator_list, "staticmethod");
        if !has_staticmethod && !parameters.is_empty() {
            // If there's no @staticmethod, remove the first parameter (self or cls)
            parameters.remove(0);
        }

        // Extract return type annotation if present
        let return_type = func_def
            .returns
            .as_ref()
            .map(|ret_ann| self.resolve_type_annotation(ret_ann, file_path, line));

        let node_id = NodeId::from(self.graph.add_node(CallNode::Method {
            name: func_def.name.to_string(),
            class: class_node,
            parameters,
            return_type,
        }));

        let key = Self::function_key(file_path, &format!("{}.{}", class_name, func_def.name));
        self.function_nodes.insert(key, node_id);

        Ok(node_id)
    }

    fn add_class_node(
        &mut self,
        class_def: &ast::StmtClassDef,
        file_path: &Path,
        converter: &LocationConverter,
    ) -> Result<NodeId> {
        let class_name = class_def.name.to_string();

        // Check if this is a Pydantic model and cache it
        // Strategy:
        //   1) прямое наследование от BaseModel / pydantic.BaseModel
        //   2) рекурсивное наследование от уже известной Pydantic‑модели (например, ItemCreate(ItemBase))
        //   3) рекурсивная проверка базовых классов в текущем файле

        // Read AST to check base classes in current file
        let file_ast = if let Ok(source) = std::fs::read_to_string(file_path) {
            rustpython_parser::parse(
                &source,
                rustpython_parser::Mode::Module,
                file_path.to_string_lossy().as_ref(),
            )
            .ok()
        } else {
            None
        };

        let is_pydantic = self.is_pydantic_model_recursive(class_def, file_ast.as_ref(), file_path);

        if is_pydantic {
            let range = class_def.range();
            let (line, column) = converter.byte_offset_to_location(range.start().into());

            // Extract Pydantic models from file to get full metadata
            // Read the file to extract models properly
            if let Ok(source) = std::fs::read_to_string(file_path) {
                if let Ok(ast) = rustpython_parser::parse(
                    &source,
                    rustpython_parser::Mode::Module,
                    file_path.to_string_lossy().as_ref(),
                ) {
                    let models = self.parser.extract_pydantic_models(
                        &ast,
                        &file_path.to_string_lossy(),
                        converter,
                    );

                    if let Some(model) = models.iter().find(|m| m.name == class_name) {
                        let mut model = model.clone();

                        // Enrich with JSON schema if extractor is available
                        if let Some(ref extractor) = self.schema_extractor {
                            if let Err(err) = extractor.enrich_schema(&mut model) {
                                debug!(
                                    class_name = %class_name,
                                    error = %err,
                                    "Failed to enrich schema"
                                );
                            }
                        }

                        self.pydantic_models.insert(class_name.clone(), model);
                    } else {
                        // Fallback: create basic schema reference
                        let schema_ref = SchemaReference {
                            name: class_name.clone(),
                            schema_type: SchemaType::Pydantic,
                            location: Location {
                                file: file_path.to_string_lossy().to_string(),
                                line,
                                column: Some(column),
                            },
                            metadata: HashMap::new(),
                        };
                        self.pydantic_models.insert(class_name.clone(), schema_ref);
                    }
                }
            }
        }

        // Check if this is an ORM model and cache it
        if let Ok(Some(orm_model)) =
            self.extract_and_cache_orm_model(class_def, file_path, converter)
        {
            debug!(
                model_name = %orm_model.name,
                file_path = %file_path.to_string_lossy(),
                line = orm_model.location.line,
                "Found ORM model"
            );
        }

        let node_id = NodeId::from(self.graph.add_node(CallNode::Class {
            name: class_name,
            file: file_path.to_path_buf(),
            methods: Vec::new(),
        }));

        let key = Self::function_key(file_path, &class_def.name);
        self.function_nodes.insert(key, node_id);

        Ok(node_id)
    }

    /// Extracts and caches all Pydantic models from a file
    /// This should be called before resolving type annotations to ensure models are available
    fn extract_and_cache_pydantic_models(&mut self, file_path: &Path) -> Result<()> {
        // 1. Check if file was already processed
        let normalized = Self::normalize_path(file_path);
        if self.processed_files.contains(&normalized) {
            return Ok(());
        }

        // 2. Check if file exists and is a Python file
        if !file_path.exists() || file_path.extension() != Some(std::ffi::OsStr::new("py")) {
            return Ok(());
        }

        // 3. Read and parse file
        let source = fs::read_to_string(file_path)?;
        let ast = parse(&source, Mode::Module, file_path.to_string_lossy().as_ref())?;
        let converter = LocationConverter::new(source);

        // 4. Extract all Pydantic models
        let models =
            self.parser
                .extract_pydantic_models(&ast, &file_path.to_string_lossy(), &converter);

        // 5. Add all models to cache (with JSON schema enrichment)
        for mut model in models {
            // Enrich with JSON schema if extractor is available
            if let Some(ref extractor) = self.schema_extractor {
                if let Err(err) = extractor.enrich_schema(&mut model) {
                    debug!(
                        model_name = %model.name,
                        error = %err,
                        "Failed to enrich schema"
                    );
                }
            }
            self.pydantic_models.insert(model.name.clone(), model);
        }

        Ok(())
    }

    fn process_calls(
        &mut self,
        module_ast: &ast::Mod,
        module_node: NodeId,
        file_path: &Path,
        converter: &LocationConverter,
    ) -> Result<()> {
        let file_path_str = file_path.to_string_lossy().to_string();
        let calls = self
            .parser
            .extract_calls(module_ast, &file_path_str, converter);
        for call in calls {
            let caller_node = match &call.caller {
                Some(caller_name) => self.find_function_node(caller_name, file_path),
                None => Some(module_node),
            };

            if let Some(caller) = caller_node {
                if let Err(err) = self.process_call(caller, &call, file_path) {
                    warn!(
                        call_name = %call.name,
                        file_path = ?file_path,
                        error = %err,
                        "Failed to process call"
                    );
                }
            }
        }
        Ok(())
    }

    fn process_decorators(
        &mut self,
        module_ast: &ast::Mod,
        file_path: &Path,
        converter: &LocationConverter,
    ) -> Result<()> {
        let file_path_str = file_path.to_string_lossy().to_string();
        let decorators = self
            .parser
            .extract_decorators(module_ast, &file_path_str, converter);

        debug!(
            decorator_count = decorators.len(),
            file_path = ?file_path,
            "Processing decorators in file"
        );

        for decorator in decorators {
            debug!(
                decorator_name = %decorator.name,
                target_function = ?decorator.target_function,
                file_path = %decorator.location.file,
                line = decorator.location.line,
                "Extracted decorator"
            );

            if let Err(err) = self.process_decorator(&decorator, file_path) {
                debug!(
                    decorator_name = %decorator.name,
                    file_path = ?file_path,
                    error = %err,
                    "Failed to process decorator"
                );
            }
        }
        Ok(())
    }

    fn convert_parameters(
        &self,
        args: &ast::Arguments,
        file_path: &Path,
        line: usize,
    ) -> Vec<Parameter> {
        let mut params = Vec::new();

        // posonlyargs, args, kwonlyargs are Vec<ArgWithDefault>
        // default is already stored inside each ArgWithDefault

        // Process posonlyargs
        for arg in &args.posonlyargs {
            params.push(self.create_parameter_from_arg_with_default(arg, file_path, line));
        }

        // Process args
        for arg in &args.args {
            params.push(self.create_parameter_from_arg_with_default(arg, file_path, line));
        }

        // Process kwonlyargs
        for arg in &args.kwonlyargs {
            params.push(self.create_parameter_from_arg_with_default(arg, file_path, line));
        }

        if let Some(arg) = &args.vararg {
            // vararg is Option<Box<Arg>>, without default
            params.push(self.create_parameter_from_arg(arg, None, file_path, line));
        }
        if let Some(arg) = &args.kwarg {
            // kwarg is Option<Box<Arg>>, without default
            params.push(self.create_parameter_from_arg(arg, None, file_path, line));
        }
        params
    }

    /// Creates a parameter from ArgWithDefault (with default)
    fn create_parameter_from_arg_with_default(
        &self,
        arg: &ast::ArgWithDefault,
        file_path: &Path,
        line: usize,
    ) -> Parameter {
        let optional = arg.default.is_some();
        let default_value = arg.default.as_deref().map(|expr| {
            // Extract text representation of the default expression
            match expr {
                ast::Expr::Constant(constant) => match &constant.value {
                    ast::Constant::Str(s) => format!("\"{}\"", s),
                    ast::Constant::Int(i) => i.to_string(),
                    ast::Constant::Float(f) => f.to_string(),
                    ast::Constant::Bool(b) => b.to_string(),
                    ast::Constant::None => "None".to_string(),
                    _ => format!("{:?}", constant.value),
                },
                _ => format!("{:?}", expr),
            }
        });

        // Extract type annotation if present
        let type_info = if let Some(annotation) = &arg.def.annotation {
            self.resolve_type_annotation(annotation, file_path, line)
        } else {
            TypeInfo {
                base_type: BaseType::Unknown,
                schema_ref: None,
                constraints: Vec::new(),
                optional,
            }
        };

        Parameter {
            name: arg.def.arg.to_string(),
            type_info,
            optional,
            default_value,
        }
    }

    /// Creates a parameter from Arg (without default)
    /// Takes &Box<Arg>
    fn create_parameter_from_arg(
        &self,
        arg: &ast::Arg,
        default: Option<&ast::Expr>,
        file_path: &Path,
        line: usize,
    ) -> Parameter {
        let optional = default.is_some();
        let default_value = default.map(|expr| {
            // Extract text representation of the default expression
            match expr {
                ast::Expr::Constant(constant) => match &constant.value {
                    ast::Constant::Str(s) => format!("\"{}\"", s),
                    ast::Constant::Int(i) => i.to_string(),
                    ast::Constant::Float(f) => f.to_string(),
                    ast::Constant::Bool(b) => b.to_string(),
                    ast::Constant::None => "None".to_string(),
                    _ => format!("{:?}", constant.value),
                },
                _ => format!("{:?}", expr),
            }
        });

        // Extract type annotation if present
        let type_info = if let Some(annotation) = &arg.annotation {
            self.resolve_type_annotation(annotation, file_path, line)
        } else {
            TypeInfo {
                base_type: BaseType::Unknown,
                schema_ref: None,
                constraints: Vec::new(),
                optional,
            }
        };

        Parameter {
            name: arg.arg.to_string(),
            type_info,
            optional,
            default_value,
        }
    }

    /// Extracts inner type from Annotated[T, Body()] or Annotated[T, Query()]
    /// Returns inner_type_expr and annotation_type string
    /// annotation_type: "Body", "Query", "Path", "Header", or None
    fn extract_annotated_type(
        &self,
        annotation: &ast::Expr,
    ) -> Option<(Box<ast::Expr>, Option<String>)> {
        if let ast::Expr::Subscript(sub) = annotation {
            let base_str = self.parser.expr_to_string(sub.value.as_ref());
            if base_str == "Annotated" || base_str.ends_with(".Annotated") {
                if let ast::Expr::Tuple(tuple) = sub.slice.as_ref() {
                    if tuple.elts.len() >= 2 {
                        let inner_type = tuple.elts[0].clone();
                        let annotation_expr = &tuple.elts[1];

                        // Extract annotation type (Body, Query, Path, Header)
                        let annotation_type = self.extract_annotation_type_name(annotation_expr);

                        return Some((Box::new(inner_type), annotation_type));
                    }
                }
            }
        }
        None
    }

    /// Extracts annotation type name from expression
    /// Body() -> "Body", Query() -> "Query", etc.
    fn extract_annotation_type_name(&self, expr: &ast::Expr) -> Option<String> {
        match expr {
            ast::Expr::Call(call) => {
                // Use expr_to_string to get the function name
                let func_str = self.parser.expr_to_string(&call.func);
                // Extract last component (Body, Query, etc.)
                // Handle cases like "Body(...)" or "fastapi.Body(...)"
                if let Some(open_paren) = func_str.find('(') {
                    let name_part = &func_str[..open_paren];
                    let last_component = name_part.rsplit('.').next().unwrap_or(name_part);
                    return Some(last_component.to_string());
                } else {
                    let last_component = func_str.rsplit('.').next().unwrap_or(&func_str);
                    return Some(last_component.to_string());
                }
            }
            ast::Expr::Name(name) => {
                return Some(name.id.to_string());
            }
            _ => {}
        }
        None
    }

    /// Extracts inner type from Annotated[T, Body()] string representation
    /// Annotated[ItemCreate, Body()] -> ItemCreate
    fn extract_annotated_inner_type(&self, annotated_str: &str) -> String {
        // Format: Annotated[TypeName, CallExpr]
        if let Some(start) = annotated_str.find('[') {
            if let Some(comma) = annotated_str[start..].find(',') {
                let inner = &annotated_str[start + 1..start + comma];
                return inner.trim().to_string();
            }
            // If no comma, try to extract everything before first ']'
            if let Some(end) = annotated_str[start..].find(']') {
                let inner = &annotated_str[start + 1..start + end];
                return inner.trim().to_string();
            }
        }
        String::new()
    }

    /// Checks if a parameter is a request body (not Query/Path/Header)
    fn is_request_body_parameter(&self, param: &Parameter) -> bool {
        // Check if type_info contains Annotated with Query/Path/Header
        if let Some(ref schema_ref) = param.type_info.schema_ref {
            let type_name = &schema_ref.name;

            // Check if it's Annotated with Query, Path, or Header
            if type_name.contains("Query(")
                || type_name.contains("Path(")
                || type_name.contains("Header(")
            {
                return false; // Not a body parameter
            }

            // If it's Annotated with Body, it's definitely a body parameter
            if type_name.contains("Body(") {
                return true;
            }
        }

        // Check parameter name for service types
        let service_types = [
            "Depends",
            "Request",
            "Response",
            "HTTPException",
            "BackgroundTasks",
            "UploadFile",
            "File",
            "Form",
            "db",
            "session",
            "current_user",
            "user",
        ];
        if service_types
            .iter()
            .any(|&st| param.name.contains(st) || param.name == st)
        {
            return false;
        }

        // If it's a Pydantic model, it's likely a body parameter
        if let Some(ref schema_ref) = param.type_info.schema_ref {
            if schema_ref.schema_type == SchemaType::Pydantic {
                return true;
            }
        }

        // Check if type name matches a Pydantic model
        let type_str = if let Some(ref schema_ref) = param.type_info.schema_ref {
            &schema_ref.name
        } else {
            return false;
        };

        let simple_name = type_str
            .rsplit('.')
            .next()
            .unwrap_or(type_str)
            .trim()
            .to_string();

        if self.pydantic_models.contains_key(&simple_name) {
            return true;
        }

        false
    }

    /// Resolves a type annotation to TypeInfo, checking if it's a Pydantic model
    fn resolve_type_annotation(
        &self,
        annotation: &ast::Expr,
        file_path: &Path,
        line: usize,
    ) -> TypeInfo {
        // First, check if it's Annotated[T, ...]
        if let Some((inner_type_expr, _annotation_type)) = self.extract_annotated_type(annotation) {
            // Recursively resolve the inner type
            return self.resolve_type_annotation(inner_type_expr.as_ref(), file_path, line);
        }

        // Continue with existing logic for non-Annotated types
        // Convert annotation to string representation
        let type_str = self.parser.expr_to_string(annotation);

        // Extract the base type name (handle cases like "Optional[User]", "List[User]", etc.)
        let base_type_name = if let Some(open_bracket) = type_str.find('[') {
            &type_str[..open_bracket]
        } else {
            &type_str
        };

        // Check if it's Optional
        let is_optional = base_type_name == "Optional" || type_str.contains("Optional");

        // Try to find the actual type name (for Optional[T], extract T)
        let actual_type_name = if base_type_name == "Optional" {
            // Extract type from Optional[T]
            if let Some(start) = type_str.find('[') {
                if let Some(end) = type_str.rfind(']') {
                    type_str[start + 1..end].trim()
                } else {
                    base_type_name
                }
            } else {
                base_type_name
            }
        } else {
            base_type_name
        };

        // Detect dict[str, Any], dict[str, int], dict[str, str], dict[str, list[int]], etc.
        // Also detect list[dict[str, Any]] patterns
        // Any dict type without a proper Pydantic schema should be flagged
        let is_missing_schema = if actual_type_name == "dict" || actual_type_name == "Dict" {
            // Check if it's a generic dict type (dict[...])
            // If it contains brackets, it's a generic dict and should be flagged
            // unless it's a specific typed dict like TypedDict
            if type_str.contains('[') {
                // Check if it's TypedDict (which is acceptable)
                !type_str.contains("TypedDict")
            } else {
                // Plain dict without type parameters is also missing schema
                true
            }
        } else if actual_type_name == "list" || actual_type_name == "List" {
            // Check for list[dict[str, Any]] patterns
            // Extract the inner type from list[...]
            if let Some(start) = type_str.find('[') {
                if let Some(end) = type_str.rfind(']') {
                    let inner_type = type_str[start + 1..end].trim();
                    // Check if inner type is dict[...]
                    if inner_type.starts_with("dict") || inner_type.starts_with("Dict") {
                        // list[dict[...]] should have response_model
                        true
                    } else {
                        false
                    }
                } else {
                    false
                }
            } else {
                false
            }
        } else {
            // Check for 'any' or 'Any' types
            actual_type_name == "any" || actual_type_name == "Any"
        };

        // Check if this type is a Pydantic model in our cache
        let schema_ref = if is_missing_schema {
            // Create SchemaReference with missing_schema flag
            let mut metadata = HashMap::new();
            metadata.insert("missing_schema".to_string(), "true".to_string());
            metadata.insert("base_type".to_string(), format!("{:?}", BaseType::Object));

            Some(SchemaReference {
                name: "Object".to_string(),
                schema_type: SchemaType::JsonSchema, // Keep as JsonSchema for missing schemas
                location: Location {
                    file: file_path.to_string_lossy().to_string(),
                    line,
                    column: None,
                },
                metadata,
            })
        } else if let Some(schema) = self.pydantic_models.get(actual_type_name) {
            Some(schema.clone())
        } else {
            // Try to resolve the type through various strategies:
            // 1. Check if it's a qualified name (e.g., "models.User", "db.schemas.RegisterRequest")
            //    Extract the last component and search for it
            // 2. Check all models in cache for partial matches
            // 3. Try to resolve through imports (if we had access to them)

            let simple_name = if let Some(last_dot) = actual_type_name.rfind('.') {
                &actual_type_name[last_dot + 1..]
            } else {
                actual_type_name
            };

            // First try simple name
            if let Some(schema) = self.pydantic_models.get(simple_name) {
                Some(schema.clone())
            } else {
                // Try to find by exact match in cache (case-insensitive)
                // This handles cases where the type might be imported from another module
                // but we only have the simple name in cache
                self.pydantic_models
                    .iter()
                    .find(|(name, _)| {
                        // Check if names match (case-insensitive)
                        name.eq_ignore_ascii_case(simple_name)
                    })
                    .map(|(_, schema)| schema.clone())
            }
        };

        // Determine base type
        let base_type = if schema_ref.is_some() {
            BaseType::Object
        } else {
            // Try to infer base type from type name
            match actual_type_name.to_lowercase().as_str() {
                "str" | "string" => BaseType::String,
                "int" | "integer" | "float" | "number" => BaseType::Number,
                "bool" | "boolean" => BaseType::Boolean,
                "list" | "array" | "tuple" => BaseType::Array,
                "dict" | "object" => BaseType::Object,
                _ => BaseType::Unknown,
            }
        };

        TypeInfo {
            base_type,
            schema_ref,
            constraints: Vec::new(),
            optional: is_optional,
        }
    }

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

    fn function_key(path: &Path, name: &str) -> String {
        format!("{}::{}", Self::normalize_path(path).to_string_lossy(), name)
    }

    fn find_function_node(&self, name: &str, current_file: &Path) -> Option<NodeId> {
        let normalized = Self::normalize_path(current_file);
        let direct_key = Self::function_key(&normalized, name);

        debug!(
            function_name = %name,
            file_path = ?current_file,
            direct_key = %direct_key,
            "Searching for function"
        );

        if let Some(node) = self.function_nodes.get(&direct_key) {
            debug!(
                function_name = %name,
                node_index = node.0.index(),
                "Found direct match for function"
            );
            return Some(*node);
        }

        // Find all matches by ends_with("::name")
        let matches: Vec<_> = self
            .function_nodes
            .iter()
            .filter(|(key, _)| key.ends_with(&format!("::{}", name)))
            .collect();

        if self.verbose {
            debug!(
                match_count = matches.len(),
                function_name = %name,
                "Found suffix matches"
            );
        }

        if matches.is_empty() {
            debug!(
                function_name = %name,
                "No suffix matches, trying graph search"
            );
            return crate::call_graph::find_node_by_name(&self.graph, name);
        }

        if matches.len() == 1 {
            debug!(
                function_name = %name,
                node_index = matches[0].1.0.index(),
                key = %matches[0].0,
                "Single match found"
            );
            return Some(*matches[0].1);
        }

        // Disambiguation: find the best match
        // 1. Prefer exact module path match
        let current_dir = normalized.parent().map(|p| p.to_path_buf());
        if let Some(dir) = current_dir {
            if let Some((key, node)) = matches.iter().find(|(key, _)| {
                if let Some(key_path) = Self::extract_path_from_key(key) {
                    key_path.parent() == Some(&dir)
                } else {
                    false
                }
            }) {
                debug!(
                    function_name = %name,
                    node_index = node.0.index(),
                    key = %key,
                    "Selected exact path match"
                );
                return Some(**node);
            }
        }

        // 2. Prefer matches with the longest common prefix
        let best_match = matches.iter().max_by_key(|(key, _)| {
            if let Some(key_path) = Self::extract_path_from_key(key) {
                Self::common_prefix_length(&normalized, &key_path)
            } else {
                0
            }
        });

        if let Some((key, node)) = best_match {
            // Log warning about ambiguity
            warn!(
                function_name = %name,
                match_count = matches.len(),
                node_index = node.0.index(),
                key = %key,
                "Ambiguous function name, selected best match"
            );
            return Some(**node);
        }

        // 3. Fallback: select first deterministically (sorted by key)
        let mut sorted_matches = matches.clone();
        sorted_matches.sort_by(|(key_a, _), (key_b, _)| key_a.cmp(key_b));
        if let Some((key, node)) = sorted_matches.first() {
            debug!(
                function_name = %name,
                node_index = node.0.index(),
                key = %key,
                "Selected first match (sorted)"
            );
            Some(**node)
        } else {
            debug!(
                function_name = %name,
                "No match found after all attempts"
            );
            None
        }
    }

    /// Extracts path from function key (format "path::name")
    fn extract_path_from_key(key: &str) -> Option<PathBuf> {
        if let Some(pos) = key.rfind("::") {
            let path = PathBuf::from(&key[..pos]);
            // Try to canonicalize, but return non-canonicalized path on error
            path.canonicalize().ok().or(Some(path))
        } else {
            None
        }
    }

    /// Calculates the length of the common prefix of two paths
    fn common_prefix_length(path1: &Path, path2: &Path) -> usize {
        let components1: Vec<_> = path1.components().collect();
        let components2: Vec<_> = path2.components().collect();
        let min_len = components1.len().min(components2.len());
        let mut common = 0;
        for i in 0..min_len {
            if components1[i] == components2[i] {
                common += 1;
            } else {
                break;
            }
        }
        common
    }

    fn node_file_path(&self, node_id: NodeId) -> Option<PathBuf> {
        let node = self.graph.node_weight(*node_id)?.clone();
        match node {
            CallNode::Function { file, .. } => Some(file),
            CallNode::Class { file, .. } => Some(file),
            CallNode::Module { path } => Some(path),
            CallNode::Method { class, .. } => {
                self.graph
                    .node_weight(*class)
                    .and_then(|owner| match owner {
                        CallNode::Class { file, .. } => Some(file.clone()),
                        _ => None,
                    })
            }
            CallNode::Route { location, .. } => {
                if location.file.is_empty() {
                    None
                } else {
                    Some(PathBuf::from(location.file))
                }
            }
            CallNode::Schema { schema } => {
                if schema.location.file.is_empty() {
                    None
                } else {
                    Some(PathBuf::from(&schema.location.file))
                }
            }
        }
    }

    fn resolve_import_path(&self, import_path: &str, current_file: &Path) -> Result<PathBuf> {
        let normalized_current = Self::normalize_path(current_file);
        let base_dir = normalized_current
            .parent()
            .map(|p| p.to_path_buf())
            .or_else(|| self.project_root.clone())
            .unwrap_or_else(|| PathBuf::from("."));

        // First try as relative import (if it starts with .)
        let candidate = if import_path.starts_with('.') {
            self.resolve_relative_import(import_path, &base_dir)
        } else {
            // For absolute imports, try multiple strategies:
            // 1. From project root (standard absolute import)
            // 2. From current file's directory (common pattern: api.routers from api/main.py)
            // 3. From app directory (for imports like "app.schemas" from "app/routes/items.py")
            let absolute_candidate = self.resolve_absolute_import(import_path);

            // Try relative resolution first (common case: api.routers from api/main.py)
            // If import starts with current directory name, treat as same-package import
            let relative_candidate = {
                let current_dir_name = normalized_current
                    .parent()
                    .and_then(|p| p.file_name())
                    .and_then(|n| n.to_str());

                let import_path_to_resolve = if let Some(dir_name) = current_dir_name {
                    // If import starts with current directory name, strip it
                    // e.g., from backend/api/main.py, "api.routers" -> "routers"
                    if import_path.starts_with(&format!("{}.", dir_name)) {
                        &import_path[dir_name.len() + 1..]
                    } else {
                        import_path
                    }
                } else {
                    import_path
                };

                let replaced = import_path_to_resolve.replace('.', std::path::MAIN_SEPARATOR_STR);
                let mut path = base_dir.join(&replaced);
                if path.is_dir() {
                    path = path.join("__init__.py");
                } else if path.extension().is_none() {
                    path.set_extension("py");
                }
                path
            };

            // Try resolving from app directory (for imports like "app.schemas")
            // This handles cases where file is in app/routes/items.py and imports from app/schemas.py
            let app_dir_candidate = {
                // Find "app" directory by going up from current file
                let mut search_path = base_dir.clone();
                let mut app_dir: Option<PathBuf> = None;

                // First check if base_dir itself is named "app"
                if let Some(dir_name) = search_path.file_name().and_then(|n| n.to_str()) {
                    if dir_name == "app" {
                        app_dir = Some(search_path.clone());
                    }
                }

                // Then look for "app" directory in parent directories
                while let Some(parent) = search_path.parent() {
                    if let Some(dir_name) = parent.file_name().and_then(|n| n.to_str()) {
                        if dir_name == "app" {
                            app_dir = Some(parent.to_path_buf());
                            break;
                        }
                    }
                    search_path = parent.to_path_buf();
                }

                if let Some(app_dir_path) = app_dir {
                    // If import starts with "app.", resolve from app directory
                    if let Some(remaining) = import_path.strip_prefix("app.") {
                        let replaced = remaining.replace('.', std::path::MAIN_SEPARATOR_STR);
                        let mut path = app_dir_path.join(&replaced);
                        if path.is_dir() {
                            path = path.join("__init__.py");
                        } else if path.extension().is_none() {
                            path.set_extension("py");
                        }
                        Some(path)
                    } else {
                        None
                    }
                } else {
                    None
                }
            };

            if self.verbose {
                debug!(
                    import_path = %import_path,
                    absolute_candidate = ?absolute_candidate,
                    relative_candidate = ?relative_candidate,
                    app_dir_candidate = ?app_dir_candidate,
                    "Trying to resolve import"
                );
            }

            // Try app directory first (for imports like "app.schemas")
            if let Some(ref app_candidate) = app_dir_candidate {
                if app_candidate.exists() {
                    debug!(
                        import_path = %import_path,
                        candidate_path = ?app_candidate,
                        "Found import via app directory resolution"
                    );
                    return Ok(app_candidate.clone());
                }
            }

            // Try relative first (more common for same-package imports)
            if relative_candidate.exists() {
                debug!(
                    import_path = %import_path,
                    candidate_path = ?relative_candidate,
                    "Found import via relative resolution"
                );
                relative_candidate
            } else if absolute_candidate.exists() {
                debug!(
                    import_path = %import_path,
                    candidate_path = ?absolute_candidate,
                    "Found import via absolute resolution"
                );
                absolute_candidate
            } else {
                // Neither exists, return absolute for error message
                absolute_candidate
            }
        };

        if candidate.exists() {
            return Ok(candidate);
        }

        // Try adding .py extension if file not found
        if candidate.extension().is_none() {
            let mut with_ext = candidate.clone();
            with_ext.set_extension("py");
            if with_ext.exists() {
                debug!(
                    import_path = %import_path,
                    candidate_path = ?with_ext,
                    "Found import with .py extension"
                );
                return Ok(with_ext);
            }
        }

        debug!(
            import_path = %import_path,
            current_file = ?current_file,
            candidate = ?candidate,
            "Cannot resolve import path"
        );

        anyhow::bail!(
            "Cannot resolve import path {} from {:?}",
            import_path,
            current_file
        )
    }

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
            let replaced = remaining.replace('.', std::path::MAIN_SEPARATOR_STR);
            path = path.join(replaced);
        }

        if path.is_dir() {
            path.join("__init__.py")
        } else if path.extension().is_none() {
            let mut with_ext = path.clone();
            with_ext.set_extension("py");
            with_ext
        } else {
            path
        }
    }

    fn resolve_absolute_import(&self, import_path: &str) -> PathBuf {
        let root = self
            .project_root
            .clone()
            .unwrap_or_else(|| PathBuf::from("."));
        let replaced = import_path.replace('.', std::path::MAIN_SEPARATOR_STR);
        let mut path = root.join(replaced);

        if path.is_dir() {
            path = path.join("__init__.py");
        } else if path.extension().is_none() {
            path.set_extension("py");
        }
        path
    }

    /// Resolves a schema name from imports in the current file
    /// Tries to find the module that imports this schema and extracts it
    fn resolve_schema_from_imports(
        &mut self,
        schema_name: &str,
        current_file: &Path,
    ) -> Result<()> {
        let normalized_file = Self::normalize_path(current_file);

        // Strategy 1: Check if we have import information for this file
        let module_paths_to_try: Vec<String> = {
            if let Some(imports_map) = self.file_imports.get(&normalized_file) {
                let mut paths = Vec::new();

                // Find the module path for this schema name
                if let Some(module_path_str) = imports_map.get(schema_name) {
                    paths.push(module_path_str.clone());
                }

                // Strategy 2: Try to find module by partial match (e.g., "schemas" for "app.schemas")
                // If schema_name is "ItemRead" and we have "schemas" -> "app.schemas" in imports,
                // try resolving "app.schemas" and searching for "ItemRead" there
                for (imported_name, module_path_str) in imports_map.iter() {
                    // If imported name matches a common module pattern (schemas, models, etc.)
                    if (imported_name == "schemas" || imported_name == "models")
                        && !paths.contains(module_path_str)
                    {
                        paths.push(module_path_str.clone());
                    }
                }
                paths
            } else {
                Vec::new()
            }
        };

        // Try resolving from collected module paths
        for module_path_str in &module_paths_to_try {
            if let Ok(module_file) = self.resolve_import_path(module_path_str, current_file) {
                // Extract and cache Pydantic models from the imported module
                if let Err(err) = self.extract_and_cache_pydantic_models(&module_file) {
                    if self.verbose {
                        debug!(
                            module_file = ?module_file,
                            error = %err,
                            "Failed to extract models"
                        );
                    }
                } else {
                    // Check if the schema is now in cache
                    if self.pydantic_models.contains_key(schema_name) {
                        if self.verbose {
                            debug!(
                                schema_name = %schema_name,
                                module_file = ?module_file,
                                "Successfully resolved schema from module"
                            );
                        }
                        return Ok(());
                    }
                }
            }
        }

        // Strategy 3: Fallback - search all processed files for the schema by name
        // This handles cases where imports weren't properly tracked
        // Collect file paths first to avoid borrowing issues
        let processed_files: Vec<PathBuf> = self.processed_files.iter().cloned().collect();
        for file_path in &processed_files {
            if let Err(err) = self.extract_and_cache_pydantic_models(file_path) {
                debug!(
                    file_path = ?file_path,
                    error = %err,
                    "Failed to extract models during fallback search"
                );
            } else if self.pydantic_models.contains_key(schema_name) {
                debug!(
                    schema_name = %schema_name,
                    file_path = ?file_path,
                    "Successfully resolved schema via fallback search"
                );
                return Ok(());
            }
        }

        Ok(())
    }

    fn normalize_path(path: &Path) -> PathBuf {
        path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
    }

    fn is_route_decorator(&self, name: &str) -> bool {
        // Check for common FastAPI route patterns
        // 1. Direct app/router access: app.get, router.post, etc.
        if name.starts_with("app.") || name.starts_with("router.") {
            return true;
        }

        // 2. Contains .route: api_router.route, main_router.route
        if name.contains(".route") {
            return true;
        }

        // 3. Common router variable names with HTTP methods
        let router_names = [
            "api_router",
            "api",
            "main_router",
            "main",
            "fastapi_router",
            "fastapi",
            "app_router",
            "web_router",
            "r",
            "rt",
            "router_instance",
        ];

        for router_name in &router_names {
            if name.starts_with(&format!("{}.", router_name)) {
                // Check if last component equals an HTTP method
                let http_methods = ["get", "post", "put", "patch", "delete", "head", "options"];
                if let Some(last_component) = name.split('.').next_back() {
                    if http_methods.contains(&last_component) {
                        return true;
                    }
                }
            }
        }

        false
    }

    fn extract_http_method(&self, decorator_name: &str) -> Option<HttpMethod> {
        let method_part = decorator_name.split('.').nth(1)?;
        method_part.parse().ok()
    }

    /// Checks if the specified decorator is in the decorator list
    fn has_decorator(&self, decorator_list: &[ast::Expr], decorator_name: &str) -> bool {
        for decorator in decorator_list {
            if let Some(name) = self.get_decorator_name(decorator) {
                // Check exact match or match of the last segment
                if name == decorator_name || name.ends_with(&format!(".{}", decorator_name)) {
                    return true;
                }
            }
        }
        false
    }

    /// Extracts decorator name from AST expression
    #[allow(clippy::only_used_in_recursion)]
    fn get_decorator_name(&self, decorator: &ast::Expr) -> Option<String> {
        match decorator {
            ast::Expr::Name(name) => Some(name.id.to_string()),
            ast::Expr::Attribute(attr) => {
                if let Some(base) = self.get_decorator_name(&attr.value) {
                    Some(format!("{}.{}", base, attr.attr))
                } else {
                    Some(attr.attr.to_string())
                }
            }
            ast::Expr::Call(call_expr) => self.get_decorator_name(&call_expr.func),
            _ => None,
        }
    }

    /// Detects if a call is a Pydantic transformation method
    /// Returns transformation type and model name if detected
    fn detect_pydantic_transformation(&self, call: &Call) -> Option<(String, String)> {
        // Check for method calls like model.model_validate(), model.model_dump(), etc.
        // Pattern: <model_name>.<method>(...)
        if call.name.contains('.') {
            let parts: Vec<&str> = call.name.split('.').collect();
            if parts.len() == 2 {
                let model_name = parts[0].to_string();
                let method = parts[1];

                // List of Pydantic transformation methods
                let transformation_methods = [
                    "model_validate",
                    "model_validate_json",
                    "model_dump",
                    "model_dump_json",
                    "model_serialize",
                    "from_orm",
                    "dict",
                    "json",
                    "parse_obj",
                    "parse_raw",
                ];

                if transformation_methods.contains(&method) {
                    return Some((method.to_string(), model_name));
                }
            }
        }

        // Also check for direct method calls (e.g., BaseModel.model_validate(...))
        if call.name.ends_with(".model_validate")
            || call.name.ends_with(".model_dump")
            || call.name.ends_with(".from_orm")
            || call.name.ends_with(".dict")
            || call.name.ends_with(".json")
        {
            let parts: Vec<&str> = call.name.rsplitn(2, '.').collect();
            if parts.len() == 2 {
                let method = parts[0];
                let model_name = parts[1];
                return Some((method.to_string(), model_name.to_string()));
            }
        }

        None
    }

    /// Classifies a Pydantic transformation method name into a TransformationType
    fn classify_transformation(&self, method: &str) -> Option<crate::models::TransformationType> {
        use crate::models::TransformationType::*;

        match method {
            // Constructors / validators from various inputs
            "model_validate" | "parse_obj" => Some(ValidateData),
            "model_validate_json" | "parse_raw" => Some(ValidateJson),
            "model_validate_dict" => Some(FromDict),
            "from_orm" | "from_attributes" => Some(FromOrm),

            // Dumps / serialization
            "model_dump" | "dict" => Some(ToDict),
            "model_dump_json" | "json" => Some(ToJson),
            "model_serialize" => Some(Serialize),

            _ => None,
        }
    }

    /// Processes a Pydantic transformation call:
    /// - determines source/target schemas
    /// - creates a DataFlow edge with concrete TransformationType
    fn process_pydantic_transformation(
        &mut self,
        caller: NodeId,
        call: &Call,
        current_file: &Path,
        transform_info: (String, String),
    ) -> Result<NodeId> {
        let (method, model_name) = transform_info;

        // Map method -> high‑level transformation type
        let Some(transformation_type) = self.classify_transformation(&method) else {
            // If for какой‑то метод нет явной классификации — считаем обычным вызовом
            if self.verbose {
                debug!(
                    method = %method,
                    "Pydantic method not classified as transformation, using regular call"
                );
            }
            return self.process_call(caller, call, current_file);
        };

        // Try to find the Pydantic model schema by name
        let to_schema = self
            .pydantic_models
            .get(&model_name)
            .cloned()
            .unwrap_or_else(|| SchemaReference {
                name: model_name.clone(),
                schema_type: SchemaType::Pydantic,
                location: call.location.clone(),
                metadata: HashMap::new(),
            });

        // For now we model "from" side heuristically:
        // - constructors/validators: data (dict/json/ORM) → Pydantic
        // - dumps/serialization: Pydantic → data (dict/json)
        let from_schema = match transformation_type {
            crate::models::TransformationType::FromDict
            | crate::models::TransformationType::ValidateData => SchemaReference {
                name: "Dict".to_string(),
                schema_type: SchemaType::JsonSchema,
                location: call.location.clone(),
                metadata: HashMap::new(),
            },
            crate::models::TransformationType::FromJson
            | crate::models::TransformationType::ValidateJson => SchemaReference {
                name: "Json".to_string(),
                schema_type: SchemaType::JsonSchema,
                location: call.location.clone(),
                metadata: HashMap::new(),
            },
            crate::models::TransformationType::FromOrm
            | crate::models::TransformationType::FromAttributes
            | crate::models::TransformationType::OrmToPydantic => SchemaReference {
                name: "OrmModel".to_string(),
                schema_type: SchemaType::OrmModel,
                location: call.location.clone(),
                metadata: HashMap::new(),
            },
            crate::models::TransformationType::ToDict => to_schema.clone(),
            crate::models::TransformationType::ToJson
            | crate::models::TransformationType::Serialize
            | crate::models::TransformationType::PydanticToOrm => to_schema.clone(),
        };

        let (from_schema, to_schema) = match transformation_type {
            // Data → Pydantic
            crate::models::TransformationType::FromDict
            | crate::models::TransformationType::FromJson
            | crate::models::TransformationType::FromOrm
            | crate::models::TransformationType::FromAttributes
            | crate::models::TransformationType::ValidateData
            | crate::models::TransformationType::ValidateJson
            | crate::models::TransformationType::OrmToPydantic => (from_schema, to_schema),
            // Pydantic → Data / ORM
            crate::models::TransformationType::ToDict
            | crate::models::TransformationType::ToJson
            | crate::models::TransformationType::Serialize
            | crate::models::TransformationType::PydanticToOrm => (to_schema.clone(), from_schema),
        };

        // Create dedicated DataFlow edge between abstract "data" and concrete Pydantic model
        // Create self-loop edge to represent data transformation within the same node
        // This is intentional: the transformation (e.g., model_dump_json) happens
        // within the caller node, so we create an edge from caller to itself
        self.graph.add_edge(
            *caller,
            *caller,
            CallEdge::DataFlow {
                from: caller,
                to: caller,
                from_schema,
                to_schema: Box::new(to_schema),
                location: call.location.clone(),
                transformation: Some(transformation_type.clone()),
            },
        );

        if self.verbose {
            debug!(
                model_name = %model_name,
                method = %method,
                transformation_type = ?transformation_type,
                "Pydantic transformation classified"
            );
        }

        Ok(caller)
    }

    /// Recursively checks if a class is a Pydantic model
    /// Checks direct inheritance from BaseModel, known Pydantic models in cache,
    /// and base classes in the current file (recursively)
    /// Uses visited set to prevent infinite recursion in case of circular inheritance
    fn is_pydantic_model_recursive(
        &self,
        class_def: &ast::StmtClassDef,
        file_ast: Option<&ast::Mod>,
        file_path: &Path,
    ) -> bool {
        self.is_pydantic_model_recursive_impl(class_def, file_ast, file_path, &mut HashSet::new())
    }

    #[allow(clippy::only_used_in_recursion)]
    fn is_pydantic_model_recursive_impl(
        &self,
        class_def: &ast::StmtClassDef,
        file_ast: Option<&ast::Mod>,
        file_path: &Path,
        visited: &mut HashSet<String>,
    ) -> bool {
        let class_name = class_def.name.to_string();

        // Prevent infinite recursion in case of circular inheritance
        if visited.contains(&class_name) {
            return false;
        }
        visited.insert(class_name);

        // Check direct inheritance from BaseModel
        let has_base_model = class_def.bases.iter().any(|base| {
            let base_str = self.parser.expr_to_string(base);
            let last_segment = base_str
                .rsplit('.')
                .next()
                .or_else(|| base_str.rsplit("::").next())
                .unwrap_or(&base_str);
            last_segment == "BaseModel" || base_str == "pydantic.BaseModel"
        });

        if has_base_model {
            return true;
        }

        // Check if any base is already known Pydantic model (from cache)
        for base in &class_def.bases {
            let base_str = self.parser.expr_to_string(base);
            let simple_name = base_str
                .rsplit('.')
                .next()
                .or_else(|| base_str.rsplit("::").next())
                .unwrap_or(&base_str);

            if self.pydantic_models.contains_key(simple_name) {
                return true;
            }
        }

        // Recursively check base classes in current file
        if let Some(ast::Mod::Module(module)) = file_ast {
            for base in &class_def.bases {
                let base_str = self.parser.expr_to_string(base);
                let simple_name = base_str
                    .rsplit('.')
                    .next()
                    .or_else(|| base_str.rsplit("::").next())
                    .unwrap_or(&base_str);

                // Find base class in current file
                for stmt in &module.body {
                    if let ast::Stmt::ClassDef(base_class_def) = stmt {
                        if base_class_def.name.as_str() == simple_name {
                            // Recursively check if base class is Pydantic
                            if self.is_pydantic_model_recursive_impl(
                                base_class_def,
                                file_ast,
                                file_path,
                                visited,
                            ) {
                                return true;
                            }
                            break;
                        }
                    }
                }
            }
        }

        false
    }

    /// Extracts column type from SQLAlchemy annotation
    /// Examples:
    /// - Mapped[int] -> ("int", false)
    /// - Column(Integer) -> ("Integer", false)
    /// - Mapped[Optional[str]] -> ("str", true)
    fn extract_column_type(&self, annotation_str: &str) -> (String, bool) {
        // Handle Mapped[T] syntax (SQLAlchemy 2.0+)
        if annotation_str.contains("Mapped[") {
            if let Some(start) = annotation_str.find('[') {
                if let Some(end) = annotation_str.rfind(']') {
                    let inner = &annotation_str[start + 1..end];
                    // Check for Optional[T]
                    if inner.trim_start().starts_with("Optional[") {
                        let type_start = inner.find('[').unwrap_or(0) + 1;
                        let type_end = inner.rfind(']').unwrap_or(inner.len());
                        let base_type = &inner[type_start..type_end];
                        return (base_type.trim().to_string(), true);
                    }
                    return (inner.trim().to_string(), false);
                }
            }
        }

        // Handle Column(Type) syntax (SQLAlchemy 1.x)
        if annotation_str.contains("Column(") {
            // Extract type from Column(Integer), Column(String), etc.
            if let Some(start) = annotation_str.find('(') {
                if let Some(end) = annotation_str.rfind(')') {
                    let inner = &annotation_str[start + 1..end];
                    // Try to extract type name
                    let type_name = inner.split(',').next().unwrap_or(inner).trim();
                    // Check for nullable parameter
                    let nullable = inner.to_lowercase().contains("nullable=true");
                    return (type_name.to_string(), nullable);
                }
            }
        }

        // Fallback: try to extract common types
        let type_lower = annotation_str.to_lowercase();
        if type_lower.contains("integer") || type_lower.contains("int") {
            ("Integer".to_string(), false)
        } else if type_lower.contains("string") || type_lower.contains("str") {
            ("String".to_string(), false)
        } else if type_lower.contains("boolean") || type_lower.contains("bool") {
            ("Boolean".to_string(), false)
        } else if type_lower.contains("float") {
            ("Float".to_string(), false)
        } else {
            ("Unknown".to_string(), false)
        }
    }

    /// Extracts and caches ORM model from a class definition
    fn extract_and_cache_orm_model(
        &mut self,
        class_def: &ast::StmtClassDef,
        file_path: &Path,
        converter: &LocationConverter,
    ) -> Result<Option<SchemaReference>> {
        let class_name = class_def.name.to_string();

        // Read AST to check if this is a SQLAlchemy model
        let file_ast = if let Ok(source) = std::fs::read_to_string(file_path) {
            rustpython_parser::parse(
                &source,
                rustpython_parser::Mode::Module,
                file_path.to_string_lossy().as_ref(),
            )
            .ok()
        } else {
            None
        };

        let is_orm = if let Some(ast) = &file_ast {
            Self::is_sqlalchemy_model(ast, &class_name)
        } else {
            false
        };

        if !is_orm {
            return Ok(None);
        }

        let range = class_def.range();
        let (line, column) = converter.byte_offset_to_location(range.start().into());

        // Extract ORM fields
        let orm_fields = self.extract_sqlalchemy_fields(class_def);

        // Create SchemaReference
        let mut metadata = HashMap::new();
        metadata.insert("orm_type".to_string(), "sqlalchemy".to_string());

        // Store fields as JSON
        if let Ok(fields_json) = serde_json::to_string(&orm_fields) {
            metadata.insert("fields".to_string(), fields_json);
        }

        let schema_ref = SchemaReference {
            name: class_name.clone(),
            schema_type: SchemaType::OrmModel,
            location: Location {
                file: file_path.to_string_lossy().to_string(),
                line,
                column: Some(column),
            },
            metadata,
        };

        // Add to cache
        self.orm_models
            .insert(class_name.clone(), schema_ref.clone());

        Ok(Some(schema_ref))
    }

    /// Extracts fields from a SQLAlchemy model
    fn extract_sqlalchemy_fields(
        &self,
        class_def: &ast::StmtClassDef,
    ) -> Vec<crate::models::SQLAlchemyField> {
        let mut fields = Vec::new();

        for body_stmt in &class_def.body {
            if let ast::Stmt::AnnAssign(ann_assign) = body_stmt {
                if let ast::Expr::Name(name) = ann_assign.target.as_ref() {
                    let field_name = name.id.to_string();
                    let annotation_str = self.parser.expr_to_string(ann_assign.annotation.as_ref());

                    // Check if annotation contains Column or Mapped
                    if annotation_str.contains("Column") || annotation_str.contains("Mapped") {
                        // Skip special fields like __tablename__
                        if field_name.starts_with("__") {
                            continue;
                        }
                        let (type_name, nullable) = self.extract_column_type(&annotation_str);
                        fields.push(crate::models::SQLAlchemyField {
                            name: field_name,
                            type_name,
                            nullable,
                        });
                    }
                }
            }
        }

        fields
    }

    /// Checks if SQLAlchemy type is compatible with Pydantic type
    fn types_compatible(&self, sql_type: &str, pydantic_type: &str) -> bool {
        // Normalize types to lowercase for comparison
        let sql_lower = sql_type.to_lowercase();
        let pydantic_lower = pydantic_type.to_lowercase();

        // Direct matches
        if sql_lower == pydantic_lower {
            return true;
        }

        // Type mappings
        match (sql_lower.as_str(), pydantic_lower.as_str()) {
            // Integer types
            ("integer", "int") | ("int", "integer") => true,

            // String types
            ("string", "str") | ("str", "string") => true,
            ("text", "str") | ("str", "text") => true,

            // Boolean types
            ("boolean", "bool") | ("bool", "boolean") => true,

            // Numeric types
            ("float", "float") => true,
            ("numeric", "float") | ("float", "numeric") => true,
            ("decimal", "float") | ("float", "decimal") => true,

            // UUID types
            (a, b) if a.contains("uuid") && b.contains("uuid") => true,

            // Date/Time types
            (a, b)
                if (a.contains("date") || a.contains("time"))
                    && (b.contains("date") || b.contains("time")) =>
            {
                true
            }

            // JSON types
            (a, b)
                if (a.contains("json") || a == "dict")
                    && (b.contains("json") || b == "dict" || b == "object") =>
            {
                true
            }

            _ => false,
        }
    }

    /// Matches Pydantic fields with SQLAlchemy fields and returns match percentage
    fn match_fields(
        &self,
        pydantic_fields: &[crate::models::PydanticFieldInfo],
        sqlalchemy_fields: &[crate::models::SQLAlchemyField],
    ) -> f64 {
        if pydantic_fields.is_empty() || sqlalchemy_fields.is_empty() {
            return 0.0;
        }

        let mut matches = 0;
        let total = pydantic_fields.len().max(sqlalchemy_fields.len());

        for pydantic_field in pydantic_fields {
            if let Some(sql_field) = sqlalchemy_fields
                .iter()
                .find(|sa_field| sa_field.name == pydantic_field.name)
            {
                // Check type compatibility
                if self.types_compatible(&sql_field.type_name, &pydantic_field.type_name) {
                    // Also check optionality compatibility
                    // SQLAlchemy nullable=true should match Pydantic optional=true
                    if sql_field.nullable == pydantic_field.optional {
                        matches += 1;
                    }
                    // Note: Partial match (types match but optionality differs) is not counted
                    // This can be adjusted in the future if needed
                }
            }
        }

        matches as f64 / total as f64
    }

    /// Finds a class node by name in the graph
    fn find_class_node_by_name(&self, class_name: &str) -> Option<NodeId> {
        self.graph.node_indices().find_map(|idx| {
            if let Some(CallNode::Class { name, .. }) = self.graph.node_weight(idx) {
                if name == class_name {
                    return Some(NodeId::from(idx));
                }
            }
            None
        })
    }
}
