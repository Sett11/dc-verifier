use crate::models::{Location, SchemaReference, SchemaType, TypeInfo};
use crate::parsers::{Call, CallArgument, FunctionInfo, Import, LocationConverter};
use anyhow::Result;
use std::path::Path;
use swc_common::{sync::Lrc, FileName, SourceMap};
use swc_ecma_ast::*;
use swc_ecma_parser::{lexer::Lexer, Parser, StringInput, Syntax, TsSyntax};

/// TypeScript code parser with call analysis (via swc)
pub struct TypeScriptParser {
    source_map: SourceMap,
}

impl TypeScriptParser {
    /// Creates a new parser
    pub fn new() -> Self {
        Self {
            source_map: SourceMap::default(),
        }
    }

    /// Parses a file via swc
    pub fn parse_file(&self, path: &Path) -> Result<(Module, String, LocationConverter)> {
        let source = std::fs::read_to_string(path)?;
        let module = self.parse_source(&source, path)?;
        let converter = LocationConverter::new(source.clone());
        Ok((module, source, converter))
    }

    /// Parses source code
    fn parse_source(&self, source: &str, path: &Path) -> Result<Module> {
        let file_name: Lrc<FileName> = FileName::Real(path.to_path_buf()).into();
        let fm = self
            .source_map
            .new_source_file(file_name, source.to_string());

        let is_tsx = path.extension().and_then(|e| e.to_str()) == Some("tsx");
        let syntax = Syntax::Typescript(TsSyntax {
            tsx: is_tsx,
            ..Default::default()
        });

        let lexer = Lexer::new(syntax, Default::default(), StringInput::from(&*fm), None);
        let mut parser = Parser::new_from(lexer);

        parser
            .parse_module()
            .map_err(|e| anyhow::anyhow!("Parse error: {:?}", e))
    }

    /// Extracts imports from module
    pub fn extract_imports(
        &self,
        module: &Module,
        file_path: &str,
        converter: &LocationConverter,
    ) -> Vec<Import> {
        let mut imports = Vec::new();

        for item in &module.body {
            if let ModuleItem::ModuleDecl(ModuleDecl::Import(import_decl)) = item {
                let span = import_decl.span;
                let (line, column) = converter.byte_offset_to_location(span.lo.0 as usize);

                // Extract import path from src
                let import_path = import_decl.src.value.as_str().unwrap_or("").to_string();

                // Extract names from specifiers
                let mut names = Vec::new();
                for specifier in &import_decl.specifiers {
                    match specifier {
                        ImportSpecifier::Named(named) => {
                            if let Some(imported) = &named.imported {
                                match imported {
                                    ModuleExportName::Ident(ident) => {
                                        names.push(ident.sym.as_ref().to_string());
                                    }
                                    ModuleExportName::Str(str) => {
                                        names.push(str.value.as_str().unwrap_or("").to_string());
                                    }
                                }
                            } else {
                                names.push(named.local.sym.as_ref().to_string());
                            }
                        }
                        ImportSpecifier::Default(default) => {
                            names.push(default.local.sym.as_ref().to_string());
                        }
                        ImportSpecifier::Namespace(namespace) => {
                            names.push(namespace.local.sym.as_ref().to_string());
                        }
                    }
                }

                imports.push(Import {
                    path: import_path,
                    names,
                    location: Location {
                        file: file_path.to_string(),
                        line,
                        column: Some(column),
                    },
                });
            }
        }

        imports
    }

    /// Extracts function calls from module
    pub fn extract_calls(
        &self,
        module: &Module,
        file_path: &str,
        converter: &LocationConverter,
        source: &str,
    ) -> Vec<Call> {
        let mut calls = Vec::new();
        let mut context = Vec::new();

        for item in &module.body {
            self.walk_module_item(item, &mut context, &mut calls, file_path, converter, source);
        }

        calls
    }

    /// Traverses ModuleItem and extracts calls
    fn walk_module_item(
        &self,
        item: &ModuleItem,
        context: &mut Vec<String>,
        calls: &mut Vec<Call>,
        file_path: &str,
        converter: &LocationConverter,
        source: &str,
    ) {
        match item {
            ModuleItem::Stmt(stmt) => {
                self.walk_stmt(stmt, context, calls, file_path, converter, source);
            }
            ModuleItem::ModuleDecl(ModuleDecl::ExportDecl(export_decl)) => {
                if let Decl::Fn(fn_decl) = &export_decl.decl {
                    context.push(fn_decl.ident.sym.as_ref().to_string());
                    if let Some(body) = &fn_decl.function.body {
                        self.walk_block_stmt(body, context, calls, file_path, converter, source);
                    }
                    context.pop();
                }
            }
            _ => {}
        }
    }

    /// Traverses Statement and extracts calls
    fn walk_stmt(
        &self,
        stmt: &Stmt,
        context: &mut Vec<String>,
        calls: &mut Vec<Call>,
        file_path: &str,
        converter: &LocationConverter,
        source: &str,
    ) {
        match stmt {
            Stmt::Expr(expr_stmt) => {
                self.walk_expr(
                    &expr_stmt.expr,
                    context,
                    calls,
                    file_path,
                    converter,
                    source,
                );
            }
            Stmt::Return(ret_stmt) => {
                if let Some(expr) = &ret_stmt.arg {
                    self.walk_expr(expr, context, calls, file_path, converter, source);
                }
            }
            Stmt::If(if_stmt) => {
                self.walk_expr(&if_stmt.test, context, calls, file_path, converter, source);
                self.walk_stmt(&if_stmt.cons, context, calls, file_path, converter, source);
                if let Some(alt) = &if_stmt.alt {
                    self.walk_stmt(alt, context, calls, file_path, converter, source);
                }
            }
            Stmt::While(while_stmt) => {
                self.walk_expr(
                    &while_stmt.test,
                    context,
                    calls,
                    file_path,
                    converter,
                    source,
                );
                self.walk_stmt(
                    &while_stmt.body,
                    context,
                    calls,
                    file_path,
                    converter,
                    source,
                );
            }
            Stmt::For(for_stmt) => {
                if let Some(init) = &for_stmt.init {
                    self.walk_var_decl_or_expr(init, context, calls, file_path, converter, source);
                }
                if let Some(test) = &for_stmt.test {
                    self.walk_expr(test, context, calls, file_path, converter, source);
                }
                if let Some(update) = &for_stmt.update {
                    self.walk_expr(update, context, calls, file_path, converter, source);
                }
                self.walk_stmt(&for_stmt.body, context, calls, file_path, converter, source);
            }
            Stmt::Block(block_stmt) => {
                self.walk_block_stmt(block_stmt, context, calls, file_path, converter, source);
            }
            Stmt::Decl(Decl::Fn(fn_decl)) => {
                context.push(fn_decl.ident.sym.as_ref().to_string());
                if let Some(body) = &fn_decl.function.body {
                    self.walk_block_stmt(body, context, calls, file_path, converter, source);
                }
                context.pop();
            }
            Stmt::Decl(Decl::Var(var_decl)) => {
                for decl in &var_decl.decls {
                    if let Some(init) = &decl.init {
                        self.walk_expr(init, context, calls, file_path, converter, source);
                    }
                }
            }
            _ => {}
        }
    }

    /// Traverses BlockStmt
    fn walk_block_stmt(
        &self,
        block: &BlockStmt,
        context: &mut Vec<String>,
        calls: &mut Vec<Call>,
        file_path: &str,
        converter: &LocationConverter,
        source: &str,
    ) {
        for stmt in &block.stmts {
            self.walk_stmt(stmt, context, calls, file_path, converter, source);
        }
    }

    /// Traverses VarDeclOrExpr
    fn walk_var_decl_or_expr(
        &self,
        init: &VarDeclOrExpr,
        context: &mut Vec<String>,
        calls: &mut Vec<Call>,
        file_path: &str,
        converter: &LocationConverter,
        source: &str,
    ) {
        match init {
            VarDeclOrExpr::VarDecl(var_decl) => {
                for decl in &var_decl.decls {
                    if let Some(init) = &decl.init {
                        self.walk_expr(init, context, calls, file_path, converter, source);
                    }
                }
            }
            VarDeclOrExpr::Expr(expr) => {
                self.walk_expr(expr, context, calls, file_path, converter, source);
            }
        }
    }

    /// Traverses Expression and extracts calls
    fn walk_expr(
        &self,
        expr: &Expr,
        context: &mut Vec<String>,
        calls: &mut Vec<Call>,
        file_path: &str,
        converter: &LocationConverter,
        source: &str,
    ) {
        match expr {
            Expr::Call(call_expr) => {
                if let Some(name) = self.call_name(&call_expr.callee) {
                    let arguments = self.extract_call_arguments(call_expr);
                    let generic_params = self.extract_generic_params_from_call(call_expr, source);
                    let span = call_expr.span;
                    let (line, column) = converter.byte_offset_to_location(span.lo.0 as usize);

                    let caller = if context.is_empty() {
                        None
                    } else {
                        Some(context.join("."))
                    };

                    // Extract AST information for member expressions
                    let (base_object, property, uses_optional_chaining) =
                        self.extract_member_info(&call_expr.callee);

                    calls.push(Call {
                        name,
                        arguments,
                        generic_params,
                        location: Location {
                            file: file_path.to_string(),
                            line,
                            column: Some(column),
                        },
                        caller,
                        base_object,
                        property,
                        uses_optional_chaining,
                    });
                }

                // Recursively traverse arguments
                for arg in &call_expr.args {
                    self.walk_expr_or_spread(arg, context, calls, file_path, converter, source);
                }
            }
            Expr::Member(member_expr) => {
                self.walk_expr(
                    member_expr.obj.as_ref(),
                    context,
                    calls,
                    file_path,
                    converter,
                    source,
                );
            }
            Expr::Bin(bin_expr) => {
                self.walk_expr(
                    bin_expr.left.as_ref(),
                    context,
                    calls,
                    file_path,
                    converter,
                    source,
                );
                self.walk_expr(
                    bin_expr.right.as_ref(),
                    context,
                    calls,
                    file_path,
                    converter,
                    source,
                );
            }
            Expr::Unary(unary_expr) => {
                self.walk_expr(
                    unary_expr.arg.as_ref(),
                    context,
                    calls,
                    file_path,
                    converter,
                    source,
                );
            }
            Expr::Cond(cond_expr) => {
                self.walk_expr(
                    cond_expr.test.as_ref(),
                    context,
                    calls,
                    file_path,
                    converter,
                    source,
                );
                self.walk_expr(
                    cond_expr.cons.as_ref(),
                    context,
                    calls,
                    file_path,
                    converter,
                    source,
                );
                self.walk_expr(
                    cond_expr.alt.as_ref(),
                    context,
                    calls,
                    file_path,
                    converter,
                    source,
                );
            }
            Expr::Assign(assign_expr) => {
                self.walk_expr(
                    assign_expr.right.as_ref(),
                    context,
                    calls,
                    file_path,
                    converter,
                    source,
                );
            }
            _ => {}
        }
    }

    /// Traverses ExprOrSpread
    fn walk_expr_or_spread(
        &self,
        arg: &ExprOrSpread,
        context: &mut Vec<String>,
        calls: &mut Vec<Call>,
        file_path: &str,
        converter: &LocationConverter,
        source: &str,
    ) {
        self.walk_expr(&arg.expr, context, calls, file_path, converter, source);
    }

    /// Extracts function name from Callee
    #[allow(clippy::only_used_in_recursion)]
    fn call_name(&self, callee: &Callee) -> Option<String> {
        match callee {
            Callee::Expr(expr) => match expr.as_ref() {
                Expr::Ident(ident) => Some(ident.sym.as_ref().to_string()),
                Expr::Member(member_expr) => {
                    let base = self.call_name(&Callee::Expr(member_expr.obj.clone()))?;
                    let prop = match &member_expr.prop {
                        MemberProp::Ident(ident) => ident.sym.as_ref().to_string(),
                        MemberProp::Computed(computed) => {
                            if let Expr::Lit(Lit::Str(str)) = computed.expr.as_ref() {
                                str.value.as_str().unwrap_or("").to_string()
                            } else {
                                return None;
                            }
                        }
                        _ => return None,
                    };
                    Some(format!("{}.{}", base, prop))
                }
                _ => None,
            },
            Callee::Super(_) => Some("super".to_string()),
            Callee::Import(_) => Some("import".to_string()),
        }
    }

    /// Extracts member expression information from Callee
    /// Returns (base_object, property, uses_optional_chaining)
    #[allow(clippy::only_used_in_recursion)]
    fn extract_member_info(&self, callee: &Callee) -> (Option<String>, Option<String>, bool) {
        match callee {
            Callee::Expr(expr) => match expr.as_ref() {
                Expr::Member(member_expr) => {
                    let base = self.extract_base_from_expr(&member_expr.obj);
                    let property = match &member_expr.prop {
                        MemberProp::Ident(ident) => Some(ident.sym.as_ref().to_string()),
                        MemberProp::Computed(computed) => {
                            if let Expr::Lit(Lit::Str(str)) = computed.expr.as_ref() {
                                Some(str.value.as_str().unwrap_or("").to_string())
                            } else {
                                None
                            }
                        }
                        _ => None,
                    };
                    // MemberExpr doesn't have optional field, check if base uses optional chaining
                    let uses_optional_chaining = self.has_optional_chaining(&member_expr.obj);
                    (base, property, uses_optional_chaining)
                }
                Expr::OptChain(opt_chain) => {
                    // Handle optional chaining expression
                    if let OptChainBase::Member(member) = opt_chain.base.as_ref() {
                        let base = self.extract_base_from_expr(&member.obj);
                        let property = match &member.prop {
                            MemberProp::Ident(ident) => Some(ident.sym.as_ref().to_string()),
                            MemberProp::Computed(computed) => {
                                if let Expr::Lit(Lit::Str(str)) = computed.expr.as_ref() {
                                    Some(str.value.as_str().unwrap_or("").to_string())
                                } else {
                                    None
                                }
                            }
                            _ => None,
                        };
                        (base, property, true)
                    } else {
                        (None, None, true)
                    }
                }
                _ => (None, None, false),
            },
            _ => (None, None, false),
        }
    }

    /// Checks if expression uses optional chaining
    #[allow(clippy::only_used_in_recursion)]
    fn has_optional_chaining(&self, expr: &Expr) -> bool {
        match expr {
            Expr::OptChain(_) => true,
            Expr::Member(member_expr) => self.has_optional_chaining(&member_expr.obj),
            _ => false,
        }
    }

    /// Extracts base object name from expression
    #[allow(clippy::only_used_in_recursion)]
    fn extract_base_from_expr(&self, expr: &Expr) -> Option<String> {
        match expr {
            Expr::Ident(ident) => Some(ident.sym.as_ref().to_string()),
            Expr::Member(member_expr) => {
                // For chained calls like a.b.c, extract the base
                self.extract_base_from_expr(&member_expr.obj)
            }
            _ => None,
        }
    }

    /// Extracts generic type parameters from a call expression
    ///
    /// In SWC AST for TypeScript, generic parameters in function calls like `useQuery<ResponseType>()`
    /// are not directly stored in CallExpr. We need to parse them from the source code
    /// or use type information.
    ///
    /// This method attempts to extract generic parameters by:
    /// 1. Checking if callee has type parameters (for future enhancement)
    /// 2. Parsing source code around the call (requires source access)
    fn extract_generic_params_from_call(
        &self,
        call_expr: &CallExpr,
        source: &str,
    ) -> Vec<crate::models::TypeInfo> {
        // Parse generic parameters from source code
        let type_names = self.parse_generic_params_from_source(call_expr, source);

        // Convert type names to TypeInfo
        // Use a dummy file_path and line - they will be set properly when we have converter
        let file_path = "";
        let line = 0;

        type_names
            .iter()
            .filter_map(|type_name| {
                self.parse_type_name_to_type_info(type_name, file_path, line)
                    .ok()
            })
            .collect()
    }

    /// Parses generic type parameters from source code around a call expression
    /// Example: useQuery<UserResponse, Error>() -> ["UserResponse", "Error"]
    fn parse_generic_params_from_source(&self, call_expr: &CallExpr, source: &str) -> Vec<String> {
        // Get the span of the callee to find where the function name ends
        let callee_span = match &call_expr.callee {
            Callee::Expr(expr) => {
                // For Box<Expr>, we need to match on the expression type to get span
                match expr.as_ref() {
                    Expr::Ident(ident) => ident.span,
                    Expr::Member(member) => member.span,
                    _ => {
                        // For other expression types, use the call_expr span as fallback
                        // This is not ideal but will work for most cases
                        return Vec::new();
                    }
                }
            }
            Callee::Super(_) => return Vec::new(), // Super calls don't have generics
            Callee::Import(_) => return Vec::new(), // Import calls don't have generics
        };

        // Find the end of the callee (where the function name ends)
        let callee_end = callee_span.hi.0 as usize;

        // Look for '<' after the callee
        if callee_end >= source.len() {
            return Vec::new();
        }

        let after_callee = &source[callee_end..];
        let mut chars = after_callee.chars().peekable();

        // Skip whitespace
        while let Some(&ch) = chars.peek() {
            if ch.is_whitespace() {
                chars.next();
            } else {
                break;
            }
        }

        // Check if next character is '<'
        if chars.peek() != Some(&'<') {
            return Vec::new(); // No generic parameters
        }
        chars.next(); // Skip '<'

        // Parse generic parameters
        let mut params = Vec::new();
        let mut current_param = String::new();
        let mut depth = 1; // Track nesting level for nested generics like Promise<Array<T>>
        let mut in_string = false;
        let mut string_char = None;

        for ch in chars {
            match ch {
                '<' if !in_string => {
                    depth += 1;
                    current_param.push(ch);
                }
                '>' if !in_string => {
                    depth -= 1;
                    if depth == 0 {
                        // End of generic parameters
                        if !current_param.trim().is_empty() {
                            params.push(current_param.trim().to_string());
                        }
                        break;
                    }
                    current_param.push(ch);
                }
                ',' if !in_string && depth == 1 => {
                    // Parameter separator at top level
                    if !current_param.trim().is_empty() {
                        params.push(current_param.trim().to_string());
                    }
                    current_param.clear();
                }
                '"' | '\'' if !in_string => {
                    in_string = true;
                    string_char = Some(ch);
                    current_param.push(ch);
                }
                ch if in_string && Some(ch) == string_char => {
                    in_string = false;
                    string_char = None;
                    current_param.push(ch);
                }
                _ => {
                    current_param.push(ch);
                }
            }
        }

        params
    }

    /// Converts a parsed type name string to TypeInfo
    /// Handles simple types, qualified names, generics, unions, etc.
    fn parse_type_name_to_type_info(
        &self,
        type_name: &str,
        file_path: &str,
        line: usize,
    ) -> Result<crate::models::TypeInfo> {
        let mut type_name = type_name.trim().to_string();

        // Handle union types: take the first type
        if let Some(pipe_pos) = type_name.find('|') {
            type_name = type_name[..pipe_pos].trim().to_string();
        }

        // Handle intersection types: take the first type
        if let Some(amp_pos) = type_name.find('&') {
            type_name = type_name[..amp_pos].trim().to_string();
        }

        // Handle arrays: UserResponse[] -> UserResponse
        let is_array = type_name.ends_with("[]");
        if is_array {
            type_name = type_name[..type_name.len() - 2].trim().to_string();
        }

        // Handle optional: UserResponse? -> UserResponse
        let is_optional = type_name.ends_with('?');
        if is_optional {
            type_name = type_name[..type_name.len() - 1].trim().to_string();
        }

        // Handle generic types: Promise<UserResponse> -> UserResponse
        type_name = self.extract_inner_type_from_generic(&type_name);

        // Handle qualified names: api.UserResponse -> UserResponse
        let final_type_name = self.parse_qualified_name(&type_name);

        // Create SchemaReference
        let schema_ref = Some(crate::models::SchemaReference {
            name: final_type_name.clone(),
            schema_type: crate::models::SchemaType::TypeScript,
            location: crate::models::Location {
                file: file_path.to_string(),
                line,
                column: None,
            },
            metadata: std::collections::HashMap::new(),
        });

        // Determine base type
        let base_type = if is_array {
            crate::models::BaseType::Array
        } else {
            crate::models::BaseType::Object
        };

        Ok(crate::models::TypeInfo {
            base_type,
            schema_ref,
            constraints: Vec::new(),
            optional: is_optional,
        })
    }

    /// Extracts inner type from generic type like Promise<T>, Array<T>
    fn extract_inner_type_from_generic(&self, type_name: &str) -> String {
        // Look for pattern: TypeName<InnerType>
        if let Some(open_pos) = type_name.find('<') {
            if let Some(close_pos) = type_name.rfind('>') {
                if close_pos > open_pos {
                    // Extract inner type, handling nested generics
                    let inner = &type_name[open_pos + 1..close_pos];
                    // Find the last '>' that matches the first '<'
                    // For now, simple approach: take everything between first < and last >
                    return inner.trim().to_string();
                }
            }
        }
        type_name.to_string()
    }

    /// Parses qualified name and returns the simple name
    /// api.UserResponse -> UserResponse
    fn parse_qualified_name(&self, type_name: &str) -> String {
        if let Some(last_dot) = type_name.rfind('.') {
            type_name[last_dot + 1..].to_string()
        } else {
            type_name.to_string()
        }
    }

    /// Extracts call arguments
    fn extract_call_arguments(&self, call_expr: &CallExpr) -> Vec<CallArgument> {
        let mut args = Vec::new();

        for arg in &call_expr.args {
            let value = self.expr_to_string(&arg.expr);
            args.push(CallArgument {
                parameter_name: None,
                value,
            });
        }

        args
    }

    /// Converts Expression to string
    fn expr_to_string(&self, expr: &Expr) -> String {
        match expr {
            Expr::Ident(ident) => ident.sym.as_ref().to_string(),
            Expr::Member(member_expr) => {
                let base = self.expr_to_string(member_expr.obj.as_ref());
                let prop = match &member_expr.prop {
                    MemberProp::Ident(ident) => ident.sym.as_ref().to_string(),
                    MemberProp::Computed(computed) => {
                        if let Expr::Lit(Lit::Str(str)) = computed.expr.as_ref() {
                            format!("[{}]", str.value.as_str().unwrap_or(""))
                        } else {
                            "[...]".to_string()
                        }
                    }
                    _ => "?".to_string(),
                };
                format!("{}.{}", base, prop)
            }
            Expr::Lit(lit) => match lit {
                Lit::Str(str) => format!("\"{}\"", str.value.as_str().unwrap_or("")),
                Lit::Num(num) => num.value.to_string(),
                Lit::Bool(b) => format!("{}", b.value),
                Lit::Null(_) => "null".to_string(),
                _ => "lit".to_string(),
            },
            Expr::Call(call) => {
                if let Some(name) = self.call_name(&call.callee) {
                    format!("{}(...)", name)
                } else {
                    "call(...)".to_string()
                }
            }
            _ => "expr".to_string(),
        }
    }

    /// Extracts Zod schemas from module
    pub fn extract_zod_schemas(
        &self,
        module: &Module,
        file_path: &str,
        converter: &LocationConverter,
    ) -> Vec<SchemaReference> {
        let mut schemas = Vec::new();

        // First extract TypeScript schemas to link with Zod
        let ts_schemas = self.extract_typescript_schemas(module, file_path, converter);
        let mut ts_schema_map = std::collections::HashMap::new();
        for ts_schema in &ts_schemas {
            ts_schema_map.insert(ts_schema.name.clone(), ts_schema.clone());
        }

        for item in &module.body {
            self.walk_for_zod(item, &mut schemas, file_path, converter, &ts_schema_map);
        }

        schemas
    }

    /// Traverses AST to find Zod schemas
    fn walk_for_zod(
        &self,
        item: &ModuleItem,
        schemas: &mut Vec<SchemaReference>,
        file_path: &str,
        converter: &LocationConverter,
        ts_schema_map: &std::collections::HashMap<String, SchemaReference>,
    ) {
        match item {
            ModuleItem::Stmt(Stmt::Decl(Decl::Var(var_decl))) => {
                for decl in &var_decl.decls {
                    if let Some(init) = &decl.init {
                        if let Expr::Call(call_expr) = init.as_ref() {
                            if let Callee::Expr(callee_expr) = &call_expr.callee {
                                if self.is_zod_call(callee_expr.as_ref()) {
                                    let span = call_expr.span;
                                    let (line, column) =
                                        converter.byte_offset_to_location(span.lo.0 as usize);

                                    let schema_name = match &decl.name {
                                        Pat::Ident(ident) => ident.id.sym.as_ref().to_string(),
                                        _ => "ZodSchema".to_string(),
                                    };

                                    let mut metadata = std::collections::HashMap::new();

                                    // Try to find associated TypeScript type
                                    if let Some(ts_schema) = ts_schema_map.get(&schema_name) {
                                        // Link Zod schema with TypeScript type
                                        metadata.insert(
                                            "typescript_type".to_string(),
                                            ts_schema.name.clone(),
                                        );
                                        // Copy fields from TypeScript schema if present
                                        if let Some(fields) = ts_schema.metadata.get("fields") {
                                            metadata.insert("fields".to_string(), fields.clone());
                                        }
                                    }

                                    // Check if this is z.object() and extract fields
                                    if self.is_zod_object_call(callee_expr.as_ref()) {
                                        let fields = self.extract_zod_object_fields(call_expr);
                                        if !fields.is_empty() {
                                            // Store fields as JSON
                                            if let Ok(fields_json) = serde_json::to_string(&fields)
                                            {
                                                metadata.insert("fields".to_string(), fields_json);
                                            }
                                        }
                                    }

                                    schemas.push(SchemaReference {
                                        name: schema_name,
                                        schema_type: SchemaType::Zod,
                                        location: Location {
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
                }
            }
            ModuleItem::Stmt(Stmt::Expr(expr_stmt)) => {
                if let Expr::Call(call_expr) = expr_stmt.expr.as_ref() {
                    if let Callee::Expr(callee_expr) = &call_expr.callee {
                        if self.is_zod_call(callee_expr.as_ref()) {
                            let span = call_expr.span;
                            let (line, column) =
                                converter.byte_offset_to_location(span.lo.0 as usize);

                            let mut metadata = std::collections::HashMap::new();

                            // Check if this is z.object() and extract fields
                            if self.is_zod_object_call(callee_expr.as_ref()) {
                                let fields = self.extract_zod_object_fields(call_expr);
                                if !fields.is_empty() {
                                    // Store fields as JSON
                                    if let Ok(fields_json) = serde_json::to_string(&fields) {
                                        metadata.insert("fields".to_string(), fields_json);
                                    }
                                }
                            }

                            schemas.push(SchemaReference {
                                name: "ZodSchema".to_string(),
                                schema_type: SchemaType::Zod,
                                location: Location {
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
            _ => {}
        }
    }

    /// Checks if expression is a Zod call
    fn is_zod_call(&self, expr: &Expr) -> bool {
        if let Expr::Member(member_expr) = expr {
            if let Expr::Ident(ident) = member_expr.obj.as_ref() {
                if ident.sym.as_ref() == "z" {
                    if let MemberProp::Ident(prop) = &member_expr.prop {
                        let method = prop.sym.as_ref();
                        return method == "object"
                            || method == "string"
                            || method == "number"
                            || method == "boolean"
                            || method == "array";
                    }
                }
            }
        }
        false
    }

    /// Checks if expression is specifically z.object() call
    fn is_zod_object_call(&self, expr: &Expr) -> bool {
        if let Expr::Member(member_expr) = expr {
            if let Expr::Ident(ident) = member_expr.obj.as_ref() {
                if ident.sym.as_ref() == "z" {
                    if let MemberProp::Ident(prop) = &member_expr.prop {
                        return prop.sym.as_ref() == "object";
                    }
                }
            }
        }
        false
    }

    /// Extracts Zod type from an expression
    /// Handles: z.string(), z.number(), z.boolean(), z.array(), z.object(), etc.
    /// Also handles chained methods: z.string().email(), z.number().min(1), etc.
    #[allow(clippy::only_used_in_recursion)]
    fn extract_zod_type(&self, expr: &Expr) -> String {
        if let Expr::Call(call) = expr {
            if let Callee::Expr(callee) = &call.callee {
                if let Expr::Member(member) = callee.as_ref() {
                    if let MemberProp::Ident(prop) = &member.prop {
                        let method = prop.sym.as_ref();

                        // Check if this is a Zod call (z.string, z.number, etc.)
                        if let Expr::Ident(ident) = member.obj.as_ref() {
                            if ident.sym.as_ref() == "z" {
                                // Base type: string, number, boolean, array, object
                                return method.to_string();
                            }
                        }

                        // If not a direct Zod call, might be a chained method
                        // Recursively check the base expression
                        return self.extract_zod_type(member.obj.as_ref());
                    }
                }
            }
        }

        "unknown".to_string()
    }

    /// Checks if a Zod expression is optional (has .optional() or .nullable())
    /// Handles chains like: z.string().optional(), z.number().nullable(), etc.
    #[allow(clippy::only_used_in_recursion)]
    fn is_zod_optional(&self, expr: &Expr) -> (bool, bool) {
        if let Expr::Call(call) = expr {
            if let Callee::Expr(callee) = &call.callee {
                if let Expr::Member(member) = callee.as_ref() {
                    if let MemberProp::Ident(prop) = &member.prop {
                        let method = prop.sym.as_ref();

                        // Check for .optional() or .nullable()
                        if method == "optional" {
                            // Recursively check the base expression
                            let (_, nullable) = self.is_zod_optional(member.obj.as_ref());
                            return (true, nullable);
                        } else if method == "nullable" {
                            // Recursively check the base expression
                            let (optional, _) = self.is_zod_optional(member.obj.as_ref());
                            return (optional, true);
                        } else {
                            // Continue checking the chain
                            return self.is_zod_optional(member.obj.as_ref());
                        }
                    }
                }
            }
        }

        (false, false)
    }

    /// Extracts fields from z.object({...}) call
    /// Example: z.object({ name: z.string(), age: z.number().optional() })
    fn extract_zod_object_fields(&self, call_expr: &CallExpr) -> Vec<crate::models::ZodField> {
        let mut fields = Vec::new();

        // Get the first argument (the object literal)
        if let Some(arg) = call_expr.args.first() {
            if let Expr::Object(obj_lit) = arg.expr.as_ref() {
                for prop in &obj_lit.props {
                    if let PropOrSpread::Prop(prop_box) = prop {
                        if let Prop::KeyValue(key_value) = prop_box.as_ref() {
                            // Extract field name
                            let field_name = match &key_value.key {
                                PropName::Ident(ident) => ident.sym.as_ref().to_string(),
                                PropName::Str(str_lit) => {
                                    str_lit.value.as_str().unwrap_or("").to_string()
                                }
                                _ => continue, // Skip other property name types
                            };

                            // Extract field type and optionality
                            let field_type = self.extract_zod_type(key_value.value.as_ref());
                            let (is_optional, is_nullable) =
                                self.is_zod_optional(key_value.value.as_ref());

                            fields.push(crate::models::ZodField {
                                name: field_name,
                                type_name: field_type,
                                optional: is_optional,
                                nullable: is_nullable,
                            });
                        }
                    }
                }
            }
        }

        fields
    }

    /// Returns true if the given TypeScript file looks like it was generated from OpenAPI
    /// (types.gen.ts, *.gen.ts, or located in openapi-client/generated/sdk directories).
    fn is_generated_types_file(file_path: &str) -> bool {
        let lower = file_path.to_lowercase();

        // Explicit generated file patterns
        if lower.ends_with("types.gen.ts")
            || lower.ends_with(".gen.ts")
            || lower.ends_with(".gen.tsx")
        {
            return true;
        }

        // Directory-based heuristics
        // We don't have Path here, so use simple substring checks.
        if lower.contains("/openapi-client/")
            || lower.contains("\\openapi-client\\")
            || lower.contains("/generated/")
            || lower.contains("\\generated\\")
            || lower.contains("/sdk/")
            || lower.contains("\\sdk\\")
        {
            return true;
        }

        false
    }

    /// Extracts TypeScript types from module
    pub fn extract_types(
        &self,
        module: &Module,
        file_path: &str,
        converter: &LocationConverter,
    ) -> Vec<TypeInfo> {
        let mut types = Vec::new();

        for item in &module.body {
            self.walk_for_types(item, &mut types, file_path, converter);
        }

        types
    }

    /// Extracts TypeScript schemas (interfaces and type aliases) from module
    pub fn extract_typescript_schemas(
        &self,
        module: &Module,
        file_path: &str,
        converter: &LocationConverter,
    ) -> Vec<SchemaReference> {
        let mut schemas = Vec::new();

        for item in &module.body {
            self.walk_for_typescript_schemas(item, &mut schemas, file_path, converter);
        }

        schemas
    }

    /// Traverses AST to find TypeScript types
    fn walk_for_types(
        &self,
        item: &ModuleItem,
        types: &mut Vec<TypeInfo>,
        file_path: &str,
        converter: &LocationConverter,
    ) {
        match item {
            ModuleItem::ModuleDecl(ModuleDecl::ExportDecl(export_decl)) => {
                match &export_decl.decl {
                    Decl::TsInterface(ts_interface) => {
                        let span = ts_interface.span;
                        let (line, column) = converter.byte_offset_to_location(span.lo.0 as usize);

                        let name = ts_interface.id.sym.as_ref().to_string();
                        let base_type = crate::models::BaseType::Object;

                        // Extract interface properties
                        let mut metadata = std::collections::HashMap::new();
                        let mut fields = Vec::new();

                        for member in &ts_interface.body.body {
                            if let swc_ecma_ast::TsTypeElement::TsPropertySignature(prop) = member {
                                let field_name = self.ts_property_key_to_string(&prop.key);
                                if let Some(type_ann) = &prop.type_ann {
                                    let field_type = self.ts_type_ann_to_string(type_ann);
                                    fields.push(format!("{}:{}", field_name, field_type));
                                }
                            }
                        }

                        if !fields.is_empty() {
                            metadata.insert("fields".to_string(), fields.join(","));
                        }

                        // Mark schemas coming from generated files
                        if Self::is_generated_types_file(file_path) {
                            metadata.insert("openapi_generated".to_string(), "true".to_string());
                            metadata.insert(
                                "openapi_generated_from".to_string(),
                                file_path.to_string(),
                            );
                        }

                        let schema_ref = SchemaReference {
                            name: name.clone(),
                            schema_type: SchemaType::TypeScript,
                            location: Location {
                                file: file_path.to_string(),
                                line,
                                column: Some(column),
                            },
                            metadata,
                        };

                        types.push(TypeInfo {
                            base_type,
                            schema_ref: Some(schema_ref),
                            constraints: Vec::new(),
                            optional: false,
                        });
                    }
                    Decl::TsTypeAlias(ts_type_alias) => {
                        let span = ts_type_alias.span;
                        let (line, column) = converter.byte_offset_to_location(span.lo.0 as usize);

                        let name = ts_type_alias.id.sym.as_ref().to_string();
                        let base_type = self.ts_type_to_base_type(ts_type_alias.type_ann.as_ref());

                        let mut metadata = std::collections::HashMap::new();

                        // Mark schemas coming from generated files
                        if Self::is_generated_types_file(file_path) {
                            metadata.insert("openapi_generated".to_string(), "true".to_string());
                            metadata.insert(
                                "openapi_generated_from".to_string(),
                                file_path.to_string(),
                            );
                        }

                        let schema_ref = SchemaReference {
                            name: name.clone(),
                            schema_type: SchemaType::TypeScript,
                            location: Location {
                                file: file_path.to_string(),
                                line,
                                column: Some(column),
                            },
                            metadata,
                        };

                        types.push(TypeInfo {
                            base_type,
                            schema_ref: Some(schema_ref),
                            constraints: Vec::new(),
                            optional: false,
                        });
                    }
                    _ => {}
                }
            }
            ModuleItem::Stmt(Stmt::Decl(Decl::TsInterface(ts_interface))) => {
                let span = ts_interface.span;
                let (line, column) = converter.byte_offset_to_location(span.lo.0 as usize);

                let name = ts_interface.id.sym.as_ref().to_string();
                let base_type = crate::models::BaseType::Object;

                let mut metadata = std::collections::HashMap::new();
                let mut fields = Vec::new();

                for member in &ts_interface.body.body {
                    if let swc_ecma_ast::TsTypeElement::TsPropertySignature(prop) = member {
                        let field_name = self.ts_property_key_to_string(&prop.key);
                        if let Some(type_ann) = &prop.type_ann {
                            let field_type = self.ts_type_ann_to_string(type_ann);
                            fields.push(format!("{}:{}", field_name, field_type));
                        }
                    }
                }

                if !fields.is_empty() {
                    metadata.insert("fields".to_string(), fields.join(","));
                }

                // Mark schemas coming from generated files
                if Self::is_generated_types_file(file_path) {
                    metadata.insert("openapi_generated".to_string(), "true".to_string());
                    metadata.insert("openapi_generated_from".to_string(), file_path.to_string());
                }

                let schema_ref = SchemaReference {
                    name: name.clone(),
                    schema_type: SchemaType::TypeScript,
                    location: Location {
                        file: file_path.to_string(),
                        line,
                        column: Some(column),
                    },
                    metadata,
                };

                types.push(TypeInfo {
                    base_type,
                    schema_ref: Some(schema_ref),
                    constraints: Vec::new(),
                    optional: false,
                });
            }
            ModuleItem::Stmt(Stmt::Decl(Decl::TsTypeAlias(ts_type_alias))) => {
                let span = ts_type_alias.span;
                let (line, column) = converter.byte_offset_to_location(span.lo.0 as usize);

                let name = ts_type_alias.id.sym.as_ref().to_string();
                let base_type = self.ts_type_to_base_type(ts_type_alias.type_ann.as_ref());

                let mut metadata = std::collections::HashMap::new();

                // Mark schemas coming from generated files
                if Self::is_generated_types_file(file_path) {
                    metadata.insert("openapi_generated".to_string(), "true".to_string());
                    metadata.insert("openapi_generated_from".to_string(), file_path.to_string());
                }

                let schema_ref = SchemaReference {
                    name: name.clone(),
                    schema_type: SchemaType::TypeScript,
                    location: Location {
                        file: file_path.to_string(),
                        line,
                        column: Some(column),
                    },
                    metadata,
                };

                types.push(TypeInfo {
                    base_type,
                    schema_ref: Some(schema_ref),
                    constraints: Vec::new(),
                    optional: false,
                });
            }
            _ => {}
        }
    }

    /// Traverses AST to find TypeScript schemas
    fn walk_for_typescript_schemas(
        &self,
        item: &ModuleItem,
        schemas: &mut Vec<SchemaReference>,
        file_path: &str,
        converter: &LocationConverter,
    ) {
        match item {
            ModuleItem::ModuleDecl(ModuleDecl::ExportDecl(export_decl)) => {
                match &export_decl.decl {
                    Decl::TsInterface(ts_interface) => {
                        let span = ts_interface.span;
                        let (line, column) = converter.byte_offset_to_location(span.lo.0 as usize);

                        let name = ts_interface.id.sym.as_ref().to_string();
                        let mut metadata = std::collections::HashMap::new();
                        let mut fields = Vec::new();

                        for member in &ts_interface.body.body {
                            if let swc_ecma_ast::TsTypeElement::TsPropertySignature(prop) = member {
                                let field_name = self.ts_property_key_to_string(&prop.key);
                                if let Some(type_ann) = &prop.type_ann {
                                    let field_type = self.ts_type_ann_to_string(type_ann);
                                    let optional = prop.optional;
                                    fields.push(format!(
                                        "{}:{}:{}",
                                        field_name,
                                        field_type,
                                        if optional { "optional" } else { "required" }
                                    ));
                                }
                            }
                        }

                        if !fields.is_empty() {
                            metadata.insert("fields".to_string(), fields.join(","));
                        }

                        // Mark schemas coming from generated files
                        if Self::is_generated_types_file(file_path) {
                            metadata.insert("openapi_generated".to_string(), "true".to_string());
                            metadata.insert(
                                "openapi_generated_from".to_string(),
                                file_path.to_string(),
                            );
                        }

                        schemas.push(SchemaReference {
                            name,
                            schema_type: SchemaType::TypeScript,
                            location: Location {
                                file: file_path.to_string(),
                                line,
                                column: Some(column),
                            },
                            metadata,
                        });
                    }
                    Decl::TsTypeAlias(ts_type_alias) => {
                        let span = ts_type_alias.span;
                        let (line, column) = converter.byte_offset_to_location(span.lo.0 as usize);

                        let name = ts_type_alias.id.sym.as_ref().to_string();
                        let type_str = self.ts_type_to_string(ts_type_alias.type_ann.as_ref());

                        let mut metadata = std::collections::HashMap::new();
                        metadata.insert("type".to_string(), type_str);

                        // Mark schemas coming from generated files
                        if Self::is_generated_types_file(file_path) {
                            metadata.insert("openapi_generated".to_string(), "true".to_string());
                            metadata.insert(
                                "openapi_generated_from".to_string(),
                                file_path.to_string(),
                            );
                        }

                        schemas.push(SchemaReference {
                            name,
                            schema_type: SchemaType::TypeScript,
                            location: Location {
                                file: file_path.to_string(),
                                line,
                                column: Some(column),
                            },
                            metadata,
                        });
                    }
                    _ => {}
                }
            }
            ModuleItem::Stmt(Stmt::Decl(Decl::TsInterface(ts_interface))) => {
                let span = ts_interface.span;
                let (line, column) = converter.byte_offset_to_location(span.lo.0 as usize);

                let name = ts_interface.id.sym.as_ref().to_string();
                let mut metadata = std::collections::HashMap::new();
                let mut fields = Vec::new();

                for member in &ts_interface.body.body {
                    if let swc_ecma_ast::TsTypeElement::TsPropertySignature(prop) = member {
                        let field_name = self.ts_property_key_to_string(&prop.key);
                        if let Some(type_ann) = &prop.type_ann {
                            let field_type = self.ts_type_ann_to_string(type_ann);
                            let optional = prop.optional;
                            fields.push(format!(
                                "{}:{}:{}",
                                field_name,
                                field_type,
                                if optional { "optional" } else { "required" }
                            ));
                        }
                    }
                }

                if !fields.is_empty() {
                    metadata.insert("fields".to_string(), fields.join(","));
                }

                // Mark schemas coming from generated files
                if Self::is_generated_types_file(file_path) {
                    metadata.insert("openapi_generated".to_string(), "true".to_string());
                    metadata.insert("openapi_generated_from".to_string(), file_path.to_string());
                }

                schemas.push(SchemaReference {
                    name,
                    schema_type: SchemaType::TypeScript,
                    location: Location {
                        file: file_path.to_string(),
                        line,
                        column: Some(column),
                    },
                    metadata,
                });
            }
            ModuleItem::Stmt(Stmt::Decl(Decl::TsTypeAlias(ts_type_alias))) => {
                let span = ts_type_alias.span;
                let (line, column) = converter.byte_offset_to_location(span.lo.0 as usize);

                let name = ts_type_alias.id.sym.as_ref().to_string();
                let type_str = self.ts_type_to_string(ts_type_alias.type_ann.as_ref());

                let mut metadata = std::collections::HashMap::new();
                metadata.insert("type".to_string(), type_str);

                // Mark schemas coming from generated files
                if Self::is_generated_types_file(file_path) {
                    metadata.insert("openapi_generated".to_string(), "true".to_string());
                    metadata.insert("openapi_generated_from".to_string(), file_path.to_string());
                }

                schemas.push(SchemaReference {
                    name,
                    schema_type: SchemaType::TypeScript,
                    location: Location {
                        file: file_path.to_string(),
                        line,
                        column: Some(column),
                    },
                    metadata,
                });
            }
            _ => {}
        }
    }

    /// Converts TypeScript type annotation to string
    fn ts_type_ann_to_string(&self, ts_type_ann: &swc_ecma_ast::TsTypeAnn) -> String {
        self.ts_type_to_string(&ts_type_ann.type_ann)
    }

    /// Converts TypeScript type to string
    fn ts_type_to_string(&self, ts_type: &swc_ecma_ast::TsType) -> String {
        match ts_type {
            swc_ecma_ast::TsType::TsKeywordType(keyword) => match keyword.kind {
                swc_ecma_ast::TsKeywordTypeKind::TsStringKeyword => "string".to_string(),
                swc_ecma_ast::TsKeywordTypeKind::TsNumberKeyword => "number".to_string(),
                swc_ecma_ast::TsKeywordTypeKind::TsBooleanKeyword => "boolean".to_string(),
                swc_ecma_ast::TsKeywordTypeKind::TsAnyKeyword => "any".to_string(),
                swc_ecma_ast::TsKeywordTypeKind::TsUnknownKeyword => "unknown".to_string(),
                swc_ecma_ast::TsKeywordTypeKind::TsVoidKeyword => "void".to_string(),
                swc_ecma_ast::TsKeywordTypeKind::TsNullKeyword => "null".to_string(),
                swc_ecma_ast::TsKeywordTypeKind::TsUndefinedKeyword => "undefined".to_string(),
                swc_ecma_ast::TsKeywordTypeKind::TsNeverKeyword => "never".to_string(),
                _ => "unknown".to_string(),
            },
            swc_ecma_ast::TsType::TsTypeRef(type_ref) => match &type_ref.type_name {
                swc_ecma_ast::TsEntityName::Ident(ident) => ident.sym.as_ref().to_string(),
                swc_ecma_ast::TsEntityName::TsQualifiedName(qualified) => {
                    format!(
                        "{}.{}",
                        self.ts_entity_name_to_string(&qualified.left),
                        qualified.right.sym.as_ref()
                    )
                }
            },
            swc_ecma_ast::TsType::TsArrayType(array_type) => {
                format!(
                    "{}[]",
                    self.ts_type_to_string(array_type.elem_type.as_ref())
                )
            }
            swc_ecma_ast::TsType::TsUnionOrIntersectionType(union) => {
                // In SWC 18.0 structure may differ, use match on type
                match union {
                    swc_ecma_ast::TsUnionOrIntersectionType::TsUnionType(union_type) => {
                        let types: Vec<String> = union_type
                            .types
                            .iter()
                            .map(|t| self.ts_type_to_string(t))
                            .collect();
                        types.join(" | ")
                    }
                    swc_ecma_ast::TsUnionOrIntersectionType::TsIntersectionType(
                        intersection_type,
                    ) => {
                        let types: Vec<String> = intersection_type
                            .types
                            .iter()
                            .map(|t| self.ts_type_to_string(t))
                            .collect();
                        types.join(" & ")
                    }
                }
            }
            _ => "unknown".to_string(),
        }
    }

    /// Converts TypeScript type to BaseType
    fn ts_type_to_base_type(&self, ts_type: &swc_ecma_ast::TsType) -> crate::models::BaseType {
        match ts_type {
            swc_ecma_ast::TsType::TsKeywordType(keyword) => match keyword.kind {
                swc_ecma_ast::TsKeywordTypeKind::TsStringKeyword => crate::models::BaseType::String,
                swc_ecma_ast::TsKeywordTypeKind::TsNumberKeyword => crate::models::BaseType::Number,
                swc_ecma_ast::TsKeywordTypeKind::TsBooleanKeyword => {
                    crate::models::BaseType::Boolean
                }
                swc_ecma_ast::TsKeywordTypeKind::TsAnyKeyword => crate::models::BaseType::Any,
                _ => crate::models::BaseType::Unknown,
            },
            swc_ecma_ast::TsType::TsArrayType(_) => crate::models::BaseType::Array,
            swc_ecma_ast::TsType::TsTypeRef(_) => crate::models::BaseType::Object,
            _ => crate::models::BaseType::Unknown,
        }
    }

    /// Converts TsEntityName to string
    #[allow(clippy::only_used_in_recursion)]
    fn ts_entity_name_to_string(&self, entity_name: &swc_ecma_ast::TsEntityName) -> String {
        match entity_name {
            swc_ecma_ast::TsEntityName::Ident(ident) => ident.sym.as_ref().to_string(),
            swc_ecma_ast::TsEntityName::TsQualifiedName(qualified) => {
                format!(
                    "{}.{}",
                    self.ts_entity_name_to_string(&qualified.left),
                    qualified.right.sym.as_ref()
                )
            }
        }
    }

    /// Converts property key to string
    fn ts_property_key_to_string(&self, key: &swc_ecma_ast::Expr) -> String {
        match key {
            Expr::Ident(ident) => ident.sym.as_ref().to_string(),
            Expr::Lit(Lit::Str(str)) => str.value.as_str().unwrap_or("").to_string(),
            _ => "unknown".to_string(),
        }
    }

    /// Extracts functions and classes from module
    pub fn extract_functions_and_classes(
        &self,
        module: &Module,
        file_path: &str,
        converter: &LocationConverter,
    ) -> Vec<FunctionOrClass> {
        let mut result = Vec::new();

        for item in &module.body {
            self.walk_for_functions_and_classes(item, &mut result, file_path, converter);
        }

        result
    }

    /// Traverses AST to find functions and classes
    fn walk_for_functions_and_classes(
        &self,
        item: &ModuleItem,
        result: &mut Vec<FunctionOrClass>,
        file_path: &str,
        converter: &LocationConverter,
    ) {
        match item {
            ModuleItem::Stmt(Stmt::Decl(Decl::Fn(fn_decl))) => {
                let span = fn_decl.ident.span;
                let (line, column) = converter.byte_offset_to_location(span.lo.0 as usize);

                let name = fn_decl.ident.sym.as_ref().to_string();
                let parameters = self.extract_function_parameters(&fn_decl.function);
                let return_type = self.extract_return_type(&fn_decl.function);
                let is_async = fn_decl.function.is_async;

                result.push(FunctionOrClass::Function {
                    name,
                    line,
                    column,
                    parameters,
                    return_type,
                    is_async,
                });
            }
            ModuleItem::Stmt(Stmt::Decl(Decl::Class(class_decl))) => {
                let span = class_decl.ident.span;
                let (line, column) = converter.byte_offset_to_location(span.lo.0 as usize);

                let name = class_decl.ident.sym.as_ref().to_string();
                let methods = self.extract_class_methods(&class_decl.class, file_path, converter);

                result.push(FunctionOrClass::Class {
                    name,
                    line,
                    column,
                    methods,
                });
            }
            ModuleItem::ModuleDecl(ModuleDecl::ExportDecl(export_decl)) => {
                match &export_decl.decl {
                    Decl::Fn(fn_decl) => {
                        let span = fn_decl.ident.span;
                        let (line, column) = converter.byte_offset_to_location(span.lo.0 as usize);

                        let name = fn_decl.ident.sym.as_ref().to_string();
                        let parameters = self.extract_function_parameters(&fn_decl.function);
                        let return_type = self.extract_return_type(&fn_decl.function);
                        let is_async = fn_decl.function.is_async;

                        result.push(FunctionOrClass::Function {
                            name,
                            line,
                            column,
                            parameters,
                            return_type,
                            is_async,
                        });
                    }
                    Decl::Class(class_decl) => {
                        let span = class_decl.ident.span;
                        let (line, column) = converter.byte_offset_to_location(span.lo.0 as usize);

                        let name = class_decl.ident.sym.as_ref().to_string();
                        let methods =
                            self.extract_class_methods(&class_decl.class, file_path, converter);

                        result.push(FunctionOrClass::Class {
                            name,
                            line,
                            column,
                            methods,
                        });
                    }
                    _ => {}
                }
            }
            ModuleItem::Stmt(Stmt::Decl(Decl::Var(var_decl))) => {
                // Handle const fn = () => {} and similar
                for decl in &var_decl.decls {
                    if let Some(init) = &decl.init {
                        if let Expr::Arrow(arrow_fn) = init.as_ref() {
                            if let Pat::Ident(ident) = &decl.name {
                                let name = ident.id.sym.as_ref().to_string();
                                let span = arrow_fn.span;
                                let (line, column) =
                                    converter.byte_offset_to_location(span.lo.0 as usize);

                                let parameters = self.extract_arrow_function_parameters(arrow_fn);
                                let return_type = self.extract_arrow_return_type(arrow_fn);

                                result.push(FunctionOrClass::Function {
                                    name,
                                    line,
                                    column,
                                    parameters,
                                    return_type,
                                    is_async: arrow_fn.is_async,
                                });
                            }
                        }
                    }
                }
            }
            _ => {}
        }
    }

    /// Finds a function in module by name (supports all function types)
    ///
    /// Searches for:
    /// - Regular function declarations
    /// - Arrow functions (const/let)
    /// - IIFE (Immediately Invoked Function Expression)
    /// - Class methods
    /// - Export functions
    pub fn find_function_by_name(
        &self,
        module: &Module,
        function_name: &str,
        file_path: &str,
        converter: &LocationConverter,
    ) -> Option<FunctionInfo> {
        let mut result = None;

        for item in &module.body {
            self.walk_for_function_by_name(item, function_name, &mut result, file_path, converter);
        }

        result
    }

    /// Traverses AST to find a function by name
    fn walk_for_function_by_name(
        &self,
        item: &ModuleItem,
        function_name: &str,
        result: &mut Option<FunctionInfo>,
        file_path: &str,
        converter: &LocationConverter,
    ) {
        match item {
            // 1. Regular function declarations
            ModuleItem::Stmt(Stmt::Decl(Decl::Fn(fn_decl))) => {
                if fn_decl.ident.sym.as_ref() == function_name {
                    let span = fn_decl.ident.span;
                    let (line, column) = converter.byte_offset_to_location(span.lo.0 as usize);
                    let parameters = self.extract_function_parameters(&fn_decl.function);
                    let return_type = self.extract_return_type(&fn_decl.function);

                    *result = Some(FunctionInfo {
                        name: function_name.to_string(),
                        parameters,
                        return_type,
                        is_async: fn_decl.function.is_async,
                        location: crate::models::Location {
                            file: file_path.to_string(),
                            line,
                            column: Some(column),
                        },
                    });
                }
            }

            // 2. Export functions
            ModuleItem::ModuleDecl(ModuleDecl::ExportDecl(export_decl)) => {
                if let Decl::Fn(fn_decl) = &export_decl.decl {
                    if fn_decl.ident.sym.as_ref() == function_name {
                        let span = fn_decl.ident.span;
                        let (line, column) = converter.byte_offset_to_location(span.lo.0 as usize);
                        let parameters = self.extract_function_parameters(&fn_decl.function);
                        let return_type = self.extract_return_type(&fn_decl.function);

                        *result = Some(FunctionInfo {
                            name: function_name.to_string(),
                            parameters,
                            return_type,
                            is_async: fn_decl.function.is_async,
                            location: crate::models::Location {
                                file: file_path.to_string(),
                                line,
                                column: Some(column),
                            },
                        });
                    }
                }
            }

            // 3. Arrow functions (const/let)
            ModuleItem::Stmt(Stmt::Decl(Decl::Var(var_decl))) => {
                for decl in &var_decl.decls {
                    if let Pat::Ident(ident) = &decl.name {
                        if ident.id.sym.as_ref() == function_name {
                            if let Some(init) = &decl.init {
                                if let Expr::Arrow(arrow_fn) = init.as_ref() {
                                    let span = arrow_fn.span;
                                    let (line, column) =
                                        converter.byte_offset_to_location(span.lo.0 as usize);
                                    let parameters =
                                        self.extract_arrow_function_parameters(arrow_fn);
                                    let return_type = self.extract_arrow_return_type(arrow_fn);

                                    *result = Some(FunctionInfo {
                                        name: function_name.to_string(),
                                        parameters,
                                        return_type,
                                        is_async: arrow_fn.is_async,
                                        location: crate::models::Location {
                                            file: file_path.to_string(),
                                            line,
                                            column: Some(column),
                                        },
                                    });
                                }
                            }
                        }
                    }
                }
            }

            // 4. IIFE - check in expressions
            ModuleItem::Stmt(Stmt::Expr(expr_stmt)) => {
                self.walk_expr_for_iife(expr_stmt, function_name, result, file_path, converter);
            }

            // 5. Class methods
            ModuleItem::Stmt(Stmt::Decl(Decl::Class(class_decl))) => {
                self.walk_class_for_method(class_decl, function_name, result, file_path, converter);
            }

            _ => {}
        }
    }

    /// Handles IIFE (Immediately Invoked Function Expression) in expressions
    /// IIFE can be: (function name() { ... })() or (() => { ... })()
    fn walk_expr_for_iife(
        &self,
        expr_stmt: &ExprStmt,
        function_name: &str,
        result: &mut Option<FunctionInfo>,
        file_path: &str,
        converter: &LocationConverter,
    ) {
        // IIFE pattern: (function() { ... })() or (() => { ... })()
        if let Expr::Call(call_expr) = expr_stmt.expr.as_ref() {
            if let Callee::Expr(callee_expr) = &call_expr.callee {
                match callee_expr.as_ref() {
                    Expr::Fn(fn_expr) => {
                        // Named IIFE: (function name() { ... })()
                        if let Some(ident) = &fn_expr.ident {
                            if ident.sym.as_ref() == function_name {
                                let span = ident.span;
                                let (line, column) =
                                    converter.byte_offset_to_location(span.lo.0 as usize);
                                let parameters =
                                    self.extract_function_parameters(&fn_expr.function);
                                let return_type = self.extract_return_type(&fn_expr.function);

                                *result = Some(FunctionInfo {
                                    name: function_name.to_string(),
                                    parameters,
                                    return_type,
                                    is_async: fn_expr.function.is_async,
                                    location: crate::models::Location {
                                        file: file_path.to_string(),
                                        line,
                                        column: Some(column),
                                    },
                                });
                            }
                        }
                    }
                    Expr::Arrow(_arrow_fn) => {
                        // Anonymous IIFE: (() => { ... })()
                        // For anonymous IIFE, we can't match by name
                        // This would require context or location-based matching
                        // For now, skip anonymous IIFE
                    }
                    _ => {}
                }
            }
        }
    }

    /// Searches for a method in a class declaration
    fn walk_class_for_method(
        &self,
        class_decl: &swc_ecma_ast::ClassDecl,
        function_name: &str,
        result: &mut Option<FunctionInfo>,
        file_path: &str,
        converter: &LocationConverter,
    ) {
        for item in &class_decl.class.body {
            if let swc_ecma_ast::ClassMember::Method(class_method) = item {
                if let swc_ecma_ast::PropName::Ident(ident) = &class_method.key {
                    if ident.sym.as_ref() == function_name {
                        let span = class_method.span;
                        let (line, column) = converter.byte_offset_to_location(span.lo.0 as usize);
                        let parameters = self.extract_function_parameters(&class_method.function);
                        let return_type = self.extract_return_type(&class_method.function);

                        *result = Some(FunctionInfo {
                            name: function_name.to_string(),
                            parameters,
                            return_type,
                            is_async: class_method.function.is_async,
                            location: crate::models::Location {
                                file: file_path.to_string(),
                                line,
                                column: Some(column),
                            },
                        });
                    }
                }
            }
        }
    }

    /// Extracts function parameters
    fn extract_function_parameters(
        &self,
        function: &swc_ecma_ast::Function,
    ) -> Vec<crate::call_graph::Parameter> {
        let mut params = Vec::new();

        for param in &function.params {
            match &param.pat {
                Pat::Ident(ident) => {
                    params.push(self.parameter_from_binding_ident(ident, None, false));
                }
                Pat::Assign(assign) => {
                    if let Pat::Ident(ident) = assign.left.as_ref() {
                        let default_value = Some(self.expr_to_literal(&assign.right));
                        params.push(self.parameter_from_binding_ident(ident, default_value, true));
                    }
                }
                _ => {}
            }
        }

        params
    }

    /// Extracts arrow function parameters
    fn extract_arrow_function_parameters(
        &self,
        arrow_fn: &swc_ecma_ast::ArrowExpr,
    ) -> Vec<crate::call_graph::Parameter> {
        let mut params = Vec::new();

        for param in &arrow_fn.params {
            match param {
                swc_ecma_ast::Pat::Ident(ident) => {
                    params.push(self.parameter_from_binding_ident(ident, None, false));
                }
                swc_ecma_ast::Pat::Assign(assign) => {
                    if let Pat::Ident(ident) = assign.left.as_ref() {
                        let default_value = Some(self.expr_to_literal(&assign.right));
                        params.push(self.parameter_from_binding_ident(ident, default_value, true));
                    }
                }
                _ => {}
            }
        }

        params
    }

    /// Extracts function return type
    fn extract_return_type(&self, function: &swc_ecma_ast::Function) -> Option<TypeInfo> {
        function
            .return_type
            .as_ref()
            .map(|type_ann| self.ts_type_ann_to_type_info(type_ann))
    }

    /// Extracts arrow function return type
    fn extract_arrow_return_type(&self, arrow_fn: &swc_ecma_ast::ArrowExpr) -> Option<TypeInfo> {
        arrow_fn
            .return_type
            .as_ref()
            .map(|type_ann| self.ts_type_ann_to_type_info(type_ann))
    }

    /// Converts TsTypeAnn to TypeInfo
    fn ts_type_ann_to_type_info(&self, type_ann: &swc_ecma_ast::TsTypeAnn) -> TypeInfo {
        let ts_type = &type_ann.type_ann;

        // Check if type is Promise<T> and extract inner type
        if self.is_promise_type(ts_type) {
            if let Some(inner_type) = self.extract_promise_inner_type(ts_type) {
                return self.ts_type_to_type_info(inner_type);
            }
        }

        // Regular processing
        let base_type = self.ts_type_to_base_type(ts_type);
        TypeInfo {
            base_type,
            schema_ref: None,
            constraints: Vec::new(),
            optional: false,
        }
    }

    /// Checks if type is Promise or PromiseLike
    fn is_promise_type(&self, ts_type: &swc_ecma_ast::TsType) -> bool {
        if let swc_ecma_ast::TsType::TsTypeRef(type_ref) = ts_type {
            let type_name = self.ts_entity_name_to_string(&type_ref.type_name);
            return type_name == "Promise" || type_name == "PromiseLike";
        }
        false
    }

    /// Extracts inner type from Promise<T> or PromiseLike<T>
    fn extract_promise_inner_type<'a>(
        &self,
        ts_type: &'a swc_ecma_ast::TsType,
    ) -> Option<&'a swc_ecma_ast::TsType> {
        if let swc_ecma_ast::TsType::TsTypeRef(type_ref) = ts_type {
            if self.is_promise_type(ts_type) {
                // Extract generic parameter
                // In SWC, type_params.params is Vec<Box<TsType>>
                if let Some(type_params) = &type_ref.type_params {
                    if let Some(first_param) = type_params.params.first() {
                        // first_param is Box<TsType>, return the inner type
                        return Some(first_param.as_ref());
                    }
                }
            }
        }
        None
    }

    /// Converts TsType to TypeInfo (recursive version for Promise extraction)
    fn ts_type_to_type_info(&self, ts_type: &swc_ecma_ast::TsType) -> TypeInfo {
        // Check for Promise<T>
        if let Some(inner_type) = self.extract_promise_inner_type(ts_type) {
            return self.ts_type_to_type_info(inner_type);
        }

        // Handle TsTypeRef with generic parameters
        if let swc_ecma_ast::TsType::TsTypeRef(type_ref) = ts_type {
            let base_type = crate::models::BaseType::Object;

            // If there are generic parameters, try to create schema reference from first param
            let schema_ref = if let Some(type_params) = &type_ref.type_params {
                if let Some(first_param) = type_params.params.first() {
                    // first_param is Box<TsType>
                    if let swc_ecma_ast::TsType::TsTypeRef(generic_ref) = first_param.as_ref() {
                        let type_name = self.ts_entity_name_to_string(&generic_ref.type_name);
                        // Create a basic schema reference
                        Some(crate::models::SchemaReference {
                            name: type_name,
                            schema_type: crate::models::SchemaType::TypeScript,
                            location: crate::models::Location {
                                file: String::new(), // Will be filled by caller
                                line: 0,
                                column: None,
                            },
                            metadata: std::collections::HashMap::new(),
                        })
                    } else {
                        None
                    }
                } else {
                    None
                }
            } else {
                None
            };

            return TypeInfo {
                base_type,
                schema_ref,
                constraints: Vec::new(),
                optional: false,
            };
        }

        // Fallback to base type conversion
        let base_type = self.ts_type_to_base_type(ts_type);
        TypeInfo {
            base_type,
            schema_ref: None,
            constraints: Vec::new(),
            optional: false,
        }
    }

    fn parameter_from_binding_ident(
        &self,
        ident: &BindingIdent,
        default_value: Option<String>,
        force_optional: bool,
    ) -> crate::call_graph::Parameter {
        let mut type_info = if let Some(type_ann) = &ident.type_ann {
            self.ts_type_ann_to_type_info(type_ann)
        } else {
            TypeInfo {
                base_type: crate::models::BaseType::Unknown,
                schema_ref: None,
                constraints: Vec::new(),
                optional: false,
            }
        };

        let optional = ident.optional || force_optional;
        type_info.optional = optional;

        crate::call_graph::Parameter {
            name: ident.id.sym.as_ref().to_string(),
            type_info,
            optional,
            default_value,
        }
    }

    fn expr_to_literal(&self, expr: &swc_ecma_ast::Expr) -> String {
        match expr {
            swc_ecma_ast::Expr::Lit(lit) => match lit {
                swc_ecma_ast::Lit::Str(s) => format!("{:?}", s.value),
                swc_ecma_ast::Lit::Bool(b) => b.value.to_string(),
                swc_ecma_ast::Lit::Num(n) => n.value.to_string(),
                swc_ecma_ast::Lit::Null(_) => "null".to_string(),
                swc_ecma_ast::Lit::BigInt(bi) => bi.value.to_string(),
                swc_ecma_ast::Lit::Regex(regex) => format!("/{:?}/{:?}", regex.exp, regex.flags),
                swc_ecma_ast::Lit::JSXText(text) => format!("{:?}", text.value),
            },
            swc_ecma_ast::Expr::Ident(ident) => ident.sym.as_ref().to_string(),
            swc_ecma_ast::Expr::Array(_) => "[]".to_string(),
            swc_ecma_ast::Expr::Object(_) => "{...}".to_string(),
            _ => format!("{:?}", expr),
        }
    }

    /// Extracts class methods
    fn extract_class_methods(
        &self,
        class: &swc_ecma_ast::Class,
        _file_path: &str,
        converter: &LocationConverter,
    ) -> Vec<ClassMethod> {
        let mut methods = Vec::new();

        for member in &class.body {
            if let swc_ecma_ast::ClassMember::Method(method) = member {
                let span = method.span;
                let (line, column) = converter.byte_offset_to_location(span.lo.0 as usize);

                let name = match &method.key {
                    swc_ecma_ast::PropName::Ident(ident) => ident.sym.as_ref().to_string(),
                    swc_ecma_ast::PropName::Str(str) => {
                        str.value.as_str().unwrap_or("").to_string()
                    }
                    _ => "unknown".to_string(),
                };

                let parameters = self.extract_function_parameters(&method.function);
                let return_type = self.extract_return_type(&method.function);
                let is_async = method.function.is_async;
                let is_static = method.is_static;

                methods.push(ClassMethod {
                    name,
                    line,
                    column,
                    parameters,
                    return_type,
                    is_async,
                    is_static,
                });
            }
        }

        methods
    }

    /// Extracts decorators from TypeScript module
    ///
    /// Extracts decorators from classes, methods, and parameters
    pub fn extract_decorators(
        &self,
        module: &Module,
        file_path: &str,
        converter: &LocationConverter,
        source: &str,
    ) -> Vec<TypeScriptDecorator> {
        let mut decorators = Vec::new();

        for item in &module.body {
            match item {
                ModuleItem::Stmt(Stmt::Decl(Decl::Class(class_decl))) => {
                    let class_name = class_decl.ident.sym.as_ref().to_string();
                    // Extract class decorators
                    decorators.extend(self.extract_class_decorators(
                        class_decl,
                        &class_name,
                        file_path,
                        converter,
                    ));
                    // Extract method decorators
                    decorators.extend(self.extract_method_decorators_from_class(
                        &class_decl.class,
                        &class_name,
                        file_path,
                        converter,
                        source,
                    ));
                }
                ModuleItem::ModuleDecl(ModuleDecl::ExportDecl(export_decl)) => {
                    if let Decl::Class(class_decl) = &export_decl.decl {
                        let class_name = class_decl.ident.sym.as_ref().to_string();
                        // Extract class decorators
                        decorators.extend(self.extract_class_decorators(
                            class_decl,
                            &class_name,
                            file_path,
                            converter,
                        ));
                        // Extract method decorators
                        decorators.extend(self.extract_method_decorators_from_class(
                            &class_decl.class,
                            &class_name,
                            file_path,
                            converter,
                            source,
                        ));
                    }
                }
                _ => {}
            }
        }

        decorators
    }

    /// Extracts decorators from class declaration
    fn extract_class_decorators(
        &self,
        class_decl: &swc_ecma_ast::ClassDecl,
        class_name: &str,
        file_path: &str,
        converter: &LocationConverter,
    ) -> Vec<TypeScriptDecorator> {
        let mut decorators = Vec::new();

        for decorator in &class_decl.class.decorators {
            if let Some(name) = self.get_decorator_name_from_expr(decorator) {
                let (args, kwargs) = self.extract_decorator_arguments_from_expr(decorator);
                let span = decorator.span;
                let (line, column) = converter.byte_offset_to_location(span.lo.0 as usize);

                decorators.push(TypeScriptDecorator {
                    name,
                    arguments: args,
                    keyword_arguments: kwargs,
                    location: Location {
                        file: file_path.to_string(),
                        line,
                        column: Some(column),
                    },
                    target: DecoratorTarget::Class(class_name.to_string()),
                });
            }
        }

        decorators
    }

    /// Extracts decorators from class methods
    fn extract_method_decorators_from_class(
        &self,
        class: &swc_ecma_ast::Class,
        class_name: &str,
        file_path: &str,
        converter: &LocationConverter,
        source: &str,
    ) -> Vec<TypeScriptDecorator> {
        let mut decorators = Vec::new();

        for member in &class.body {
            if let swc_ecma_ast::ClassMember::Method(method) = member {
                let method_name = match &method.key {
                    swc_ecma_ast::PropName::Ident(ident) => ident.sym.as_ref().to_string(),
                    swc_ecma_ast::PropName::Str(str) => {
                        str.value.as_str().unwrap_or("").to_string()
                    }
                    _ => "unknown".to_string(),
                };

                // Extract method decorators
                for decorator in &method.function.decorators {
                    if let Some(name) = self.get_decorator_name_from_expr(decorator) {
                        let (args, kwargs) = self.extract_decorator_arguments_from_expr(decorator);
                        let span = decorator.span;
                        let (line, column) = converter.byte_offset_to_location(span.lo.0 as usize);

                        decorators.push(TypeScriptDecorator {
                            name,
                            arguments: args,
                            keyword_arguments: kwargs,
                            location: Location {
                                file: file_path.to_string(),
                                line,
                                column: Some(column),
                            },
                            target: DecoratorTarget::Method {
                                class: class_name.to_string(),
                                method: method_name.clone(),
                            },
                        });
                    }
                }

                // Extract parameter decorators
                decorators.extend(self.extract_parameter_decorators(
                    &method.function,
                    Some(class_name),
                    Some(&method_name),
                    source,
                    file_path,
                    converter,
                ));
            }
        }

        decorators
    }

    /// Extracts parameter decorators
    ///
    /// В SWC AST декораторы параметров могут быть не доступны напрямую.
    /// Парсим из исходного кода, используя позиции параметров.
    fn extract_parameter_decorators(
        &self,
        function: &swc_ecma_ast::Function,
        class_name: Option<&str>,
        method_name: Option<&str>,
        source: &str,
        _file_path: &str,
        converter: &LocationConverter,
    ) -> Vec<TypeScriptDecorator> {
        let mut decorators = Vec::new();

        // Парсим декораторы параметров из исходного кода
        // Ищем паттерн: @Decorator() param: Type
        for (param_idx, param) in function.params.iter().enumerate() {
            let param_name = match &param.pat {
                Pat::Ident(ident) => ident.id.sym.as_ref().to_string(),
                _ => format!("param{}", param_idx),
            };

            // Пытаемся найти декораторы перед параметром в исходном коде
            // Это приблизительный подход - ищем @Body(), @Query(), @Param() перед именем параметра
            if let Some(decorator_info) =
                self.find_parameter_decorator_in_source(source, &param_name, param.span, converter)
            {
                if let (Some(class), Some(method)) = (class_name, method_name) {
                    decorators.push(TypeScriptDecorator {
                        name: decorator_info.name,
                        arguments: decorator_info.arguments,
                        keyword_arguments: decorator_info.keyword_arguments,
                        location: decorator_info.location,
                        target: DecoratorTarget::Parameter {
                            class: class.to_string(),
                            method: method.to_string(),
                            parameter: param_name,
                        },
                    });
                }
            }
        }

        decorators
    }

    /// Finds parameter decorator in source code
    ///
    /// Ищет декораторы типа @Body(), @Query(), @Param() перед параметром
    fn find_parameter_decorator_in_source(
        &self,
        source: &str,
        param_name: &str,
        param_span: swc_common::Span,
        converter: &LocationConverter,
    ) -> Option<TypeScriptDecorator> {
        // Получаем позицию параметра в исходном коде
        let param_start = param_span.lo.0 as usize;
        let param_end = param_span.hi.0 as usize;

        // Ищем перед параметром (в пределах ~200 символов)
        let search_start = param_start.saturating_sub(200);
        let search_text = &source[search_start..param_end.min(source.len())];

        // Ищем паттерны декораторов NestJS
        let decorator_prefixes = ["Body", "Query", "Param", "Headers", "Req", "Res"];

        for name in &decorator_prefixes {
            let pattern = format!("@{}(", name);
            if let Some(pos) = search_text.rfind(&pattern) {
                // Найдено, создаем декоратор
                let decorator_pos = search_start + pos;
                let (line, column) = converter.byte_offset_to_location(decorator_pos);

                // Извлекаем аргументы из скобок
                let mut args = Vec::new();
                let after_at_and_name = &search_text[pos + pattern.len()..];
                if let Some(paren_end) = after_at_and_name.find(')') {
                    let arg_text = &after_at_and_name[..paren_end];
                    if !arg_text.trim().is_empty() {
                        let arg = arg_text.trim().trim_matches('\'').trim_matches('"');
                        if !arg.is_empty() {
                            args.push(arg.to_string());
                        }
                    }
                }

                return Some(TypeScriptDecorator {
                    name: name.to_string(),
                    arguments: args,
                    keyword_arguments: std::collections::HashMap::new(),
                    location: Location {
                        file: String::new(), // Будет заполнено вызывающим кодом
                        line,
                        column: Some(column),
                    },
                    target: DecoratorTarget::Parameter {
                        class: String::new(),
                        method: String::new(),
                        parameter: param_name.to_string(),
                    },
                });
            }
        }

        None
    }

    /// Gets decorator name from SWC AST expression
    fn get_decorator_name_from_expr(&self, decorator: &swc_ecma_ast::Decorator) -> Option<String> {
        Self::get_decorator_name_from_expr_inner(decorator.expr.as_ref())
    }

    /// Helper to get decorator name recursively
    fn get_decorator_name_from_expr_inner(expr: &swc_ecma_ast::Expr) -> Option<String> {
        match expr {
            Expr::Ident(ident) => Some(ident.sym.as_ref().to_string()),
            Expr::Member(member) => {
                let obj_name = Self::get_decorator_name_from_expr_inner(member.obj.as_ref())?;
                let prop_name = match &member.prop {
                    swc_ecma_ast::MemberProp::Ident(ident) => ident.sym.as_ref().to_string(),
                    swc_ecma_ast::MemberProp::Computed(_) => return None,
                    swc_ecma_ast::MemberProp::PrivateName(_) => return None,
                };
                Some(format!("{}.{}", obj_name, prop_name))
            }
            Expr::Call(call) => match &call.callee {
                swc_ecma_ast::Callee::Expr(expr) => Self::get_decorator_name_from_expr_inner(expr),
                _ => None,
            },
            _ => None,
        }
    }

    /// Extracts decorator arguments from SWC AST expression
    fn extract_decorator_arguments_from_expr(
        &self,
        decorator: &swc_ecma_ast::Decorator,
    ) -> (Vec<String>, std::collections::HashMap<String, String>) {
        let mut args = Vec::new();
        let kwargs = std::collections::HashMap::new();

        if let Expr::Call(call_expr) = decorator.expr.as_ref() {
            // Extract positional arguments
            for arg in &call_expr.args {
                args.push(self.expr_to_string_for_decorator(arg));
            }

            // Keyword arguments are not common in TypeScript decorators,
            // but we handle them for completeness
            // Note: TypeScript decorators don't support keyword arguments like Python,
            // but we keep this for potential future use
        }

        (args, kwargs)
    }

    /// Converts expression to string for decorator arguments
    fn expr_to_string_for_decorator(&self, expr: &swc_ecma_ast::ExprOrSpread) -> String {
        match expr.expr.as_ref() {
            Expr::Lit(lit) => match lit {
                Lit::Str(s) => s.value.as_str().unwrap_or("").to_string(),
                Lit::Num(n) => n.value.to_string(),
                Lit::Bool(b) => b.value.to_string(),
                Lit::Null(_) => "null".to_string(),
                _ => format!("{:?}", lit),
            },
            Expr::Ident(ident) => ident.sym.as_ref().to_string(),
            Expr::Member(member) => {
                let obj = Self::expr_to_string_for_decorator_inner(member.obj.as_ref());
                let prop = match &member.prop {
                    swc_ecma_ast::MemberProp::Ident(ident) => ident.sym.as_ref().to_string(),
                    _ => "?".to_string(),
                };
                format!("{}.{}", obj, prop)
            }
            _ => format!("{:?}", expr.expr),
        }
    }

    /// Helper to convert expression to string
    fn expr_to_string_for_decorator_inner(expr: &swc_ecma_ast::Expr) -> String {
        match expr {
            Expr::Ident(ident) => ident.sym.as_ref().to_string(),
            Expr::Member(member) => {
                let obj = Self::expr_to_string_for_decorator_inner(member.obj.as_ref());
                let prop = match &member.prop {
                    swc_ecma_ast::MemberProp::Ident(ident) => ident.sym.as_ref().to_string(),
                    _ => "?".to_string(),
                };
                format!("{}.{}", obj, prop)
            }
            _ => format!("{:?}", expr),
        }
    }
}

/// Function or class from TypeScript code
#[derive(Debug, Clone)]
pub enum FunctionOrClass {
    Function {
        name: String,
        line: usize,
        column: usize,
        parameters: Vec<crate::call_graph::Parameter>,
        return_type: Option<TypeInfo>,
        is_async: bool,
    },
    Class {
        name: String,
        line: usize,
        column: usize,
        methods: Vec<ClassMethod>,
    },
}

/// Class method
#[derive(Debug, Clone)]
pub struct ClassMethod {
    pub name: String,
    pub line: usize,
    pub column: usize,
    pub parameters: Vec<crate::call_graph::Parameter>,
    pub return_type: Option<TypeInfo>,
    pub is_async: bool,
    pub is_static: bool,
}

/// TypeScript decorator (for NestJS, etc.)
#[derive(Debug, Clone)]
pub struct TypeScriptDecorator {
    /// Decorator name (e.g., "Controller", "Get", "Body")
    pub name: String,
    /// Decorator positional arguments
    pub arguments: Vec<String>,
    /// Decorator keyword arguments
    pub keyword_arguments: std::collections::HashMap<String, String>,
    /// Location in code
    pub location: Location,
    /// Target of the decorator (class, method, or parameter)
    pub target: DecoratorTarget,
}

/// Target of a TypeScript decorator
#[derive(Debug, Clone)]
pub enum DecoratorTarget {
    /// Decorator on a class
    Class(String),
    /// Decorator on a method
    Method { class: String, method: String },
    /// Decorator on a parameter
    Parameter {
        class: String,
        method: String,
        parameter: String,
    },
}

impl Default for TypeScriptParser {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_extract_imports() {
        let parser = TypeScriptParser::new();
        let source = r#"
import { Component } from './Component';
import express from 'express';
"#;
        let temp_dir = TempDir::new().unwrap();
        let test_file = temp_dir.path().join("test.ts");
        std::fs::write(&test_file, source).unwrap();

        let (module, _, converter) = parser.parse_file(&test_file).unwrap();
        let imports = parser.extract_imports(&module, test_file.to_str().unwrap(), &converter);

        assert_eq!(imports.len(), 2);
        assert_eq!(imports[0].path, "./Component");
        assert_eq!(imports[1].path, "express");
    }

    #[test]
    fn test_extract_calls() {
        let parser = TypeScriptParser::new();
        let source = r#"
function test() {
    doSomething();
    anotherFunction(arg1, arg2);
}
"#;
        let temp_dir = TempDir::new().unwrap();
        let test_file = temp_dir.path().join("test.ts");
        std::fs::write(&test_file, source).unwrap();

        let (module, source, converter) = parser.parse_file(&test_file).unwrap();
        let calls = parser.extract_calls(&module, test_file.to_str().unwrap(), &converter, &source);

        assert!(calls.len() >= 2);
        assert!(calls.iter().any(|c| c.name == "doSomething"));
        assert!(calls.iter().any(|c| c.name == "anotherFunction"));
    }

    #[test]
    fn test_extract_typescript_schemas_interface() {
        let parser = TypeScriptParser::new();
        let source = r#"
interface User {
    name: string;
    age: number;
    email?: string;
}
"#;
        let temp_dir = TempDir::new().unwrap();
        let test_file = temp_dir.path().join("test.ts");
        std::fs::write(&test_file, source).unwrap();

        let (module, _, converter) = parser.parse_file(&test_file).unwrap();
        let schemas =
            parser.extract_typescript_schemas(&module, test_file.to_str().unwrap(), &converter);

        assert_eq!(schemas.len(), 1);
        assert_eq!(schemas[0].name, "User");
        assert_eq!(schemas[0].schema_type, SchemaType::TypeScript);
        assert!(schemas[0].metadata.contains_key("fields"));
    }

    #[test]
    fn test_extract_typescript_schemas_generated_file_metadata() {
        let parser = TypeScriptParser::new();
        let source = r#"
interface Item {
    id: string;
}
"#;
        let temp_dir = TempDir::new().unwrap();
        // Imitate common OpenAPI-generated types file
        let test_file = temp_dir.path().join("types.gen.ts");
        std::fs::write(&test_file, source).unwrap();

        let (module, _, converter) = parser.parse_file(&test_file).unwrap();
        let schemas =
            parser.extract_typescript_schemas(&module, test_file.to_str().unwrap(), &converter);

        assert_eq!(schemas.len(), 1);
        let schema = &schemas[0];
        assert_eq!(schema.name, "Item");
        assert_eq!(schema.schema_type, SchemaType::TypeScript);
        // New metadata flags for generated files
        assert_eq!(
            schema.metadata.get("openapi_generated").map(String::as_str),
            Some("true")
        );
        assert!(schema.metadata.contains_key("openapi_generated_from"));
    }

    #[test]
    fn test_extract_typescript_schemas_type_alias() {
        let parser = TypeScriptParser::new();
        let source = r#"
type UserId = string;
type UserRole = 'admin' | 'user';
"#;
        let temp_dir = TempDir::new().unwrap();
        let test_file = temp_dir.path().join("test.ts");
        std::fs::write(&test_file, source).unwrap();

        let (module, _, converter) = parser.parse_file(&test_file).unwrap();
        let schemas =
            parser.extract_typescript_schemas(&module, test_file.to_str().unwrap(), &converter);

        assert_eq!(schemas.len(), 2);
        assert!(schemas.iter().any(|s| s.name == "UserId"));
        assert!(schemas.iter().any(|s| s.name == "UserRole"));
    }

    #[test]
    fn test_extract_zod_schemas() {
        let parser = TypeScriptParser::new();
        let source = r#"
const userSchema = z.object({
    name: z.string(),
    age: z.number(),
});
"#;
        let temp_dir = TempDir::new().unwrap();
        let test_file = temp_dir.path().join("test.ts");
        std::fs::write(&test_file, source).unwrap();

        let (module, _, converter) = parser.parse_file(&test_file).unwrap();
        let schemas = parser.extract_zod_schemas(&module, test_file.to_str().unwrap(), &converter);

        assert_eq!(schemas.len(), 1);
        assert_eq!(schemas[0].name, "userSchema");
        assert_eq!(schemas[0].schema_type, SchemaType::Zod);
    }

    #[test]
    fn test_extract_functions_and_classes() {
        let parser = TypeScriptParser::new();
        let source = r#"
export function processUser(user: User): void {
    // implementation
}

class UserService {
    async getUser(id: string): Promise<User> {
        return {} as User;
    }
}
"#;
        let temp_dir = TempDir::new().unwrap();
        let test_file = temp_dir.path().join("test.ts");
        std::fs::write(&test_file, source).unwrap();

        let (module, _, converter) = parser.parse_file(&test_file).unwrap();
        let items =
            parser.extract_functions_and_classes(&module, test_file.to_str().unwrap(), &converter);

        assert!(items.len() >= 2);
        let has_function = items.iter().any(|item| {
            if let FunctionOrClass::Function { name, .. } = item {
                name == "processUser"
            } else {
                false
            }
        });
        let has_class = items.iter().any(|item| {
            if let FunctionOrClass::Class { name, .. } = item {
                name == "UserService"
            } else {
                false
            }
        });
        assert!(has_function);
        assert!(has_class);
    }

    #[test]
    fn test_zod_typescript_sync() {
        let parser = TypeScriptParser::new();
        let source = r#"
interface User {
    name: string;
    age: number;
}

const User = z.object({
    name: z.string(),
    age: z.number(),
});
"#;
        let temp_dir = TempDir::new().unwrap();
        let test_file = temp_dir.path().join("test.ts");
        std::fs::write(&test_file, source).unwrap();

        let (module, _, converter) = parser.parse_file(&test_file).unwrap();
        let zod_schemas =
            parser.extract_zod_schemas(&module, test_file.to_str().unwrap(), &converter);

        // Check that Zod schema is linked with TypeScript interface (if names match)
        let user_schema = zod_schemas.iter().find(|s| s.name == "User");
        if let Some(schema) = user_schema {
            // If there is a link, it should be in metadata
            assert!(
                schema.metadata.contains_key("typescript_type")
                    || schema.metadata.contains_key("fields")
            );
        }
    }

    #[test]
    fn test_union_intersection_types() {
        let parser = TypeScriptParser::new();
        let source = r#"
interface A {
    a: string;
}

interface B {
    b: number;
}

type Union = A | B;
type Intersection = A & B;
"#;
        let temp_dir = TempDir::new().unwrap();
        let test_file = temp_dir.path().join("test.ts");
        std::fs::write(&test_file, source).unwrap();

        let (module, _, converter) = parser.parse_file(&test_file).unwrap();
        let schemas =
            parser.extract_typescript_schemas(&module, test_file.to_str().unwrap(), &converter);

        assert!(schemas.len() >= 4);
        assert!(schemas.iter().any(|s| s.name == "Union"));
        assert!(schemas.iter().any(|s| s.name == "Intersection"));
    }

    #[test]
    fn test_nested_classes() {
        let parser = TypeScriptParser::new();
        let source = r#"
class Outer {
    outerMethod(): void {}
}

class Inner {
    innerMethod(): void {}
    
    private nestedMethod(): void {}
    
    static staticMethod(): void {}
}
"#;
        let temp_dir = TempDir::new().unwrap();
        let test_file = temp_dir.path().join("test.ts");
        std::fs::write(&test_file, source).unwrap();

        let (module, _, converter) = parser.parse_file(&test_file).unwrap();
        let items =
            parser.extract_functions_and_classes(&module, test_file.to_str().unwrap(), &converter);

        let outer_class = items.iter().find(|item| {
            if let FunctionOrClass::Class { name, .. } = item {
                name == "Outer"
            } else {
                false
            }
        });
        assert!(outer_class.is_some());

        let inner_class = items.iter().find(|item| {
            if let FunctionOrClass::Class { name, .. } = item {
                name == "Inner"
            } else {
                false
            }
        });
        assert!(inner_class.is_some());

        // Check that Inner class has methods
        if let Some(FunctionOrClass::Class { methods, .. }) = inner_class {
            assert!(methods.len() >= 2);
        }
    }

    #[test]
    fn test_generic_types() {
        let parser = TypeScriptParser::new();
        let source = r#"
interface Container<T> {
    value: T;
    getValue(): T;
}

type StringContainer = Container<string>;
"#;
        let temp_dir = TempDir::new().unwrap();
        let test_file = temp_dir.path().join("test.ts");
        std::fs::write(&test_file, source).unwrap();

        let (module, _, converter) = parser.parse_file(&test_file).unwrap();
        let schemas =
            parser.extract_typescript_schemas(&module, test_file.to_str().unwrap(), &converter);

        assert!(schemas.len() >= 2);
        assert!(schemas.iter().any(|s| s.name == "Container"));
        assert!(schemas.iter().any(|s| s.name == "StringContainer"));
    }

    #[test]
    fn test_generic_params_structure() {
        // Test to understand SWC AST structure for generic parameters
        let parser = TypeScriptParser::new();
        let source = r#"
        useQuery<UserResponse, Error>({ queryKey: ['user'], queryFn: () => {} });
        useMutation<ResponseType, ErrorType, VariablesType>({ mutationFn: () => {} });
        "#;
        let temp_dir = TempDir::new().unwrap();
        let test_file = temp_dir.path().join("test.ts");
        std::fs::write(&test_file, source).unwrap();

        let (module, source, converter) = parser.parse_file(&test_file).unwrap();
        let calls = parser.extract_calls(&module, test_file.to_str().unwrap(), &converter, &source);

        // This test helps us understand the structure
        // Generic params should be extracted in the future
        assert!(calls.len() >= 2);
        assert!(calls.iter().any(|c| c.name == "useQuery"));
        assert!(calls.iter().any(|c| c.name == "useMutation"));
    }
}
