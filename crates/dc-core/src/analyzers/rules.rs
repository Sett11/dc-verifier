use crate::analyzers::schema_parser::SchemaParser;
use crate::models::{BaseType, Contract, Mismatch, MismatchType, SeverityLevel, TypeInfo};

/// Trait for contract checking rules
pub trait ContractRule: Send + Sync {
    /// Checks contract and returns found mismatches
    fn check(&self, contract: &Contract) -> Vec<Mismatch>;

    /// Rule name
    fn name(&self) -> &str;
}

/// Type mismatch checking rule
pub struct TypeMismatchRule;

impl ContractRule for TypeMismatchRule {
    fn check(&self, contract: &Contract) -> Vec<Mismatch> {
        let mut mismatches = Vec::new();

        // Parse schemas
        let Ok(from_schema) = SchemaParser::parse(&contract.from_schema) else {
            return mismatches;
        };
        let Ok(to_schema) = SchemaParser::parse(&contract.to_schema) else {
            return mismatches;
        };

        // Compare field types
        for (field_name, from_field) in &from_schema.properties {
            if let Some(to_field) = to_schema.properties.get(field_name) {
                // Check type mismatch
                if from_field.base_type != to_field.base_type {
                    mismatches.push(Mismatch {
                        mismatch_type: MismatchType::TypeMismatch,
                        path: field_name.clone(),
                        expected: TypeInfo {
                            base_type: from_field.base_type,
                            schema_ref: None,
                            constraints: from_field.constraints.clone(),
                            optional: from_field.optional,
                        },
                        actual: TypeInfo {
                            base_type: to_field.base_type,
                            schema_ref: None,
                            constraints: to_field.constraints.clone(),
                            optional: to_field.optional,
                        },
                        location: contract.to_schema.location.clone(),
                        message: format!(
                            "Type mismatch for field '{}': expected {:?}, got {:?}",
                            field_name, from_field.base_type, to_field.base_type
                        ),
                        severity_level: SeverityLevel::High,
                    });
                }
            }
        }

        mismatches
    }

    fn name(&self) -> &str {
        "type_mismatch"
    }
}

/// Missing field checking rule
pub struct MissingFieldRule;

impl ContractRule for MissingFieldRule {
    fn check(&self, contract: &Contract) -> Vec<Mismatch> {
        let mut mismatches = Vec::new();

        // Parse schemas
        let Ok(from_schema) = SchemaParser::parse(&contract.from_schema) else {
            return mismatches;
        };
        let Ok(to_schema) = SchemaParser::parse(&contract.to_schema) else {
            return mismatches;
        };

        // Check required fields in target schema
        for required_field in &to_schema.required {
            if !from_schema.properties.contains_key(required_field) {
                // Field is missing in source schema
                let to_field = to_schema.properties.get(required_field);
                mismatches.push(Mismatch {
                    mismatch_type: MismatchType::MissingField,
                    path: required_field.clone(),
                    expected: TypeInfo {
                        base_type: to_field.map(|f| f.base_type).unwrap_or(BaseType::Unknown),
                        schema_ref: None,
                        constraints: to_field.map(|f| f.constraints.clone()).unwrap_or_default(),
                        optional: false, // Required field
                    },
                    actual: TypeInfo {
                        base_type: BaseType::Unknown,
                        schema_ref: None,
                        constraints: Vec::new(),
                        optional: true,
                    },
                    location: contract.from_schema.location.clone(),
                    message: format!(
                        "Missing required field '{}' in source schema",
                        required_field
                    ),
                    severity_level: SeverityLevel::High,
                });
            }
        }

        // Also check fields that exist in to_schema but are missing in from_schema
        // (if they are not optional)
        for (field_name, to_field) in &to_schema.properties {
            // Additional check: ensure field is not already in required list
            // This prevents duplicate mismatches for the same field
            if !to_field.optional
                && !from_schema.properties.contains_key(field_name)
                && !to_schema.required.contains(field_name)
            {
                // Add to required if not already there
                mismatches.push(Mismatch {
                    mismatch_type: MismatchType::MissingField,
                    path: field_name.clone(),
                    expected: TypeInfo {
                        base_type: to_field.base_type,
                        schema_ref: None,
                        constraints: to_field.constraints.clone(),
                        optional: false,
                    },
                    actual: TypeInfo {
                        base_type: BaseType::Unknown,
                        schema_ref: None,
                        constraints: Vec::new(),
                        optional: true,
                    },
                    location: contract.from_schema.location.clone(),
                    message: format!("Missing required field '{}' in source schema", field_name),
                    severity_level: SeverityLevel::High,
                });
            }
        }

        mismatches
    }

    fn name(&self) -> &str {
        "missing_field"
    }
}

/// Unnormalized data checking rule
pub struct UnnormalizedDataRule;

impl ContractRule for UnnormalizedDataRule {
    fn check(&self, contract: &Contract) -> Vec<Mismatch> {
        let mut mismatches = Vec::new();

        // Parse schemas
        let Ok(from_schema) = SchemaParser::parse(&contract.from_schema) else {
            return mismatches;
        };
        let Ok(to_schema) = SchemaParser::parse(&contract.to_schema) else {
            return mismatches;
        };

        // Check fields that require normalization
        for (field_name, from_field) in &from_schema.properties {
            if let Some(to_field) = to_schema.properties.get(field_name) {
                // Check normalization constraints
                let from_has_email = from_field
                    .constraints
                    .iter()
                    .any(|c| matches!(c, crate::models::Constraint::Email));
                let to_has_email = to_field
                    .constraints
                    .iter()
                    .any(|c| matches!(c, crate::models::Constraint::Email));

                // If target requires email but source has no email validation
                // or vice versa - this may be a normalization problem
                if to_has_email && !from_has_email && from_field.base_type == BaseType::String {
                    mismatches.push(Mismatch {
                        mismatch_type: MismatchType::UnnormalizedData,
                        path: field_name.clone(),
                        expected: TypeInfo {
                            base_type: to_field.base_type,
                            schema_ref: None,
                            constraints: to_field.constraints.clone(),
                            optional: to_field.optional,
                        },
                        actual: TypeInfo {
                            base_type: from_field.base_type,
                            schema_ref: None,
                            constraints: from_field.constraints.clone(),
                            optional: from_field.optional,
                        },
                        location: contract.from_schema.location.clone(),
                        message: format!(
                            "Field '{}' may require normalization (email format expected)",
                            field_name
                        ),
                        severity_level: SeverityLevel::Medium,
                    });
                }

                // Check other constraints that may require normalization
                // For example, strings should be lowercase
                let from_has_pattern = from_field
                    .constraints
                    .iter()
                    .any(|c| matches!(c, crate::models::Constraint::Pattern(_)));
                let to_has_pattern = to_field
                    .constraints
                    .iter()
                    .any(|c| matches!(c, crate::models::Constraint::Pattern(_)));
                if to_has_pattern && !from_has_pattern && from_field.base_type == BaseType::String {
                    // If target has pattern but source doesn't - possible problem
                    mismatches.push(Mismatch {
                        mismatch_type: MismatchType::UnnormalizedData,
                        path: field_name.clone(),
                        expected: TypeInfo {
                            base_type: to_field.base_type,
                            schema_ref: None,
                            constraints: to_field.constraints.clone(),
                            optional: to_field.optional,
                        },
                        actual: TypeInfo {
                            base_type: from_field.base_type,
                            schema_ref: None,
                            constraints: from_field.constraints.clone(),
                            optional: from_field.optional,
                        },
                        location: contract.from_schema.location.clone(),
                        message: format!(
                            "Field '{}' may require normalization (pattern validation expected)",
                            field_name
                        ),
                        severity_level: SeverityLevel::Medium,
                    });
                }
            }
        }

        mismatches
    }

    fn name(&self) -> &str {
        "unnormalized_data"
    }
}

/// Missing schema checking rule
pub struct MissingSchemaRule;

impl ContractRule for MissingSchemaRule {
    fn check(&self, contract: &Contract) -> Vec<Mismatch> {
        let mut mismatches = Vec::new();

        // Check from_schema
        // If missing schema in source (from_schema), it's likely a request parameter → Critical
        if contract.from_schema.metadata.contains_key("missing_schema") {
            mismatches.push(Mismatch {
                mismatch_type: MismatchType::MissingSchema,
                path: "".to_string(),
                expected: TypeInfo {
                    base_type: BaseType::Object,
                    schema_ref: None,
                    constraints: Vec::new(),
                    optional: false,
                },
                actual: TypeInfo {
                    base_type: BaseType::Any,
                    schema_ref: None,
                    constraints: Vec::new(),
                    optional: false,
                },
                location: contract.from_schema.location.clone(),
                message: format!(
                    "Source schema '{}' is missing validation schema (dict[str, Any] or any)",
                    contract.from_schema.name
                ),
                severity_level: SeverityLevel::Critical,
            });
        }

        // Check to_schema
        // If missing schema in target (to_schema), it's likely a response → High
        if contract.to_schema.metadata.contains_key("missing_schema") {
            mismatches.push(Mismatch {
                mismatch_type: MismatchType::MissingSchema,
                path: "".to_string(),
                expected: TypeInfo {
                    base_type: BaseType::Object,
                    schema_ref: None,
                    constraints: Vec::new(),
                    optional: false,
                },
                actual: TypeInfo {
                    base_type: BaseType::Any,
                    schema_ref: None,
                    constraints: Vec::new(),
                    optional: false,
                },
                location: contract.to_schema.location.clone(),
                message: format!(
                    "Target schema '{}' is missing validation schema (dict[str, Any] or any)",
                    contract.to_schema.name
                ),
                severity_level: SeverityLevel::High,
            });
        }

        mismatches
    }

    fn name(&self) -> &str {
        "missing_schema"
    }
}
