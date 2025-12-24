use dc_core::models::{Location, SchemaReference, SchemaType};
use dc_core::parsers::LocationConverter;
use serde_json;
use swc_ecma_ast::{
    CallExpr, Callee, Expr, MemberProp, Module, ModuleItem, Prop, PropName, PropOrSpread, Stmt,
};

/// Zod schema extractor from TypeScript code
pub struct ZodExtractor;

impl ZodExtractor {
    /// Creates a new extractor
    pub fn new() -> Self {
        Self
    }

    /// Extracts Zod schema from AST node
    pub fn extract_schema(
        &self,
        node: &Expr,
        file_path: &str,
        line: usize,
    ) -> Option<SchemaReference> {
        // Look for calls like z.object(), z.string(), etc.
        if let Expr::Call(call_expr) = node {
            return self.extract_schema_with_context(call_expr, None, file_path, line);
        }
        None
    }

    /// Extracts Zod schema from AST node with context (for use from TypeScriptParser)
    pub fn extract_schema_with_context(
        &self,
        call_expr: &CallExpr,
        var_name: Option<&str>,
        file_path: &str,
        line: usize,
    ) -> Option<SchemaReference> {
        if let Callee::Expr(callee_expr) = &call_expr.callee {
            if self.is_zod_call(callee_expr) {
                let schema_name = self.extract_schema_name(call_expr, var_name);

                let mut metadata = std::collections::HashMap::new();

                // Check if this is z.object() and extract fields
                if self.is_zod_object_call(callee_expr) {
                    let fields = self.extract_zod_object_fields(call_expr);
                    if !fields.is_empty() {
                        // Store fields as JSON
                        if let Ok(fields_json) = serde_json::to_string(&fields) {
                            metadata.insert("fields".to_string(), fields_json);
                        }
                    }
                }

                return Some(SchemaReference {
                    name: schema_name,
                    schema_type: SchemaType::Zod,
                    location: Location {
                        file: file_path.to_string(),
                        line,
                        column: None,
                    },
                    metadata,
                });
            }
        }
        None
    }

    /// Checks if expression is a Zod call (z.object, z.string, etc.)
    fn is_zod_call(&self, expr: &Expr) -> bool {
        if let Expr::Member(member_expr) = expr {
            if let Expr::Ident(ident) = member_expr.obj.as_ref() {
                if ident.sym.as_ref() == "z" {
                    // Check Zod methods
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

    /// Extracts schema name from call
    fn extract_schema_name(&self, _call_expr: &CallExpr, var_name: Option<&str>) -> String {
        // Use variable name if provided
        if let Some(name) = var_name {
            return name.to_string();
        }

        // Try to find variable name from context
        // For now, return default name if context unavailable
        "ZodSchema".to_string()
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

    /// Extracts fields from z.object({...}) call
    /// Example: z.object({ name: z.string(), age: z.number().optional() })
    fn extract_zod_object_fields(&self, call_expr: &CallExpr) -> Vec<dc_core::models::ZodField> {
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

                            fields.push(dc_core::models::ZodField {
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

    /// Converts Zod schema to SchemaReference
    pub fn zod_to_schema(&self, zod_schema: &str, location: Location) -> SchemaReference {
        SchemaReference {
            name: zod_schema.to_string(),
            schema_type: SchemaType::Zod,
            location,
            metadata: std::collections::HashMap::new(),
        }
    }

    /// Finds all usages of a Zod schema in a module
    /// Looks for patterns like: schemaName.safeParse(...), schemaName.parse(...)
    pub fn find_zod_schema_usage(
        &self,
        schema_name: &str,
        module: &Module,
        file_path: &str,
        converter: &LocationConverter,
    ) -> Vec<dc_core::models::ZodUsage> {
        let mut usages = Vec::new();

        for item in &module.body {
            self.walk_for_zod_usage(item, schema_name, &mut usages, file_path, converter);
        }

        usages
    }

    /// Recursively walks AST to find Zod schema usages
    fn walk_for_zod_usage(
        &self,
        item: &ModuleItem,
        schema_name: &str,
        usages: &mut Vec<dc_core::models::ZodUsage>,
        file_path: &str,
        converter: &LocationConverter,
    ) {
        match item {
            ModuleItem::Stmt(stmt) => {
                self.walk_stmt_for_zod_usage(stmt, schema_name, usages, file_path, converter);
            }
            ModuleItem::ModuleDecl(_) => {
                // Module declarations don't contain Zod usage
            }
        }
    }

    /// Walks Statement to find Zod schema usages
    fn walk_stmt_for_zod_usage(
        &self,
        stmt: &Stmt,
        schema_name: &str,
        usages: &mut Vec<dc_core::models::ZodUsage>,
        file_path: &str,
        converter: &LocationConverter,
    ) {
        match stmt {
            Stmt::Expr(expr_stmt) => {
                self.walk_expr_for_zod_usage(
                    expr_stmt.expr.as_ref(),
                    schema_name,
                    usages,
                    file_path,
                    converter,
                );
            }
            Stmt::If(if_stmt) => {
                self.walk_expr_for_zod_usage(
                    if_stmt.test.as_ref(),
                    schema_name,
                    usages,
                    file_path,
                    converter,
                );
                self.walk_stmt_for_zod_usage(
                    if_stmt.cons.as_ref(),
                    schema_name,
                    usages,
                    file_path,
                    converter,
                );
                if let Some(alternate) = &if_stmt.alt {
                    self.walk_stmt_for_zod_usage(
                        alternate.as_ref(),
                        schema_name,
                        usages,
                        file_path,
                        converter,
                    );
                }
            }
            Stmt::Block(block_stmt) => {
                for stmt in &block_stmt.stmts {
                    self.walk_stmt_for_zod_usage(stmt, schema_name, usages, file_path, converter);
                }
            }
            Stmt::Return(ret_stmt) => {
                if let Some(expr) = &ret_stmt.arg {
                    self.walk_expr_for_zod_usage(
                        expr.as_ref(),
                        schema_name,
                        usages,
                        file_path,
                        converter,
                    );
                }
            }
            Stmt::Decl(swc_ecma_ast::Decl::Var(var_decl)) => {
                for decl in &var_decl.decls {
                    if let Some(init) = &decl.init {
                        self.walk_expr_for_zod_usage(
                            init.as_ref(),
                            schema_name,
                            usages,
                            file_path,
                            converter,
                        );
                    }
                }
            }
            _ => {}
        }
    }

    /// Walks Expression to find Zod schema usages
    #[allow(clippy::only_used_in_recursion)]
    fn walk_expr_for_zod_usage(
        &self,
        expr: &Expr,
        schema_name: &str,
        usages: &mut Vec<dc_core::models::ZodUsage>,
        file_path: &str,
        converter: &LocationConverter,
    ) {
        match expr {
            Expr::Call(call_expr) => {
                if let Callee::Expr(callee_expr) = &call_expr.callee {
                    if let Expr::Member(member_expr) = callee_expr.as_ref() {
                        // Check if this is schemaName.safeParse() or schemaName.parse()
                        if let Expr::Ident(ident) = member_expr.obj.as_ref() {
                            if ident.sym.as_ref() == schema_name {
                                if let MemberProp::Ident(prop) = &member_expr.prop {
                                    let method = prop.sym.as_ref();
                                    if method == "safeParse"
                                        || method == "parse"
                                        || method == "safeParseAsync"
                                        || method == "parseAsync"
                                    {
                                        let span = call_expr.span;
                                        let (line, column) =
                                            converter.byte_offset_to_location(span.lo.0 as usize);

                                        usages.push(dc_core::models::ZodUsage {
                                            schema_name: schema_name.to_string(),
                                            method: method.to_string(),
                                            location: Location {
                                                file: file_path.to_string(),
                                                line,
                                                column: Some(column),
                                            },
                                            api_call_location: None, // Will be filled later
                                        });
                                    }
                                }
                            }
                        }
                    }
                }

                // Recursively check arguments
                for arg in &call_expr.args {
                    self.walk_expr_for_zod_usage(
                        arg.expr.as_ref(),
                        schema_name,
                        usages,
                        file_path,
                        converter,
                    );
                }
            }
            Expr::Member(member_expr) => {
                self.walk_expr_for_zod_usage(
                    member_expr.obj.as_ref(),
                    schema_name,
                    usages,
                    file_path,
                    converter,
                );
            }
            _ => {}
        }
    }
}

impl Default for ZodExtractor {
    fn default() -> Self {
        Self::new()
    }
}
