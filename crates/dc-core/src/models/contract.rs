use crate::models::{Location, SchemaReference, TypeInfo};
use serde::{Deserialize, Serialize};

/// Contract between two chain links
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Contract {
    /// Source link identifier
    pub from_link_id: String,
    /// Target link identifier
    pub to_link_id: String,
    /// Source data schema
    pub from_schema: SchemaReference,
    /// Target data schema
    pub to_schema: SchemaReference,
    /// Detected mismatches
    pub mismatches: Vec<Mismatch>,
    /// Severity of issues in contract
    pub severity: Severity,
}

/// Detected mismatch at junction
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Mismatch {
    /// Mismatch type
    pub mismatch_type: MismatchType,
    /// Field path (e.g., "discount" or "client_data.full_name")
    pub path: String,
    /// Expected type/value
    pub expected: TypeInfo,
    /// Actual type/value
    pub actual: TypeInfo,
    /// Location in code
    pub location: Location,
    /// Error message
    pub message: String,
}

/// Mismatch type
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MismatchType {
    /// Type mismatch (e.g., number vs string)
    TypeMismatch,
    /// Missing required field
    MissingField,
    /// Extra field
    ExtraField,
    /// Validation mismatch (e.g., min/max)
    ValidationMismatch,
    /// Unnormalized data
    UnnormalizedData,
    /// Missing schema validation (dict[str, Any] or any)
    MissingSchema,
}

/// Problem severity
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Severity {
    /// Info (not critical)
    Info,
    /// Warning (may cause problems)
    Warning,
    /// Critical issue (will cause error)
    Critical,
}
