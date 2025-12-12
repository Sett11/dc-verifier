use crate::models::Location;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Reference to data schema
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SchemaReference {
    /// Schema name (e.g., "UserLogin", "RegisterRequest")
    pub name: String,
    /// Schema type
    pub schema_type: SchemaType,
    /// Schema location (file and line)
    pub location: Location,
    /// Additional metadata
    pub metadata: HashMap<String, String>,
}

/// Schema type
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SchemaType {
    /// Pydantic model (Python)
    Pydantic,
    /// Zod schema (TypeScript)
    Zod,
    /// TypeScript type/interface
    TypeScript,
    /// OpenAPI schema
    OpenAPI,
    /// JSON Schema
    JsonSchema,
}

/// Type information
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TypeInfo {
    /// Base type
    pub base_type: BaseType,
    /// Schema reference (if any)
    pub schema_ref: Option<SchemaReference>,
    /// Constraints/validation
    pub constraints: Vec<Constraint>,
    /// Whether it is optional
    pub optional: bool,
}

/// Base data type
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum BaseType {
    String,
    Number,
    Integer,
    Boolean,
    Object,
    Array,
    Null,
    Any,
    Unknown,
}

/// Constraint/validation for type
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum Constraint {
    /// Minimum length (for strings) or value (for numbers)
    Min(ConstraintValue),
    /// Maximum length (for strings) or value (for numbers)
    Max(ConstraintValue),
    /// Regular expression (for strings)
    Pattern(String),
    /// Email validation
    Email,
    /// URL validation
    Url,
    /// Enum values
    Enum(Vec<String>),
}

/// Constraint value
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum ConstraintValue {
    Integer(i64),
    Float(f64),
}
