use crate::call_graph::HttpMethod;
use anyhow::Result;
use rustpython_parser::ast;
use std::path::Path;

/// Information about a dynamically generated endpoint
#[derive(Debug, Clone)]
pub struct DynamicEndpoint {
    pub path: String,
    pub method: HttpMethod,
    pub request_schema: Option<String>,
    pub response_schema: Option<String>,
    /// Index of argument for request schema (if any)
    pub request_schema_param_index: Option<usize>,
    /// Index of argument for response schema (if any)
    pub response_schema_param_index: Option<usize>,
}

/// Trait for router generators from various libraries
pub trait RouterGenerator: Send + Sync {
    /// Module name (e.g., "fastapi_users", "fastapi_limiter")
    fn module_name(&self) -> &str;

    /// Check if this generator can handle the given call expression
    fn can_handle(&self, call_expr: &ast::ExprCall) -> bool;

    /// Analyze the call and return list of generated endpoints
    fn analyze_call(
        &self,
        call_expr: &ast::ExprCall,
        current_file: &Path,
        file_ast: Option<&ast::Mod>,
    ) -> Result<Vec<DynamicEndpoint>>;

    /// Extract schema names from call arguments
    fn extract_schemas(&self, call_expr: &ast::ExprCall) -> Vec<String>;
}
