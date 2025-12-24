use anyhow::Result;
use rustpython_parser::ast;
use rustpython_parser::ast::Ranged;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use tracing::warn;

use crate::call_graph::CallNode;
use crate::models::{ImportError, Location};

/// Determines if the given module name represents an external dependency
/// by inspecting requirements.txt and pyproject.toml in the project root.
fn is_external_dependency(module_name: &str, project_root: &Path) -> bool {
    // 1. Check requirements.txt
    let requirements = project_root.join("requirements.txt");
    if requirements.is_file() {
        if let Ok(content) = std::fs::read_to_string(&requirements) {
            for line in content.lines() {
                let line = line.trim_start();
                if line.is_empty() || line.starts_with('#') {
                    continue;
                }
                if line.starts_with(module_name) || line.starts_with(&format!("{}-", module_name)) {
                    return true;
                }
            }
        }
    }

    // 2. Check pyproject.toml (heuristic search)
    let pyproject = project_root.join("pyproject.toml");
    if pyproject.is_file() {
        if let Ok(content) = std::fs::read_to_string(&pyproject) {
            if content.contains(module_name) {
                return true;
            }
        }
    }

    false
}

/// Tries to find Python virtual environment directory near project root.
/// Looks for .venv, venv, env and also respects VIRTUAL_ENV env variable.
fn find_venv_path(project_root: &Path) -> Option<PathBuf> {
    let mut candidates: Vec<PathBuf> = vec![
        project_root.join(".venv"),
        project_root.join("venv"),
        project_root.join("env"),
    ];

    if let Ok(venv_env) = std::env::var("VIRTUAL_ENV") {
        candidates.push(PathBuf::from(venv_env));
    }

    for candidate in candidates {
        if candidate.is_dir() {
            let lib_dir = candidate.join("lib");
            let entries = std::fs::read_dir(&lib_dir).ok()?;
            for entry in entries.flatten() {
                let path = entry.path();
                let is_python_dir = path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .map(|s| s.starts_with("python"))
                    .unwrap_or(false);
                if !is_python_dir {
                    continue;
                }
                let site_packages = path.join("site-packages");
                if site_packages.is_dir() {
                    return Some(candidate.clone());
                }
            }
        }
    }

    None
}

/// Tries to locate a module inside virtualenv's site-packages.
/// Supports both package directories (module/__init__.py) and single files (module.py).
fn find_module_in_venv(module_name: &str, venv_path: &Path) -> Option<PathBuf> {
    let lib_dir = venv_path.join("lib");
    let entries = std::fs::read_dir(&lib_dir).ok()?;

    for entry in entries.flatten() {
        let python_dir = entry.path();
        let is_python_dir = python_dir
            .file_name()
            .and_then(|n| n.to_str())
            .map(|s| s.starts_with("python"))
            .unwrap_or(false);
        if !is_python_dir {
            continue;
        }

        let site_packages = python_dir.join("site-packages");
        if !site_packages.is_dir() {
            continue;
        }

        let pkg_dir = site_packages.join(module_name);
        if pkg_dir.join("__init__.py").is_file() {
            return Some(pkg_dir);
        }

        let module_file = site_packages.join(format!("{}.py", module_name));
        if module_file.is_file() {
            return Some(module_file);
        }
    }

    None
}
use crate::parsers::{Call, CallArgument, Import, LocationConverter};

/// Python code parser with call analysis
pub struct PythonParser {
    /// Cache for resolved imports: (module, normalized project_root) -> resolved path or None
    import_cache: HashMap<(String, Option<String>), Option<PathBuf>>,
}

impl PythonParser {
    /// Creates a new parser
    pub fn new() -> Self {
        Self {
            import_cache: HashMap::new(),
        }
    }

    /// Parses a file and extracts call nodes
    /// Note: This method is not currently used directly. CallGraphBuilder works directly with AST.
    pub fn parse_file(&self, _path: &Path) -> Result<Vec<CallNode>> {
        Ok(Vec::new())
    }

    /// Tries to resolve import path in a safe way, taking virtual environments and external
    /// dependencies into account. Never panics; instead returns structured ImportError.
    pub fn resolve_import_safe(
        &self,
        import_path: &str,
        _current_file: &Path,
        project_root: &Path,
    ) -> Result<Option<PathBuf>, ImportError> {
        // 1. Check if this looks like an external dependency
        if !is_external_dependency(import_path, project_root) {
            // Local module that we cannot resolve here – treat as missing but non-fatal
            return Ok(None);
        }

        // 2. Try to locate a virtual environment and module in its site-packages
        if let Some(venv_path) = find_venv_path(project_root) {
            if let Some(path) = find_module_in_venv(import_path, &venv_path) {
                return Ok(Some(path));
            }
        }

        // 3. External dependency not found even in virtual environment
        Err(ImportError::ExternalDependency {
            module: import_path.to_string(),
            suggestion: format!("Install with: pip install {}", import_path),
        })
    }

    /// Cached wrapper around resolve_import_safe.
    /// Does NOT cache ImportError; only successful and None results are cached.
    pub fn resolve_import_cached(
        &mut self,
        import_path: &str,
        project_root: &Path,
    ) -> Result<Option<PathBuf>, ImportError> {
        let key = (
            import_path.to_string(),
            project_root
                .canonicalize()
                .ok()
                .and_then(|p| p.to_str().map(|s| s.to_string())),
        );

        if let Some(cached) = self.import_cache.get(&key) {
            return Ok(cached.clone());
        }

        let result = self.resolve_import_safe(import_path, Path::new(""), project_root)?;
        // Cache only successful resolution (Some) and None; errors are propagated as-is.
        self.import_cache.insert(key, result.clone());
        Ok(result)
    }

    /// Extracts imports from AST
    pub fn extract_imports(
        &self,
        ast: &ast::Mod,
        file_path: &str,
        converter: &LocationConverter,
    ) -> Vec<Import> {
        let mut imports = Vec::new();

        if let ast::Mod::Module(module) = ast {
            for stmt in &module.body {
                self.extract_imports_from_stmt(stmt, &mut imports, file_path, converter);
            }
        }

        imports
    }

    fn extract_imports_from_stmt(
        &self,
        stmt: &ast::Stmt,
        imports: &mut Vec<Import>,
        file_path: &str,
        converter: &LocationConverter,
    ) {
        match stmt {
            ast::Stmt::Import(import_stmt) => {
                let range = import_stmt.range();
                let (line, column) = converter.byte_offset_to_location(range.start().into());
                for alias in &import_stmt.names {
                    imports.push(Import {
                        path: alias.name.to_string(),
                        names: vec![],
                        location: crate::models::Location {
                            file: file_path.to_string(),
                            line,
                            column: Some(column),
                        },
                    });
                }
            }
            ast::Stmt::ImportFrom(import_from) => {
                let range = import_from.range();
                let (line, column) = converter.byte_offset_to_location(range.start().into());
                if let Some(module) = &import_from.module {
                    for alias in &import_from.names {
                        imports.push(Import {
                            path: module.to_string(),
                            names: vec![alias.name.to_string()],
                            location: crate::models::Location {
                                file: file_path.to_string(),
                                line,
                                column: Some(column),
                            },
                        });
                    }
                }
            }
            _ => {}
        }
    }

    /// Extracts function calls from AST
    pub fn extract_calls(
        &self,
        ast: &ast::Mod,
        file_path: &str,
        converter: &LocationConverter,
    ) -> Vec<Call> {
        let mut calls = Vec::new();

        if let ast::Mod::Module(module) = ast {
            let mut context = Vec::new();
            self.walk_statements(&module.body, &mut context, &mut calls, file_path, converter);
        }

        calls
    }

    /// Extracts FastAPI decorators
    pub fn extract_decorators(
        &self,
        ast: &ast::Mod,
        file_path: &str,
        converter: &LocationConverter,
    ) -> Vec<crate::call_graph::Decorator> {
        let mut decorators = Vec::new();

        if let ast::Mod::Module(module) = ast {
            for stmt in &module.body {
                self.collect_decorators(stmt, None, &mut decorators, file_path, converter);
            }
        }

        decorators
    }

    /// Extracts Pydantic models from AST
    pub fn extract_pydantic_models(
        &self,
        ast: &ast::Mod,
        file_path: &str,
        converter: &LocationConverter,
    ) -> Vec<crate::models::SchemaReference> {
        let mut models = Vec::new();

        if let ast::Mod::Module(module) = ast {
            for stmt in &module.body {
                if let ast::Stmt::ClassDef(class_def) = stmt {
                    // Check if class inherits from BaseModel
                    if self.is_pydantic_base_model(&class_def.bases) {
                        let mut metadata = std::collections::HashMap::new();

                        // Extract field information using new structured format
                        let mut fields = Vec::new();
                        let mut has_from_attributes = false;

                        // Extract fields from current class
                        for body_stmt in &class_def.body {
                            match body_stmt {
                                ast::Stmt::AnnAssign(ann_assign) => {
                                    if let Ok(field_info) = self.extract_field_info(ann_assign) {
                                        fields.push(field_info);
                                    }
                                }
                                ast::Stmt::Assign(assign_stmt) => {
                                    // Check for model_config = {"from_attributes": True, ...}
                                    if let Some(ast::Expr::Name(name)) = assign_stmt.targets.first()
                                    {
                                        if name.id.as_str() == "model_config" {
                                            if let Some(config_dict) =
                                                self.extract_model_config(&assign_stmt.value)
                                            {
                                                // Store all model_config keys in metadata
                                                for (key, value) in &config_dict {
                                                    metadata.insert(
                                                        format!("model_config.{}", key),
                                                        value.clone(),
                                                    );
                                                }

                                                // Check the value for from_attributes (for backward compatibility)
                                                if let Some(value) =
                                                    config_dict.get("from_attributes")
                                                {
                                                    // Accept True, "True", or true
                                                    if value == "True"
                                                        || value == "true"
                                                        || value == &true.to_string()
                                                    {
                                                        has_from_attributes = true;
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                                _ => {}
                            }
                        }

                        // Extract fields from base classes (recursively, within current file)
                        for base in &class_def.bases {
                            let base_name = self.extract_class_name_from_expr(base);
                            if let Some(base_model) =
                                self.find_pydantic_model_by_name(&base_name, ast)
                            {
                                // Recursively get fields from base class
                                if let Ok(base_fields) =
                                    self.extract_fields_from_class_def(base_model)
                                {
                                    fields.extend(base_fields);
                                }
                            }
                        }

                        // Store fields as JSON
                        if !fields.is_empty() {
                            match serde_json::to_string(&fields) {
                                Ok(fields_json) => {
                                    metadata.insert("fields".to_string(), fields_json);
                                }
                                Err(_) => {
                                    // Fallback: use old string format for compatibility
                                    let fields_str: Vec<String> = fields
                                        .iter()
                                        .map(|f| {
                                            let mut s = format!("{}:{}", f.name, f.type_name);
                                            if let Some(inner) = &f.inner_type {
                                                s.push_str(&format!("[{}]", inner));
                                            }
                                            if f.optional {
                                                s = format!("Optional[{}]", s);
                                            }
                                            s
                                        })
                                        .collect();
                                    metadata.insert("fields".to_string(), fields_str.join(","));
                                }
                            }
                        }

                        if has_from_attributes {
                            metadata.insert("from_attributes".to_string(), "true".to_string());
                        }

                        let range = class_def.range();
                        let (line, column) =
                            converter.byte_offset_to_location(range.start().into());
                        models.push(crate::models::SchemaReference {
                            name: class_def.name.to_string(),
                            schema_type: crate::models::SchemaType::Pydantic,
                            location: crate::models::Location {
                                file: file_path.to_string(),
                                line,
                                column: Some(column),
                            },
                            metadata,
                        });
                    }
                }
            }
        }

        models
    }

    /// Checks if the base class is Pydantic BaseModel
    fn is_pydantic_base_model(&self, bases: &[ast::Expr]) -> bool {
        for base in bases {
            let base_name = self.expr_to_string(base);
            // Extract the last segment of the path (split by '.' or '::')
            #[allow(clippy::double_ended_iterator_last)]
            let last_segment = base_name
                .split('.')
                .last()
                .or_else(|| base_name.split("::").last())
                .unwrap_or_else(|| {
                    warn!(
                        base_name = %base_name,
                        "Failed to extract module name from import path, using base_name"
                    );
                    &base_name
                });

            // Check exact match
            if last_segment == "BaseModel" || base_name == "pydantic.BaseModel" {
                return true;
            }
        }
        false
    }
}

impl Default for PythonParser {
    fn default() -> Self {
        Self::new()
    }
}

impl PythonParser {
    fn walk_statements(
        &self,
        stmts: &[ast::Stmt],
        context: &mut Vec<String>,
        calls: &mut Vec<Call>,
        file_path: &str,
        converter: &LocationConverter,
    ) {
        for stmt in stmts {
            self.walk_stmt(stmt, context, calls, file_path, converter);
        }
    }

    fn walk_stmt(
        &self,
        stmt: &ast::Stmt,
        context: &mut Vec<String>,
        calls: &mut Vec<Call>,
        file_path: &str,
        converter: &LocationConverter,
    ) {
        match stmt {
            ast::Stmt::FunctionDef(func_def) => {
                context.push(func_def.name.to_string());
                self.walk_statements(&func_def.body, context, calls, file_path, converter);
                context.pop();
            }
            ast::Stmt::AsyncFunctionDef(func_def) => {
                context.push(func_def.name.to_string());
                self.walk_statements(&func_def.body, context, calls, file_path, converter);
                context.pop();
            }
            ast::Stmt::ClassDef(class_def) => {
                context.push(class_def.name.to_string());
                self.walk_statements(&class_def.body, context, calls, file_path, converter);
                context.pop();
            }
            ast::Stmt::Expr(expr_stmt) => {
                self.walk_expr(&expr_stmt.value, context, calls, file_path, converter);
            }
            ast::Stmt::Return(ret_stmt) => {
                if let Some(value) = &ret_stmt.value {
                    self.walk_expr(value, context, calls, file_path, converter);
                }
            }
            ast::Stmt::Assign(assign_stmt) => {
                self.walk_expr(&assign_stmt.value, context, calls, file_path, converter);
            }
            ast::Stmt::AnnAssign(assign_stmt) => {
                if let Some(value) = &assign_stmt.value {
                    self.walk_expr(value, context, calls, file_path, converter);
                }
            }
            ast::Stmt::AugAssign(assign_stmt) => {
                self.walk_expr(&assign_stmt.value, context, calls, file_path, converter);
            }
            ast::Stmt::If(if_stmt) => {
                self.walk_expr(&if_stmt.test, context, calls, file_path, converter);
                self.walk_statements(&if_stmt.body, context, calls, file_path, converter);
                self.walk_statements(&if_stmt.orelse, context, calls, file_path, converter);
            }
            ast::Stmt::For(for_stmt) => {
                self.walk_expr(&for_stmt.iter, context, calls, file_path, converter);
                self.walk_statements(&for_stmt.body, context, calls, file_path, converter);
                self.walk_statements(&for_stmt.orelse, context, calls, file_path, converter);
            }
            ast::Stmt::AsyncFor(for_stmt) => {
                self.walk_expr(&for_stmt.iter, context, calls, file_path, converter);
                self.walk_statements(&for_stmt.body, context, calls, file_path, converter);
                self.walk_statements(&for_stmt.orelse, context, calls, file_path, converter);
            }
            ast::Stmt::While(while_stmt) => {
                self.walk_expr(&while_stmt.test, context, calls, file_path, converter);
                self.walk_statements(&while_stmt.body, context, calls, file_path, converter);
                self.walk_statements(&while_stmt.orelse, context, calls, file_path, converter);
            }
            ast::Stmt::With(with_stmt) => {
                for item in &with_stmt.items {
                    self.walk_expr(&item.context_expr, context, calls, file_path, converter);
                    if let Some(vars) = &item.optional_vars {
                        self.walk_expr(vars, context, calls, file_path, converter);
                    }
                }
                self.walk_statements(&with_stmt.body, context, calls, file_path, converter);
            }
            ast::Stmt::AsyncWith(with_stmt) => {
                for item in &with_stmt.items {
                    self.walk_expr(&item.context_expr, context, calls, file_path, converter);
                    if let Some(vars) = &item.optional_vars {
                        self.walk_expr(vars, context, calls, file_path, converter);
                    }
                }
                self.walk_statements(&with_stmt.body, context, calls, file_path, converter);
            }
            ast::Stmt::Try(try_stmt) => {
                self.walk_statements(&try_stmt.body, context, calls, file_path, converter);
                self.walk_statements(&try_stmt.orelse, context, calls, file_path, converter);
                self.walk_statements(&try_stmt.finalbody, context, calls, file_path, converter);
                for handler in &try_stmt.handlers {
                    match handler {
                        ast::ExceptHandler::ExceptHandler(except_handler) => {
                            if let Some(typ) = &except_handler.type_ {
                                self.walk_expr(typ, context, calls, file_path, converter);
                            }
                            self.walk_statements(
                                &except_handler.body,
                                context,
                                calls,
                                file_path,
                                converter,
                            );
                        }
                    }
                }
            }
            _ => {}
        }
    }

    #[allow(clippy::only_used_in_recursion)]
    fn walk_expr(
        &self,
        expr: &ast::Expr,
        context: &mut Vec<String>,
        calls: &mut Vec<Call>,
        file_path: &str,
        converter: &LocationConverter,
    ) {
        match expr {
            ast::Expr::Call(call_expr) => {
                if let Some(name) = self.call_name(&call_expr.func) {
                    let arguments = self.extract_call_arguments(call_expr);
                    let range = call_expr.range();
                    let (line, column) = converter.byte_offset_to_location(range.start().into());
                    let location = Location {
                        file: file_path.to_string(),
                        line,
                        column: Some(column),
                    };
                    let caller = if context.is_empty() {
                        None
                    } else {
                        Some(context.join("."))
                    };

                    calls.push(Call {
                        name,
                        arguments,
                        generic_params: Vec::new(), // Python doesn't have generic params in calls
                        location,
                        caller,
                        base_object: None,
                        property: None,
                        uses_optional_chaining: false,
                    });
                }

                for arg in &call_expr.args {
                    self.walk_expr(arg, context, calls, file_path, converter);
                }
                for kw in &call_expr.keywords {
                    self.walk_expr(&kw.value, context, calls, file_path, converter);
                }
            }
            ast::Expr::Attribute(attr) => {
                self.walk_expr(&attr.value, context, calls, file_path, converter);
            }
            ast::Expr::BoolOp(bool_op) => {
                for value in &bool_op.values {
                    self.walk_expr(value, context, calls, file_path, converter);
                }
            }
            ast::Expr::BinOp(bin_op) => {
                self.walk_expr(&bin_op.left, context, calls, file_path, converter);
                self.walk_expr(&bin_op.right, context, calls, file_path, converter);
            }
            ast::Expr::UnaryOp(unary) => {
                self.walk_expr(&unary.operand, context, calls, file_path, converter);
            }
            ast::Expr::Compare(compare) => {
                self.walk_expr(&compare.left, context, calls, file_path, converter);
                for comparator in &compare.comparators {
                    self.walk_expr(comparator, context, calls, file_path, converter);
                }
            }
            ast::Expr::IfExp(if_expr) => {
                self.walk_expr(&if_expr.test, context, calls, file_path, converter);
                self.walk_expr(&if_expr.body, context, calls, file_path, converter);
                self.walk_expr(&if_expr.orelse, context, calls, file_path, converter);
            }
            ast::Expr::List(list) => {
                for elt in &list.elts {
                    self.walk_expr(elt, context, calls, file_path, converter);
                }
            }
            ast::Expr::Tuple(tuple) => {
                for elt in &tuple.elts {
                    self.walk_expr(elt, context, calls, file_path, converter);
                }
            }
            ast::Expr::Set(set) => {
                for elt in &set.elts {
                    self.walk_expr(elt, context, calls, file_path, converter);
                }
            }
            ast::Expr::Dict(dict) => {
                for key_expr in dict.keys.iter().flatten() {
                    self.walk_expr(key_expr, context, calls, file_path, converter);
                }
                for value in &dict.values {
                    self.walk_expr(value, context, calls, file_path, converter);
                }
            }
            ast::Expr::Subscript(sub) => {
                self.walk_expr(&sub.value, context, calls, file_path, converter);
                self.walk_expr(&sub.slice, context, calls, file_path, converter);
            }
            ast::Expr::Await(await_expr) => {
                self.walk_expr(&await_expr.value, context, calls, file_path, converter);
            }
            ast::Expr::Lambda(lambda_expr) => {
                self.walk_expr(&lambda_expr.body, context, calls, file_path, converter);
            }
            ast::Expr::GeneratorExp(gen_expr) => {
                self.walk_expr(&gen_expr.elt, context, calls, file_path, converter);
                for comp in &gen_expr.generators {
                    self.walk_expr(&comp.iter, context, calls, file_path, converter);
                    self.walk_expr(&comp.target, context, calls, file_path, converter);
                    for if_expr in &comp.ifs {
                        self.walk_expr(if_expr, context, calls, file_path, converter);
                    }
                }
            }
            _ => {}
        }
    }

    #[allow(clippy::only_used_in_recursion)]
    fn call_name(&self, expr: &ast::Expr) -> Option<String> {
        match expr {
            ast::Expr::Name(name) => Some(name.id.to_string()),
            ast::Expr::Attribute(attr) => {
                let base = self.call_name(&attr.value)?;
                Some(format!("{}.{}", base, attr.attr))
            }
            _ => None,
        }
    }

    fn extract_call_arguments(&self, call_expr: &ast::ExprCall) -> Vec<CallArgument> {
        let mut args = Vec::new();

        for arg in &call_expr.args {
            args.push(CallArgument {
                parameter_name: None,
                value: self.expr_to_string(arg),
            });
        }

        for kw in &call_expr.keywords {
            args.push(CallArgument {
                parameter_name: kw.arg.as_ref().map(|name| name.to_string()),
                value: self.expr_to_string(&kw.value),
            });
        }

        args
    }

    /// Converts an expression to its string representation
    pub fn expr_to_string(&self, expr: &ast::Expr) -> String {
        match expr {
            ast::Expr::Name(name) => name.id.to_string(),
            ast::Expr::Attribute(attr) => {
                format!("{}.{}", self.expr_to_string(&attr.value), attr.attr)
            }
            ast::Expr::Subscript(sub) => {
                let base = self.expr_to_string(&sub.value);
                let slice_expr = sub.slice.as_ref();
                let slice_str = match slice_expr {
                    ast::Expr::Tuple(tuple) => tuple
                        .elts
                        .iter()
                        .map(|elt| self.expr_to_string(elt))
                        .collect::<Vec<_>>()
                        .join(", "),
                    _ => self.expr_to_string(slice_expr),
                };
                format!("{}[{}]", base, slice_str)
            }
            ast::Expr::Constant(constant) => match &constant.value {
                ast::Constant::Str(s) => s.clone(),
                ast::Constant::Bytes(b) => {
                    format!("bytes(len={})", b.len())
                }
                ast::Constant::Int(i) => i.to_string(),
                ast::Constant::Float(f) => f.to_string(),
                ast::Constant::Complex { real, imag } => format!("{}+{}j", real, imag),
                ast::Constant::Bool(b) => b.to_string(),
                ast::Constant::None => "None".to_string(),
                ast::Constant::Ellipsis => "...".to_string(),
                ast::Constant::Tuple(_) => "tuple".to_string(),
            },
            ast::Expr::Call(call_expr) => {
                if let Some(name) = self.call_name(&call_expr.func) {
                    format!("{}(...)", name)
                } else {
                    "call(...)".to_string()
                }
            }
            _ => format!("{:?}", expr),
        }
    }

    #[allow(clippy::only_used_in_recursion)]
    fn collect_decorators(
        &self,
        stmt: &ast::Stmt,
        class_context: Option<String>,
        decorators: &mut Vec<crate::call_graph::Decorator>,
        file_path: &str,
        converter: &LocationConverter,
    ) {
        match stmt {
            ast::Stmt::FunctionDef(func_def) => {
                let target_name = class_context
                    .as_ref()
                    .map(|class| format!("{}.{}", class, func_def.name))
                    .unwrap_or_else(|| func_def.name.to_string());

                self.process_function_decorators(
                    &target_name,
                    &func_def.decorator_list,
                    class_context.as_deref(),
                    decorators,
                    file_path,
                    converter,
                );
            }
            ast::Stmt::AsyncFunctionDef(func_def) => {
                let target_name = class_context
                    .as_ref()
                    .map(|class| format!("{}.{}", class, func_def.name))
                    .unwrap_or_else(|| func_def.name.to_string());

                self.process_function_decorators(
                    &target_name,
                    &func_def.decorator_list,
                    class_context.as_deref(),
                    decorators,
                    file_path,
                    converter,
                );
            }
            ast::Stmt::ClassDef(class_def) => {
                let next_context = class_context
                    .as_ref()
                    .map(|ctx| format!("{}.{}", ctx, class_def.name))
                    .unwrap_or_else(|| class_def.name.to_string());

                for body_stmt in &class_def.body {
                    self.collect_decorators(
                        body_stmt,
                        Some(next_context.clone()),
                        decorators,
                        file_path,
                        converter,
                    );
                }
            }
            _ => {}
        }
    }

    /// Processes function decorators (common code for FunctionDef and AsyncFunctionDef)
    fn process_function_decorators(
        &self,
        func_name: &str,
        decorator_list: &[ast::Expr],
        class_context: Option<&str>,
        decorators: &mut Vec<crate::call_graph::Decorator>,
        file_path: &str,
        converter: &LocationConverter,
    ) {
        let target_name = class_context
            .map(|class| format!("{}.{}", class, func_name))
            .unwrap_or_else(|| func_name.to_string());

        for decorator in decorator_list {
            if let Some(name) = self.get_decorator_name(decorator) {
                if self.is_route_decorator(&name) {
                    let (args, kwargs) = self.extract_decorator_arguments(decorator);
                    // Extract real location from decorator AST
                    let range = decorator.range();
                    let (line, column) = converter.byte_offset_to_location(range.start().into());
                    decorators.push(crate::call_graph::Decorator {
                        name,
                        arguments: args,
                        keyword_arguments: kwargs,
                        location: Location {
                            file: file_path.to_string(),
                            line,
                            column: Some(column),
                        },
                        target_function: Some(target_name.clone()),
                    });
                }
            }
        }
    }

    #[allow(clippy::only_used_in_recursion)]
    fn get_decorator_name(&self, decorator: &ast::Expr) -> Option<String> {
        match decorator {
            ast::Expr::Attribute(attr) => self
                .get_decorator_name(&attr.value)
                .map(|base| format!("{}.{}", base, attr.attr)),
            ast::Expr::Name(name) => Some(name.id.to_string()),
            ast::Expr::Call(call_expr) => self.get_decorator_name(&call_expr.func),
            _ => None,
        }
    }

    fn extract_decorator_arguments(
        &self,
        decorator: &ast::Expr,
    ) -> (Vec<String>, std::collections::HashMap<String, String>) {
        if let ast::Expr::Call(call_expr) = decorator {
            let mut args = Vec::new();
            let mut kwargs = std::collections::HashMap::new();

            // Extract positional arguments
            for arg in &call_expr.args {
                args.push(self.expr_to_string(arg));
            }

            // Extract keyword arguments
            for kw in &call_expr.keywords {
                if let Some(arg_name) = &kw.arg {
                    kwargs.insert(arg_name.to_string(), self.expr_to_string(&kw.value));
                }
            }

            (args, kwargs)
        } else {
            (Vec::new(), std::collections::HashMap::new())
        }
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

    /// Extracts response_model from decorator keyword arguments
    /// Handles patterns like:
    /// - response_model=ItemRead
    /// - response_model=Page[ItemRead]
    pub fn extract_response_model_from_decorator(
        &self,
        decorator: &crate::call_graph::Decorator,
    ) -> Option<String> {
        decorator.keyword_arguments.get("response_model").cloned()
    }

    /// Extracts the base model name from a response_model type
    /// Handles generic types like Page[ItemRead] -> ItemRead
    /// Recursively extracts inner types from nested generics
    pub fn extract_base_model_from_response_model(&self, response_model: &str) -> String {
        self.extract_inner_type_recursive(response_model)
    }

    /// Recursively extracts inner type from generic types
    /// Examples:
    /// - Page[ItemRead] -> ItemRead
    /// - list[ItemRead] -> ItemRead
    /// - Optional[ItemRead] -> ItemRead
    /// - dict[str, ItemRead] -> ItemRead (extracts value type)
    /// - Page[Optional[ItemRead]] -> ItemRead (nested generic)
    #[allow(clippy::only_used_in_recursion)]
    fn extract_inner_type_recursive(&self, type_str: &str) -> String {
        // Remove whitespace
        let trimmed = type_str.trim();

        // If this is a generic type with square brackets
        if let Some(start_bracket) = trimmed.find('[') {
            if let Some(end_bracket) = trimmed.rfind(']') {
                if start_bracket < end_bracket {
                    let base_type = trimmed[..start_bracket].trim();
                    let inner = &trimmed[start_bracket + 1..end_bracket];
                    let inner_trimmed = inner.trim();

                    // If inner is empty (e.g., Page[]), return original
                    if inner_trimmed.is_empty() {
                        return trimmed.to_string();
                    }

                    // Handle dict[K, V] - extract value type (V)
                    if base_type == "dict" || base_type == "Dict" {
                        // Find comma separating key and value types
                        if let Some(comma_pos) = inner_trimmed.find(',') {
                            let value_type = &inner_trimmed[comma_pos + 1..].trim();
                            // Recursively extract from value type
                            if value_type.contains('[') {
                                return self.extract_inner_type_recursive(value_type);
                            }
                            return value_type.to_string();
                        }
                        // If no comma, treat as single type
                        return self.extract_inner_type_recursive(inner_trimmed);
                    }

                    // Handle list[T], List[T] - extract T
                    if base_type == "list" || base_type == "List" {
                        // Recursively extract from inner type
                        if inner_trimmed.contains('[') {
                            return self.extract_inner_type_recursive(inner_trimmed);
                        }
                        return inner_trimmed.to_string();
                    }

                    // Handle Optional[T] - extract T
                    if base_type == "Optional" {
                        // Recursively extract from inner type
                        if inner_trimmed.contains('[') {
                            return self.extract_inner_type_recursive(inner_trimmed);
                        }
                        return inner_trimmed.to_string();
                    }

                    // Handle Union[T1, T2, None] - extract first non-None type
                    if base_type == "Union" {
                        // Split by comma and find first non-None type
                        let parts: Vec<&str> = inner_trimmed.split(',').collect();
                        for part in parts {
                            let part_trimmed = part.trim();
                            if part_trimmed != "None" && !part_trimmed.is_empty() {
                                // Recursively extract from this type
                                if part_trimmed.contains('[') {
                                    return self.extract_inner_type_recursive(part_trimmed);
                                }
                                return part_trimmed.to_string();
                            }
                        }
                        // If all are None, return original
                        return trimmed.to_string();
                    }

                    // For other generic types (e.g., Page[T], CustomGeneric[T])
                    // Recursively extract from inner type
                    if inner_trimmed.contains('[') {
                        return self.extract_inner_type_recursive(inner_trimmed);
                    }
                    return inner_trimmed.to_string();
                }
            }
        }

        // If not generic, return as is
        trimmed.to_string()
    }

    /// Extracts model_config dictionary
    /// Returns HashMap with config keys and string values
    fn extract_model_config(
        &self,
        expr: &ast::Expr,
    ) -> Option<std::collections::HashMap<String, String>> {
        if let ast::Expr::Dict(dict) = expr {
            let mut config = std::collections::HashMap::new();
            for (key_expr, value_expr) in dict.keys.iter().zip(dict.values.iter()) {
                if let Some(key_expr) = key_expr {
                    let key = self.expr_to_string(key_expr);
                    let value = self.expr_to_string(value_expr);
                    config.insert(key, value);
                }
            }
            Some(config)
        } else {
            None
        }
    }

    /// Extracts Optional or Union type information
    /// Returns (is_optional, base_type)
    /// Examples:
    /// - Optional[str] -> (true, "str")
    /// - Union[str, None] -> (true, "str")
    /// - str | None -> (true, "str")
    fn extract_optional_or_union_type(&self, expr: &ast::Expr) -> (bool, String) {
        match expr {
            ast::Expr::Subscript(sub) => {
                // Check for Optional[T] or Union[T, None]
                let base = self.expr_to_string(sub.value.as_ref());
                let slice_str = self.expr_to_string(sub.slice.as_ref());

                if base == "Optional" {
                    // Optional[T] -> extract T
                    return (true, slice_str);
                } else if base == "Union" {
                    // Union[T1, T2, None] -> collect all non-None types
                    if let ast::Expr::Tuple(tuple) = sub.slice.as_ref() {
                        let mut types = Vec::new();
                        let mut has_none = false;
                        for elt in &tuple.elts {
                            let elt_str = self.expr_to_string(elt);
                            if elt_str == "None" {
                                has_none = true;
                            } else {
                                types.push(elt_str);
                            }
                        }
                        if !types.is_empty() {
                            let combined = if types.len() == 1 {
                                types[0].clone()
                            } else {
                                format!("Union[{}]", types.join(", "))
                            };
                            return (has_none, combined);
                        }
                    }
                }
                (false, self.expr_to_string(expr))
            }
            ast::Expr::BinOp(bin_op) => {
                // Check for str | int | None (Python 3.10+ union syntax)
                // rustpython_parser uses Operator enum
                use rustpython_parser::ast::Operator;
                if matches!(bin_op.op, Operator::BitOr) {
                    // Recursively collect all types from nested BitOr operations
                    let (left_optional, left_types) =
                        self.collect_union_types(bin_op.left.as_ref());
                    let (right_optional, right_types) =
                        self.collect_union_types(bin_op.right.as_ref());

                    let mut all_types = left_types;
                    all_types.extend(right_types);
                    let has_none = left_optional || right_optional;

                    if !all_types.is_empty() {
                        let combined = if all_types.len() == 1 {
                            all_types[0].clone()
                        } else {
                            format!("Union[{}]", all_types.join(", "))
                        };
                        return (has_none, combined);
                    }
                }
                (false, self.expr_to_string(expr))
            }
            _ => (false, self.expr_to_string(expr)),
        }
    }

    /// Recursively collects all types from a union expression (handles nested BitOr)
    /// Returns (has_none, vec_of_types)
    fn collect_union_types(&self, expr: &ast::Expr) -> (bool, Vec<String>) {
        use rustpython_parser::ast::Operator;
        match expr {
            ast::Expr::BinOp(bin_op) if matches!(bin_op.op, Operator::BitOr) => {
                let (left_optional, mut left_types) =
                    self.collect_union_types(bin_op.left.as_ref());
                let (right_optional, right_types) = self.collect_union_types(bin_op.right.as_ref());
                left_types.extend(right_types);
                (left_optional || right_optional, left_types)
            }
            _ => {
                let type_str = self.expr_to_string(expr);
                let has_none = type_str == "None";
                if has_none {
                    (true, Vec::new())
                } else {
                    (false, vec![type_str])
                }
            }
        }
    }

    /// Extracts the value type from dict[K, V] -> returns V
    fn extract_dict_value_type(&self, slice: &ast::Expr) -> anyhow::Result<String> {
        if let ast::Expr::Tuple(tuple) = slice {
            // dict[K, V] -> tuple with 2 elements
            if tuple.elts.len() >= 2 {
                // Return the second element (value type)
                Ok(self.expr_to_string(&tuple.elts[1]))
            } else {
                Ok("Any".to_string())
            }
        } else {
            // Fallback: treat as single type
            Ok(self.expr_to_string(slice))
        }
    }

    /// Extracts type information with support for generic types
    /// Returns (is_optional, base_type, inner_type)
    fn extract_type_with_generics(
        &self,
        expr: &ast::Expr,
    ) -> anyhow::Result<(bool, String, Option<String>)> {
        match expr {
            ast::Expr::Subscript(sub) => {
                let base = self.expr_to_string(sub.value.as_ref());
                let slice_str = self.expr_to_string(sub.slice.as_ref());

                if base == "Optional" {
                    // Optional[T] -> (optional=true, T, None)
                    Ok((true, slice_str, None))
                } else if base == "list" || base == "List" {
                    // list[T] -> (optional=false, "array", Some(T))
                    Ok((false, "array".to_string(), Some(slice_str)))
                } else if base == "dict" || base == "Dict" {
                    // dict[K, V] -> extract V as inner_type
                    let inner = self.extract_dict_value_type(sub.slice.as_ref())?;
                    Ok((false, "object".to_string(), Some(inner)))
                } else {
                    // Other generic types (e.g., Page[T])
                    Ok((false, base, Some(slice_str)))
                }
            }
            ast::Expr::BinOp(bin_op) => {
                use rustpython_parser::ast::Operator;
                if matches!(bin_op.op, Operator::BitOr) {
                    // Python 3.10+ union syntax: str | int | None
                    let (has_none, types) = self.collect_union_types(expr);
                    let combined = if types.len() == 1 {
                        types[0].clone()
                    } else {
                        format!("Union[{}]", types.join(", "))
                    };
                    Ok((has_none, combined, None))
                } else {
                    Ok((false, self.expr_to_string(expr), None))
                }
            }
            _ => {
                // Check for Optional/Union using existing method
                let (is_optional, base_type) = self.extract_optional_or_union_type(expr);
                Ok((is_optional, base_type, None))
            }
        }
    }

    /// Extracts structured field constraints from Field() call
    fn extract_field_constraints_structured(
        &self,
        expr: &ast::Expr,
    ) -> anyhow::Result<Vec<crate::models::FieldConstraint>> {
        let mut constraints = Vec::new();

        if let ast::Expr::Call(call_expr) = expr {
            // Check if it's Field() call
            if let Some(call_name) = self.call_name(&call_expr.func) {
                if call_name == "Field" || call_name.ends_with(".Field") {
                    for kw in &call_expr.keywords {
                        if let Some(arg_name) = &kw.arg {
                            let value_str = self.expr_to_string(&kw.value);

                            match arg_name.as_str() {
                                "min_length" => {
                                    if let Ok(value) = value_str.parse::<usize>() {
                                        constraints
                                            .push(crate::models::FieldConstraint::MinLength(value));
                                    }
                                }
                                "max_length" => {
                                    if let Ok(value) = value_str.parse::<usize>() {
                                        constraints
                                            .push(crate::models::FieldConstraint::MaxLength(value));
                                    }
                                }
                                "min" => {
                                    if let Ok(value) = value_str.parse::<f64>() {
                                        constraints
                                            .push(crate::models::FieldConstraint::MinValue(value));
                                    }
                                }
                                "max" => {
                                    if let Ok(value) = value_str.parse::<f64>() {
                                        constraints
                                            .push(crate::models::FieldConstraint::MaxValue(value));
                                    }
                                }
                                "regex" | "pattern" => {
                                    constraints
                                        .push(crate::models::FieldConstraint::Pattern(value_str));
                                }
                                _ => {}
                            }
                        }
                    }
                }
            }
        }

        Ok(constraints)
    }

    /// Extracts information about a field from AnnAssign
    fn extract_field_info(
        &self,
        ann_assign: &ast::StmtAnnAssign,
    ) -> anyhow::Result<crate::models::PydanticFieldInfo> {
        let field_name = if let ast::Expr::Name(name) = ann_assign.target.as_ref() {
            name.id.to_string()
        } else {
            return Err(anyhow::anyhow!("Invalid field target"));
        };

        let field_type_expr = ann_assign.annotation.as_ref();

        // Extract base type, inner type, and optionality
        let (is_optional, base_type, inner_type) =
            self.extract_type_with_generics(field_type_expr)?;

        // Extract constraints from Field() if present
        let constraints = if let Some(value) = &ann_assign.value {
            self.extract_field_constraints_structured(value)?
        } else {
            Vec::new()
        };

        // Extract default value
        let default_value = ann_assign.value.as_ref().and_then(|v| {
            // Skip if it's Field() call
            if let ast::Expr::Call(call) = v.as_ref() {
                if let Some(call_name) = self.call_name(&call.func) {
                    if call_name == "Field" || call_name.ends_with(".Field") {
                        return None;
                    }
                }
            }
            Some(self.expr_to_string(v))
        });

        Ok(crate::models::PydanticFieldInfo {
            name: field_name,
            type_name: base_type,
            inner_type,
            optional: is_optional,
            constraints,
            default_value,
        })
    }

    /// Extracts class name from expression (handles dotted names)
    fn extract_class_name_from_expr(&self, expr: &ast::Expr) -> String {
        let expr_str = self.expr_to_string(expr);
        expr_str
            .rsplit('.')
            .next()
            .or_else(|| expr_str.rsplit("::").next())
            .unwrap_or(&expr_str)
            .to_string()
    }

    /// Finds a Pydantic model by name in the AST
    fn find_pydantic_model_by_name<'a>(
        &self,
        class_name: &str,
        ast: &'a ast::Mod,
    ) -> Option<&'a ast::StmtClassDef> {
        if let ast::Mod::Module(module) = ast {
            for stmt in &module.body {
                if let ast::Stmt::ClassDef(class_def) = stmt {
                    if class_def.name.as_str() == class_name {
                        // Check if it's a Pydantic model
                        if self.is_pydantic_base_model(&class_def.bases) {
                            return Some(class_def);
                        }
                    }
                }
            }
        }
        None
    }

    /// Extracts fields from a class definition
    fn extract_fields_from_class_def(
        &self,
        class_def: &ast::StmtClassDef,
    ) -> anyhow::Result<Vec<crate::models::PydanticFieldInfo>> {
        let mut fields = Vec::new();

        // Extract fields from current class
        for body_stmt in &class_def.body {
            if let ast::Stmt::AnnAssign(ann_assign) = body_stmt {
                if let Ok(field_info) = self.extract_field_info(ann_assign) {
                    fields.push(field_info);
                }
            }
        }

        // Note: Recursive extraction from base classes in other files
        // would require access to full AST of all files, which can be enhanced later

        Ok(fields)
    }
}
