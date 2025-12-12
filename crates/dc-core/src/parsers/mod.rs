pub mod location;
pub mod openapi;
pub mod python;
pub mod typescript;

pub use location::*;
pub use openapi::*;
pub use python::*;
pub use typescript::*;

/// Module/function import
#[derive(Debug, Clone)]
pub struct Import {
    /// Import path (e.g., "fastapi" or "db.crud")
    pub path: String,
    /// Imported names (if any)
    pub names: Vec<String>,
    /// Location in code
    pub location: crate::models::Location,
}

/// Function call
#[derive(Debug, Clone)]
pub struct Call {
    /// Called function name
    pub name: String,
    /// Call arguments
    pub arguments: Vec<CallArgument>,
    /// Location in code
    pub location: crate::models::Location,
    /// Name of function/method containing the call
    pub caller: Option<String>,
}

/// Function call argument
#[derive(Debug, Clone)]
pub struct CallArgument {
    /// Parameter name (if named)
    pub parameter_name: Option<String>,
    /// Argument value (variable name or expression)
    pub value: String,
}
