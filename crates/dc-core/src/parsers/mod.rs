pub mod location;
pub mod openapi;
pub mod python;
pub mod typescript;

pub use location::*;
pub use openapi::*;
pub use python::*;
pub use typescript::*;

// Re-export TypeScript-specific types
pub use typescript::{DecoratorTarget, TypeScriptDecorator};

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
    /// Generic type parameters (для useQuery<ResponseType, ErrorType>)
    pub generic_params: Vec<crate::models::TypeInfo>,
    /// Location in code
    pub location: crate::models::Location,
    /// Name of function/method containing the call
    pub caller: Option<String>,
    /// Base object name for member expressions (e.g., "client" in "client.get()")
    pub base_object: Option<String>,
    /// Property name for member expressions (e.g., "get" in "client.get()")
    pub property: Option<String>,
    /// Whether the call uses optional chaining (?.)
    pub uses_optional_chaining: bool,
}

/// Function call argument
#[derive(Debug, Clone)]
pub struct CallArgument {
    /// Parameter name (if named)
    pub parameter_name: Option<String>,
    /// Argument value (variable name or expression)
    pub value: String,
}

/// Information about a function found in module
#[derive(Debug, Clone)]
pub struct FunctionInfo {
    /// Function name
    pub name: String,
    /// Function parameters
    pub parameters: Vec<crate::call_graph::Parameter>,
    /// Return type (if known)
    pub return_type: Option<crate::models::TypeInfo>,
    /// Whether function is async
    pub is_async: bool,
    /// Location in code
    pub location: crate::models::Location,
}
