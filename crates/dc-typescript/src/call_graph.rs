use anyhow::{Context, Result};
use dc_core::call_graph::{CallEdge, CallGraph, CallNode};
use dc_core::models::NodeId;
use dc_core::parsers::TypeScriptParser;
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
        }
    }

    /// Sets the maximum recursion depth
    pub fn with_max_depth(mut self, max_depth: Option<usize>) -> Self {
        self.max_depth = max_depth;
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
            let (module, _source, converter) = self
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
                .extract_calls(&module, &file_path_str, &converter);
            for call in calls {
                if let Err(err) = self.process_call(module_node, &call, &normalized) {
                    eprintln!(
                        "Error processing call '{}' from {:?}: {}",
                        call.name, normalized, err
                    );
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
