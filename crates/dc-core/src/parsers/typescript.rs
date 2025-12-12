use crate::models::{Location, SchemaReference, SchemaType, TypeInfo};
use crate::parsers::{Call, CallArgument, Import, LocationConverter};
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
    ) -> Vec<Call> {
        let mut calls = Vec::new();
        let mut context = Vec::new();

        for item in &module.body {
            self.walk_module_item(item, &mut context, &mut calls, file_path, converter);
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
    ) {
        match item {
            ModuleItem::Stmt(stmt) => {
                self.walk_stmt(stmt, context, calls, file_path, converter);
            }
            ModuleItem::ModuleDecl(ModuleDecl::ExportDecl(export_decl)) => {
                if let Decl::Fn(fn_decl) = &export_decl.decl {
                    context.push(fn_decl.ident.sym.as_ref().to_string());
                    if let Some(body) = &fn_decl.function.body {
                        self.walk_block_stmt(body, context, calls, file_path, converter);
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
    ) {
        match stmt {
            Stmt::Expr(expr_stmt) => {
                self.walk_expr(&expr_stmt.expr, context, calls, file_path, converter);
            }
            Stmt::Return(ret_stmt) => {
                if let Some(expr) = &ret_stmt.arg {
                    self.walk_expr(expr, context, calls, file_path, converter);
                }
            }
            Stmt::If(if_stmt) => {
                self.walk_expr(&if_stmt.test, context, calls, file_path, converter);
                self.walk_stmt(&if_stmt.cons, context, calls, file_path, converter);
                if let Some(alt) = &if_stmt.alt {
                    self.walk_stmt(alt, context, calls, file_path, converter);
                }
            }
            Stmt::While(while_stmt) => {
                self.walk_expr(&while_stmt.test, context, calls, file_path, converter);
                self.walk_stmt(&while_stmt.body, context, calls, file_path, converter);
            }
            Stmt::For(for_stmt) => {
                if let Some(init) = &for_stmt.init {
                    self.walk_var_decl_or_expr(init, context, calls, file_path, converter);
                }
                if let Some(test) = &for_stmt.test {
                    self.walk_expr(test, context, calls, file_path, converter);
                }
                if let Some(update) = &for_stmt.update {
                    self.walk_expr(update, context, calls, file_path, converter);
                }
                self.walk_stmt(&for_stmt.body, context, calls, file_path, converter);
            }
            Stmt::Block(block_stmt) => {
                self.walk_block_stmt(block_stmt, context, calls, file_path, converter);
            }
            Stmt::Decl(Decl::Fn(fn_decl)) => {
                context.push(fn_decl.ident.sym.as_ref().to_string());
                if let Some(body) = &fn_decl.function.body {
                    self.walk_block_stmt(body, context, calls, file_path, converter);
                }
                context.pop();
            }
            Stmt::Decl(Decl::Var(var_decl)) => {
                for decl in &var_decl.decls {
                    if let Some(init) = &decl.init {
                        self.walk_expr(init, context, calls, file_path, converter);
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
    ) {
        for stmt in &block.stmts {
            self.walk_stmt(stmt, context, calls, file_path, converter);
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
    ) {
        match init {
            VarDeclOrExpr::VarDecl(var_decl) => {
                for decl in &var_decl.decls {
                    if let Some(init) = &decl.init {
                        self.walk_expr(init, context, calls, file_path, converter);
                    }
                }
            }
            VarDeclOrExpr::Expr(expr) => {
                self.walk_expr(expr, context, calls, file_path, converter);
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
    ) {
        match expr {
            Expr::Call(call_expr) => {
                if let Some(name) = self.call_name(&call_expr.callee) {
                    let arguments = self.extract_call_arguments(call_expr);
                    let span = call_expr.span;
                    let (line, column) = converter.byte_offset_to_location(span.lo.0 as usize);

                    let caller = if context.is_empty() {
                        None
                    } else {
                        Some(context.join("."))
                    };

                    calls.push(Call {
                        name,
                        arguments,
                        location: Location {
                            file: file_path.to_string(),
                            line,
                            column: Some(column),
                        },
                        caller,
                    });
                }

                // Recursively traverse arguments
                for arg in &call_expr.args {
                    self.walk_expr_or_spread(arg, context, calls, file_path, converter);
                }
            }
            Expr::Member(member_expr) => {
                self.walk_expr(
                    member_expr.obj.as_ref(),
                    context,
                    calls,
                    file_path,
                    converter,
                );
            }
            Expr::Bin(bin_expr) => {
                self.walk_expr(bin_expr.left.as_ref(), context, calls, file_path, converter);
                self.walk_expr(
                    bin_expr.right.as_ref(),
                    context,
                    calls,
                    file_path,
                    converter,
                );
            }
            Expr::Unary(unary_expr) => {
                self.walk_expr(
                    unary_expr.arg.as_ref(),
                    context,
                    calls,
                    file_path,
                    converter,
                );
            }
            Expr::Cond(cond_expr) => {
                self.walk_expr(
                    cond_expr.test.as_ref(),
                    context,
                    calls,
                    file_path,
                    converter,
                );
                self.walk_expr(
                    cond_expr.cons.as_ref(),
                    context,
                    calls,
                    file_path,
                    converter,
                );
                self.walk_expr(cond_expr.alt.as_ref(), context, calls, file_path, converter);
            }
            Expr::Assign(assign_expr) => {
                self.walk_expr(
                    assign_expr.right.as_ref(),
                    context,
                    calls,
                    file_path,
                    converter,
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
    ) {
        self.walk_expr(&arg.expr, context, calls, file_path, converter);
    }

    /// Extracts function name from Callee
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

                            let metadata = std::collections::HashMap::new();

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
        match expr {
            Expr::Member(member_expr) => {
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
            _ => {}
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

                        let schema_ref = SchemaReference {
                            name: name.clone(),
                            schema_type: SchemaType::TypeScript,
                            location: Location {
                                file: file_path.to_string(),
                                line,
                                column: Some(column),
                            },
                            metadata: std::collections::HashMap::new(),
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

                let schema_ref = SchemaReference {
                    name: name.clone(),
                    schema_type: SchemaType::TypeScript,
                    location: Location {
                        file: file_path.to_string(),
                        line,
                        column: Some(column),
                    },
                    metadata: std::collections::HashMap::new(),
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
                        qualified.right.sym.as_ref().to_string()
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
    fn ts_entity_name_to_string(&self, entity_name: &swc_ecma_ast::TsEntityName) -> String {
        match entity_name {
            swc_ecma_ast::TsEntityName::Ident(ident) => ident.sym.as_ref().to_string(),
            swc_ecma_ast::TsEntityName::TsQualifiedName(qualified) => {
                format!(
                    "{}.{}",
                    self.ts_entity_name_to_string(&qualified.left),
                    qualified.right.sym.as_ref().to_string()
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
        let base_type = self.ts_type_to_base_type(&type_ann.type_ann);
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
            match member {
                swc_ecma_ast::ClassMember::Method(method) => {
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
                _ => {}
            }
        }

        methods
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

        let (module, _, converter) = parser.parse_file(&test_file).unwrap();
        let calls = parser.extract_calls(&module, test_file.to_str().unwrap(), &converter);

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
}
