use anyhow::{Context, Result};
use rustpython_parser::ast::Ranged;
use rustpython_parser::{ast, parse, Mode};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use crate::call_graph::decorator::Decorator;
use crate::call_graph::{CallEdge, CallGraph, CallNode, HttpMethod, Parameter};
use crate::models::{BaseType, NodeId, TypeInfo};
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
    /// Project root
    project_root: Option<PathBuf>,
    /// Maximum recursion depth (None = unlimited)
    max_depth: Option<usize>,
    /// Current recursion depth
    current_depth: usize,
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
            project_root: None,
            max_depth: None,
            current_depth: 0,
        }
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
            Ok(path) => path,
            Err(err) => {
                eprintln!(
                    "Failed to resolve import '{}' from {:?}: {}",
                    import.path, current_file, err
                );
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
        let _ = self.build_from_entry(&import_path);

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
            return Ok(());
        }

        let handler_name = match &decorator.target_function {
            Some(name) => name,
            None => return Ok(()),
        };

        let Some(handler_node) = self.find_function_node(handler_name, current_file) else {
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

        let route_node = NodeId::from(self.graph.add_node(CallNode::Route {
            path: route_path,
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
        for import in imports {
            if let Err(err) = self.process_import(module_node, &import, file_path) {
                eprintln!(
                    "Failed to process import {} in {:?}: {}",
                    import.path, file_path, err
                );
            }
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
        let parameters = self.convert_parameters(&func_def.args);

        // Get location from AST
        let range = func_def.range();
        let (line, _column) = converter.byte_offset_to_location(range.start().into());

        let node_id = NodeId::from(self.graph.add_node(CallNode::Function {
            name: func_def.name.to_string(),
            file: file_path.to_path_buf(),
            line,
            parameters,
            return_type: None,
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
        let parameters = self.convert_parameters(&func_def.args);

        // Get location from AST
        let range = func_def.range();
        let (line, _column) = converter.byte_offset_to_location(range.start().into());

        let node_id = NodeId::from(self.graph.add_node(CallNode::Function {
            name: func_def.name.to_string(),
            file: file_path.to_path_buf(),
            line,
            parameters,
            return_type: None,
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
        _converter: &LocationConverter,
    ) -> Result<NodeId> {
        let mut parameters = self.convert_parameters(&func_def.args);
        // Check decorators before removing the first parameter
        let has_staticmethod = self.has_decorator(&func_def.decorator_list, "staticmethod");
        if !has_staticmethod && !parameters.is_empty() {
            // If there's no @staticmethod, remove the first parameter (self or cls)
            // For @classmethod we can remove cls, for regular methods - self
            parameters.remove(0);
        }

        let node_id = NodeId::from(self.graph.add_node(CallNode::Method {
            name: func_def.name.to_string(),
            class: class_node,
            parameters,
            return_type: None,
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
        _converter: &LocationConverter,
    ) -> Result<NodeId> {
        let mut parameters = self.convert_parameters(&func_def.args);
        // Check decorators before removing the first parameter
        let has_staticmethod = self.has_decorator(&func_def.decorator_list, "staticmethod");
        if !has_staticmethod && !parameters.is_empty() {
            // If there's no @staticmethod, remove the first parameter (self or cls)
            parameters.remove(0);
        }

        let node_id = NodeId::from(self.graph.add_node(CallNode::Method {
            name: func_def.name.to_string(),
            class: class_node,
            parameters,
            return_type: None,
        }));

        let key = Self::function_key(file_path, &format!("{}.{}", class_name, func_def.name));
        self.function_nodes.insert(key, node_id);

        Ok(node_id)
    }

    fn add_class_node(
        &mut self,
        class_def: &ast::StmtClassDef,
        file_path: &Path,
        _converter: &LocationConverter,
    ) -> Result<NodeId> {
        let node_id = NodeId::from(self.graph.add_node(CallNode::Class {
            name: class_def.name.to_string(),
            file: file_path.to_path_buf(),
            methods: Vec::new(),
        }));

        let key = Self::function_key(file_path, &class_def.name);
        self.function_nodes.insert(key, node_id);

        Ok(node_id)
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
        for decorator in decorators {
            if let Err(err) = self.process_decorator(&decorator, file_path) {
                eprintln!(
                    "Failed to process decorator {} in {:?}: {}",
                    decorator.name, file_path, err
                );
            }
        }
        Ok(())
    }

    fn convert_parameters(&self, args: &ast::Arguments) -> Vec<Parameter> {
        let mut params = Vec::new();

        // posonlyargs, args, kwonlyargs are Vec<ArgWithDefault>
        // default is already stored inside each ArgWithDefault

        // Process posonlyargs
        for arg in &args.posonlyargs {
            params.push(self.create_parameter_from_arg_with_default(arg));
        }

        // Process args
        for arg in &args.args {
            params.push(self.create_parameter_from_arg_with_default(arg));
        }

        // Process kwonlyargs
        for arg in &args.kwonlyargs {
            params.push(self.create_parameter_from_arg_with_default(arg));
        }

        if let Some(arg) = &args.vararg {
            // vararg is Option<Box<Arg>>, without default
            params.push(self.create_parameter_from_arg(arg, None));
        }
        if let Some(arg) = &args.kwarg {
            // kwarg is Option<Box<Arg>>, without default
            params.push(self.create_parameter_from_arg(arg, None));
        }
        params
    }

    /// Creates a parameter from ArgWithDefault (with default)
    fn create_parameter_from_arg_with_default(&self, arg: &ast::ArgWithDefault) -> Parameter {
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

        Parameter {
            name: arg.def.arg.to_string(),
            type_info: TypeInfo {
                base_type: BaseType::Unknown,
                schema_ref: None,
                constraints: Vec::new(),
                optional,
            },
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

        Parameter {
            name: arg.arg.to_string(),
            type_info: TypeInfo {
                base_type: BaseType::Unknown,
                schema_ref: None,
                constraints: Vec::new(),
                optional,
            },
            optional,
            default_value,
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
        if let Some(node) = self.function_nodes.get(&direct_key) {
            return Some(*node);
        }

        // Find all matches by ends_with("::name")
        let matches: Vec<_> = self
            .function_nodes
            .iter()
            .filter(|(key, _)| key.ends_with(&format!("::{}", name)))
            .collect();

        if matches.is_empty() {
            return crate::call_graph::find_node_by_name(&self.graph, name);
        }

        if matches.len() == 1 {
            return Some(*matches[0].1);
        }

        // Disambiguation: find the best match
        // 1. Prefer exact module path match
        let current_dir = normalized.parent().map(|p| p.to_path_buf());
        if let Some(dir) = current_dir {
            if let Some((_, node)) = matches.iter().find(|(key, _)| {
                if let Some(key_path) = Self::extract_path_from_key(key) {
                    key_path.parent() == Some(&dir)
                } else {
                    false
                }
            }) {
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

        if let Some((_, node)) = best_match {
            // Log warning about ambiguity
            eprintln!(
                "Warning: Ambiguous function name '{}' found {} matches, selected one",
                name,
                matches.len()
            );
            return Some(**node);
        }

        // 3. Fallback: select first deterministically (sorted by key)
        let mut sorted_matches = matches.clone();
        sorted_matches.sort_by(|(key_a, _), (key_b, _)| key_a.cmp(key_b));
        sorted_matches.first().map(|(_, node)| **node)
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

        let candidate = if import_path.starts_with('.') {
            self.resolve_relative_import(import_path, &base_dir)
        } else {
            self.resolve_absolute_import(import_path)
        };

        if candidate.exists() {
            return Ok(candidate);
        }

        // Try adding .py extension if file not found
        if candidate.extension().is_none() {
            let mut with_ext = candidate.clone();
            with_ext.set_extension("py");
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

    fn normalize_path(path: &Path) -> PathBuf {
        path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
    }

    fn is_route_decorator(&self, name: &str) -> bool {
        name.starts_with("app.") || name.starts_with("router.") || name.contains(".route")
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
