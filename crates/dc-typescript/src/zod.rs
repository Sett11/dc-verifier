use dc_core::models::{Location, SchemaReference, SchemaType};
use swc_ecma_ast::{CallExpr, Callee, Expr, MemberProp};

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
            if let Callee::Expr(callee_expr) = &call_expr.callee {
                // Check if this is a call to z.object() or similar
                if self.is_zod_call(callee_expr) {
                    // Extract schema name from context or create default
                    let schema_name = self.extract_schema_name(call_expr, None);

                    return Some(SchemaReference {
                        name: schema_name,
                        schema_type: SchemaType::Zod,
                        location: Location {
                            file: file_path.to_string(),
                            line,
                            column: None,
                        },
                        metadata: std::collections::HashMap::new(),
                    });
                }
            }
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

                return Some(SchemaReference {
                    name: schema_name,
                    schema_type: SchemaType::Zod,
                    location: Location {
                        file: file_path.to_string(),
                        line,
                        column: None,
                    },
                    metadata: std::collections::HashMap::new(),
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

    /// Converts Zod schema to SchemaReference
    pub fn zod_to_schema(&self, zod_schema: &str, location: Location) -> SchemaReference {
        SchemaReference {
            name: zod_schema.to_string(),
            schema_type: SchemaType::Zod,
            location,
            metadata: std::collections::HashMap::new(),
        }
    }
}

impl Default for ZodExtractor {
    fn default() -> Self {
        Self::new()
    }
}
