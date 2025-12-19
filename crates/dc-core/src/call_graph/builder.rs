use anyhow::{Context, Result};
use rustpython_parser::ast::Ranged;
use rustpython_parser::{ast, parse, Mode};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

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
        Self::with_parser(PythonParser::new())
    }

    /// Creates a call graph builder with a custom parser (for tests)
    pub fn with_parser(parser: PythonParser) -> Self {
        Self {
            graph: CallGraph::new(),
            entry_points: Vec::new(),
            processed_files: HashSet::new(),
            parser,
            module_nodes: HashMap::new(),
            function_nodes: HashMap::new(),
            pydantic_models: HashMap::new(),
            schema_extractor: None,
            project_root: None,
            max_depth: None,
            current_depth: 0,
            verbose: false,
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

    /// Processes an import: adds a node and an edge
    pub fn process_import(
        &mut self,
        from: NodeId,
        import: &Import,
        current_file: &Path,
    ) -> Result<NodeId> {
        let import_path = match self.resolve_import_path(&import.path, current_file) {
            Ok(path) => {
                if self.verbose {
                    eprintln!("[DEBUG] Resolved import '{}' -> {:?}", import.path, path);
                }
                path
            }
            Err(err) => {
                if self.verbose {
                    eprintln!(
                        "[DEBUG] Failed to resolve import '{}' from {:?}: {}",
                        import.path, current_file, err
                    );
                }
                return Ok(from);
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

        // Recursively build graph for the imported module
        if self.verbose {
            eprintln!(
                "[DEBUG] Recursively building graph for imported module {:?}",
                import_path
            );
        }
        let _ = self.build_from_entry(&import_path);

        // Extract and cache Pydantic models from imported file
        if let Err(err) = self.extract_and_cache_pydantic_models(&import_path) {
            if self.verbose {
                eprintln!(
                    "[DEBUG] Failed to extract Pydantic models from {:?}: {}",
                    import_path, err
                );
            }
        }

        // If import has specific names (e.g., "from api.routers import auth"),
        // also try to resolve and process those submodules
        if !import.names.is_empty() {
            let import_dir = if import_path.is_dir() {
                &import_path
            } else {
                import_path.parent().unwrap_or_else(|| Path::new("."))
            };

            for name in &import.names {
                // Try to find submodule: api/routers/auth.py
                let submodule_candidates = vec![
                    import_dir.join(format!("{}.py", name)),
                    import_dir.join(name).join("__init__.py"),
                ];

                for candidate in submodule_candidates {
                    if candidate.exists() {
                        if self.verbose {
                            eprintln!(
                                "[DEBUG] Found submodule '{}' from import '{}': {:?}",
                                name, import.path, candidate
                            );
                        }
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
            if self.verbose {
                eprintln!(
                    "[DEBUG] Decorator '{}' is not a route decorator (file: {:?})",
                    decorator.name, current_file
                );
            }
            return Ok(());
        }

        let handler_name = match &decorator.target_function {
            Some(name) => name,
            None => {
                if self.verbose {
                    eprintln!(
                        "[DEBUG] Route decorator '{}' has no target function (file: {:?})",
                        decorator.name, current_file
                    );
                }
                return Ok(());
            }
        };

        if self.verbose {
            eprintln!(
                "[DEBUG] Processing route decorator '{}' -> handler '{}' (file: {:?})",
                decorator.name, handler_name, current_file
            );
        }

        // Try to find handler node - handle qualified names (ClassName.method)
        let handler_node = if handler_name.contains('.') {
            // Try qualified name first, then simple name
            let parts: Vec<&str> = handler_name.split('.').collect();
            let simple_name = parts.last().copied().unwrap_or(handler_name);

            if self.verbose {
                eprintln!(
                    "[DEBUG] Handler name '{}' contains '.', trying qualified then simple name '{}'",
                    handler_name, simple_name
                );
            }

            // First try qualified name as-is
            if let Some(node) = self.find_function_node(handler_name, current_file) {
                if self.verbose {
                    eprintln!(
                        "[DEBUG] Found handler using qualified name '{}'",
                        handler_name
                    );
                }
                Some(node)
            } else {
                // Fall back to simple name
                if self.verbose {
                    eprintln!(
                        "[DEBUG] Qualified name failed, trying simple name '{}'",
                        simple_name
                    );
                }
                self.find_function_node(simple_name, current_file)
            }
        } else {
            self.find_function_node(handler_name, current_file)
        };

        let Some(handler_node) = handler_node else {
            if self.verbose {
                eprintln!(
                    "[DEBUG] Failed to find function node for handler '{}' in file {:?}",
                    handler_name, current_file
                );
            }
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

        // Check for response_model in decorator keyword arguments
        let response_model_type = decorator
            .keyword_arguments
            .get("response_model")
            .map(|s| s.trim().to_string());

        // Try to resolve response_model from imports if not found in cache
        if let Some(ref response_model_str) = response_model_type {
            // Extract base model name (handle generic types like Page[ItemRead] -> ItemRead)
            let base_model_name = self.parser.extract_base_model_from_response_model(response_model_str);
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
                        eprintln!(
                            "[DEBUG] Failed to resolve schema '{}' from imports in {:?}: {}",
                            response_model_name, current_file, err
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
                            if self.verbose {
                                eprintln!(
                                    "[DEBUG] Failed to enrich schema for {}: {}",
                                    response_model_name, err
                                );
                            }
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
                let base_model_name = self.parser.extract_base_model_from_response_model(response_model_str);
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

        let route_node = NodeId::from(self.graph.add_node(CallNode::Route {
            path: route_path.clone(),
            method: http_method,
            handler: handler_node,
            location: location.clone(),
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

        if self.verbose {
            eprintln!(
                "[DEBUG] Created route node: {} {} -> handler node {:?} (file: {:?})",
                format!("{:?}", http_method).to_uppercase(),
                route_path,
                handler_node.0.index(),
                current_file
            );
        }

        Ok(())
    }

    /// Gets the built graph
    pub fn into_graph(self) -> CallGraph {
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
                eprintln!(
                    "Failed to process import {} in {:?}: {}",
                    import.path, file_path, err
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
        // Check if any base class is BaseModel
        let is_pydantic = class_def.bases.iter().any(|base| {
            let base_str = self.parser.expr_to_string(base);
            // Use rsplit to get the last segment efficiently
            let last_segment = base_str
                .rsplit('.')
                .next()
                .or_else(|| base_str.rsplit("::").next())
                .unwrap_or(&base_str);
            last_segment == "BaseModel" || base_str == "pydantic.BaseModel"
        });

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
                                if self.verbose {
                                    eprintln!(
                                        "[DEBUG] Failed to enrich schema for {}: {}",
                                        class_name, err
                                    );
                                }
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
                    if self.verbose {
                        eprintln!(
                            "[DEBUG] Failed to enrich schema for {}: {}",
                            model.name, err
                        );
                    }
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
                    eprintln!(
                        "Failed to process call {} in {:?}: {}",
                        call.name, file_path, err
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

        if self.verbose {
            eprintln!(
                "[DEBUG] Processing {} decorators in file {:?}",
                decorators.len(),
                file_path
            );
        }

        for decorator in decorators {
            if self.verbose {
                eprintln!(
                    "[DEBUG] Extracted decorator '{}' -> target: {:?} (file: {:?}, line: {})",
                    decorator.name,
                    decorator.target_function,
                    decorator.location.file,
                    decorator.location.line
                );
            }

            if let Err(err) = self.process_decorator(&decorator, file_path) {
                if self.verbose {
                    eprintln!(
                        "[DEBUG] Failed to process decorator {} in {:?}: {}",
                        decorator.name, file_path, err
                    );
                }
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

    /// Resolves a type annotation to TypeInfo, checking if it's a Pydantic model
    fn resolve_type_annotation(
        &self,
        annotation: &ast::Expr,
        _file_path: &Path,
        _line: usize,
    ) -> TypeInfo {
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
                schema_type: SchemaType::JsonSchema,
                location: Location {
                    file: _file_path.to_string_lossy().to_string(),
                    line: _line,
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

        if self.verbose {
            eprintln!(
                "[DEBUG] Searching for function '{}' in file {:?} (direct key: {})",
                name, current_file, direct_key
            );
        }

        if let Some(node) = self.function_nodes.get(&direct_key) {
            if self.verbose {
                eprintln!(
                    "[DEBUG] Found direct match for '{}' -> node {:?}",
                    name,
                    node.0.index()
                );
            }
            return Some(*node);
        }

        // Find all matches by ends_with("::name")
        let matches: Vec<_> = self
            .function_nodes
            .iter()
            .filter(|(key, _)| key.ends_with(&format!("::{}", name)))
            .collect();

        if self.verbose {
            eprintln!(
                "[DEBUG] Found {} suffix matches for '{}'",
                matches.len(),
                name
            );
        }

        if matches.is_empty() {
            if self.verbose {
                eprintln!(
                    "[DEBUG] No suffix matches, trying graph search for '{}'",
                    name
                );
            }
            return crate::call_graph::find_node_by_name(&self.graph, name);
        }

        if matches.len() == 1 {
            if self.verbose {
                eprintln!(
                    "[DEBUG] Single match found for '{}' -> node {:?} (key: {})",
                    name,
                    matches[0].1 .0.index(),
                    matches[0].0
                );
            }
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
                if self.verbose {
                    eprintln!(
                        "[DEBUG] Selected exact path match for '{}' -> node {:?} (key: {})",
                        name,
                        node.0.index(),
                        key
                    );
                }
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
            if self.verbose {
                eprintln!(
                    "[DEBUG] Warning: Ambiguous function name '{}' found {} matches, selected best match -> node {:?} (key: {})",
                    name,
                    matches.len(),
                    node.0.index(),
                    key
                );
            }
            return Some(**node);
        }

        // 3. Fallback: select first deterministically (sorted by key)
        let mut sorted_matches = matches.clone();
        sorted_matches.sort_by(|(key_a, _), (key_b, _)| key_a.cmp(key_b));
        if let Some((key, node)) = sorted_matches.first() {
            if self.verbose {
                eprintln!(
                    "[DEBUG] Selected first match (sorted) for '{}' -> node {:?} (key: {})",
                    name,
                    node.0.index(),
                    key
                );
            }
            Some(**node)
        } else {
            if self.verbose {
                eprintln!("[DEBUG] No match found for '{}' after all attempts", name);
            }
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
                
                // Look for "app" directory in parent directories
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
                    if import_path.starts_with("app.") {
                        let remaining = &import_path[4..]; // Skip "app."
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
                eprintln!(
                    "[DEBUG] Trying to resolve '{}': absolute={:?}, relative={:?}, app_dir={:?}",
                    import_path, absolute_candidate, relative_candidate, app_dir_candidate
                );
            }

            // Try app directory first (for imports like "app.schemas")
            if let Some(ref app_candidate) = app_dir_candidate {
                if app_candidate.exists() {
                    if self.verbose {
                        eprintln!(
                            "[DEBUG] Found '{}' via app directory resolution: {:?}",
                            import_path, app_candidate
                        );
                    }
                    return Ok(app_candidate.clone());
                }
            }

            // Try relative first (more common for same-package imports)
            if relative_candidate.exists() {
                if self.verbose {
                    eprintln!(
                        "[DEBUG] Found '{}' via relative resolution: {:?}",
                        import_path, relative_candidate
                    );
                }
                relative_candidate
            } else if absolute_candidate.exists() {
                if self.verbose {
                    eprintln!(
                        "[DEBUG] Found '{}' via absolute resolution: {:?}",
                        import_path, absolute_candidate
                    );
                }
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
                if self.verbose {
                    eprintln!(
                        "[DEBUG] Found '{}' with .py extension: {:?}",
                        import_path, with_ext
                    );
                }
                return Ok(with_ext);
            }
        }

        if self.verbose {
            eprintln!(
                "[DEBUG] Cannot resolve import path '{}' from {:?} (tried: {:?})",
                import_path, current_file, candidate
            );
        }

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

        // Check if we have import information for this file
        let Some(imports_map) = self.file_imports.get(&normalized_file) else {
            return Ok(()); // No imports recorded for this file
        };

        // Find the module path for this schema name
        let Some(module_path_str) = imports_map.get(schema_name) else {
            return Ok(()); // Schema not found in imports
        };

        // Resolve the module path to a file path
        let module_file = self.resolve_import_path(module_path_str, current_file)?;

        // Extract and cache Pydantic models from the imported module
        // This will populate pydantic_models cache with the schema
        self.extract_and_cache_pydantic_models(&module_file)?;

        // Check if the schema is now in cache
        if self.pydantic_models.contains_key(schema_name) && self.verbose {
            eprintln!(
                "[DEBUG] Successfully resolved schema '{}' from module {:?}",
                schema_name, module_file
            );
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
}
