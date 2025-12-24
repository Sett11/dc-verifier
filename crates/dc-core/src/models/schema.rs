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
    /// ORM model (SQLAlchemy, etc.)
    OrmModel,
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

/// Information about a field in a Pydantic model
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PydanticFieldInfo {
    pub name: String,
    pub type_name: String,
    pub inner_type: Option<String>, // For list[T], dict[K, V]
    pub optional: bool,
    pub constraints: Vec<FieldConstraint>,
    pub default_value: Option<String>,
}

/// Constraint for a field
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum FieldConstraint {
    MinLength(usize),
    MaxLength(usize),
    MinValue(f64),
    MaxValue(f64),
    Pattern(String),
    Email,
    Url,
}

/// Information about a field in a SQLAlchemy model
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SQLAlchemyField {
    pub name: String,
    pub type_name: String,
    pub nullable: bool,
}

/// Information about a field in a Zod schema
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ZodField {
    pub name: String,
    pub type_name: String,
    pub optional: bool,
    pub nullable: bool,
}

/// Information about Zod schema usage
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ZodUsage {
    /// Name of the Zod schema being used
    pub schema_name: String,
    /// Method called on the schema (safeParse, parse, etc.)
    pub method: String,
    /// Location where the schema is used
    pub location: Location,
    /// Optional: associated API call (if found nearby)
    pub api_call_location: Option<Location>,
}

/// Information about field mismatch between Zod and Pydantic schemas
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FieldMismatch {
    /// Field name
    pub field_name: String,
    /// Type in Zod schema
    pub zod_type: String,
    /// Type in Pydantic model
    pub pydantic_type: String,
    /// Reason for mismatch
    pub reason: String,
}

/// Type of data transformation between schemas / representations
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum TransformationType {
    /// Construct Pydantic model from a dict / mapping
    FromDict,
    /// Construct Pydantic model from JSON string
    FromJson,
    /// Construct Pydantic model from arbitrary object via ORM mapping
    FromOrm,
    /// Construct Pydantic model from attributes / ORM model (heuristic)
    FromAttributes,
    /// Validate data and construct Pydantic model (model_validate, parse_obj, etc.)
    ValidateData,
    /// Validate JSON payload and construct Pydantic model (model_validate_json, parse_raw, etc.)
    ValidateJson,
    /// Dump Pydantic model to a dict / mapping
    ToDict,
    /// Dump Pydantic model to JSON string
    ToJson,
    /// Generic serialization of Pydantic model (model_dump / model_serialize)
    Serialize,
    /// ORM model → Pydantic model (bidirectional ORM bridge)
    OrmToPydantic,
    /// Pydantic model → ORM model (bidirectional ORM bridge)
    PydanticToOrm,
}
