use crate::models::{Location, TypeInfo};
use serde::{Deserialize, Serialize};

/// Variable in code
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Variable {
    /// Variable name
    pub name: String,
    /// Type information
    pub type_info: TypeInfo,
    /// Location in code
    pub location: Location,
    /// Variable source
    pub source: VariableSource,
}

/// Variable source
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum VariableSource {
    /// Function parameter
    Parameter,
    /// Function return value
    Return,
    /// Imported variable
    Import,
    /// Local variable
    Local,
    /// Object field
    Field,
}
