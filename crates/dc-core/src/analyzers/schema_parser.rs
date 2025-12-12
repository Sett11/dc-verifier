use crate::models::{BaseType, Constraint, ConstraintValue, SchemaReference, SchemaType};
use anyhow::Result;
use serde_json::Value;
use std::collections::HashMap;

/// JSON Schema representation for comparison
#[derive(Debug, Clone, PartialEq)]
pub struct JsonSchema {
    /// Schema type (object, array, string, number, etc.)
    pub schema_type: String,
    /// Object fields (for type="object")
    pub properties: HashMap<String, FieldInfo>,
    /// Required fields
    pub required: Vec<String>,
    /// Array items (for type="array")
    pub items: Option<Box<JsonSchema>>,
    /// Additional constraints
    pub constraints: Vec<Constraint>,
}

/// Field information in schema
#[derive(Debug, Clone, PartialEq)]
pub struct FieldInfo {
    /// Field type
    pub field_type: String,
    /// Base type (for comparison)
    pub base_type: BaseType,
    /// Whether it is optional
    pub optional: bool,
    /// Constraints/validation
    pub constraints: Vec<Constraint>,
    /// Nested schema (for objects)
    pub nested_schema: Option<Box<JsonSchema>>,
}

/// Schema parser from SchemaReference
pub struct SchemaParser;

impl SchemaParser {
    /// Parses SchemaReference into JsonSchema
    pub fn parse(schema_ref: &SchemaReference) -> Result<JsonSchema> {
        match schema_ref.schema_type {
            SchemaType::Pydantic => Self::parse_pydantic(schema_ref),
            SchemaType::Zod => Self::parse_zod(schema_ref),
            SchemaType::TypeScript => Self::parse_typescript(schema_ref),
            SchemaType::OpenAPI => Self::parse_openapi(schema_ref),
            SchemaType::JsonSchema => Self::parse_json_schema(schema_ref),
        }
    }

    /// Parses Pydantic schema
    fn parse_pydantic(schema_ref: &SchemaReference) -> Result<JsonSchema> {
        // Check if full JSON schema is in metadata
        if let Some(json_schema_str) = schema_ref.metadata.get("json_schema") {
            // Use full JSON schema
            let json_value: Value = serde_json::from_str(json_schema_str)?;
            let mut schema = Self::parse_json_value(&json_value)?;

            // Synchronize optional flags with required
            for (field_name, field_info) in schema.properties.iter_mut() {
                field_info.optional = !schema.required.contains(field_name);
            }

            return Ok(schema);
        }

        // Fallback: use metadata
        let mut properties = HashMap::new();
        let mut required = Vec::new();

        // Extract required from metadata if present
        if let Some(required_str) = schema_ref.metadata.get("required") {
            for field in required_str.split(',') {
                let field = field.trim();
                if !field.is_empty() {
                    required.push(field.to_string());
                }
            }
        }

        // Try to extract information from metadata
        if let Some(fields_str) = schema_ref.metadata.get("fields") {
            // Parse fields from metadata: split only by first ':'
            for field in fields_str.split(',') {
                let field = field.trim();
                if field.is_empty() {
                    continue;
                }

                // Split only by first ':'
                if let Some(colon_pos) = field.find(':') {
                    let name = field[..colon_pos].trim().to_string();
                    let field_type = field[colon_pos + 1..].trim().to_string();

                    // Skip empty names or types
                    if name.is_empty() || field_type.is_empty() {
                        continue;
                    }

                    properties.insert(
                        name.clone(),
                        FieldInfo {
                            field_type: field_type.clone(),
                            base_type: Self::base_type_from_string(&field_type),
                            optional: true, // By default fields are optional
                            constraints: Vec::new(),
                            nested_schema: None,
                        },
                    );
                }
            }
        }

        // Synchronize optional and required: if required is empty, all fields optional=true
        // Otherwise set optional=false for fields in required
        if required.is_empty() {
            // If required is empty, all fields optional
            for field_info in properties.values_mut() {
                field_info.optional = true;
            }
        } else {
            // Set optional=false for fields in required
            for field_name in &required {
                if let Some(field_info) = properties.get_mut(field_name) {
                    field_info.optional = false;
                }
            }
            // Other fields optional=true
            for (field_name, field_info) in properties.iter_mut() {
                if !required.contains(field_name) {
                    field_info.optional = true;
                }
            }
        }

        Ok(JsonSchema {
            schema_type: "object".to_string(),
            properties,
            required,
            items: None,
            constraints: Vec::new(),
        })
    }

    /// Parses Zod schema
    fn parse_zod(schema_ref: &SchemaReference) -> Result<JsonSchema> {
        // Similar to Pydantic
        Self::parse_pydantic(schema_ref)
    }

    /// Parses TypeScript schema
    fn parse_typescript(schema_ref: &SchemaReference) -> Result<JsonSchema> {
        // Check if JSON schema is in metadata (if TypeScript schema was converted)
        if let Some(json_schema_str) = schema_ref.metadata.get("json_schema") {
            let json_value: Value = serde_json::from_str(json_schema_str)?;
            let mut schema = Self::parse_json_value(&json_value)?;

            // Synchronize optional flags with required
            for (field_name, field_info) in schema.properties.iter_mut() {
                field_info.optional = !schema.required.contains(field_name);
            }

            return Ok(schema);
        }

        // Extract fields from metadata (format: "name:type:optional" or "name:type")
        let mut properties = HashMap::new();
        let mut required = Vec::new();

        if let Some(fields_str) = schema_ref.metadata.get("fields") {
            for field in fields_str.split(',') {
                let field = field.trim();
                if field.is_empty() {
                    continue;
                }

                // Split by ':'
                let parts: Vec<&str> = field.split(':').collect();
                if parts.len() >= 2 {
                    let name = parts[0].trim().to_string();
                    let field_type = parts[1].trim().to_string();
                    let optional = parts
                        .get(2)
                        .map(|s| s.trim() == "optional")
                        .unwrap_or(false);

                    if !name.is_empty() && !field_type.is_empty() {
                        let base_type = Self::base_type_from_string(&field_type);
                        let field_info = FieldInfo {
                            field_type,
                            base_type,
                            optional,
                            constraints: Vec::new(),
                            nested_schema: None,
                        };
                        properties.insert(name.clone(), field_info);

                        if !optional {
                            required.push(name);
                        }
                    }
                }
            }
        }

        // If there is a type in metadata (for type aliases)
        if let Some(type_str) = schema_ref.metadata.get("type") {
            let base_type = Self::base_type_from_string(type_str);
            let schema_type = match base_type {
                BaseType::String => "string",
                BaseType::Number => "number",
                BaseType::Integer => "integer",
                BaseType::Boolean => "boolean",
                BaseType::Object => "object",
                BaseType::Array => "array",
                BaseType::Null => "null",
                BaseType::Any => "any",
                BaseType::Unknown => "unknown",
            };
            return Ok(JsonSchema {
                schema_type: schema_type.to_string(),
                properties: HashMap::new(),
                required: Vec::new(),
                items: None,
                constraints: Vec::new(),
            });
        }

        Ok(JsonSchema {
            schema_type: "object".to_string(),
            properties,
            required,
            items: None,
            constraints: Vec::new(),
        })
    }

    /// Parses OpenAPI schema
    fn parse_openapi(schema_ref: &SchemaReference) -> Result<JsonSchema> {
        Self::parse_json_schema(schema_ref)
    }

    /// Parses JSON Schema
    fn parse_json_schema(schema_ref: &SchemaReference) -> Result<JsonSchema> {
        // Extract JSON Schema from metadata
        let json_schema_str = schema_ref
            .metadata
            .get("json_schema")
            .ok_or_else(|| anyhow::anyhow!("JSON schema not found in metadata"))?;

        // Deserialize JSON
        let json_value: Value = serde_json::from_str(json_schema_str)?;

        Self::parse_json_value(&json_value)
    }

    /// Parses JSON Schema from Value
    fn parse_json_value(json_value: &Value) -> Result<JsonSchema> {
        let schema_type = json_value
            .get("type")
            .and_then(|v| v.as_str())
            .unwrap_or("object")
            .to_string();

        let mut properties = HashMap::new();
        let mut required = Vec::new();
        let mut constraints = Vec::new();

        // Extract properties for objects
        if let Some(props) = json_value.get("properties").and_then(|v| v.as_object()) {
            for (name, prop_value) in props {
                let field_info = Self::parse_property(prop_value)?;
                properties.insert(name.clone(), field_info);
            }
        }

        // Extract required fields
        if let Some(req) = json_value.get("required").and_then(|v| v.as_array()) {
            for field in req {
                if let Some(name) = field.as_str() {
                    required.push(name.to_string());
                }
            }
        }

        // Extract constraints
        if let Some(min) = json_value.get("minimum").and_then(|v| v.as_f64()) {
            constraints.push(Constraint::Min(ConstraintValue::Float(min)));
        }
        if let Some(max) = json_value.get("maximum").and_then(|v| v.as_f64()) {
            constraints.push(Constraint::Max(ConstraintValue::Float(max)));
        }
        if let Some(min_len) = json_value.get("minLength").and_then(|v| v.as_u64()) {
            constraints.push(Constraint::Min(ConstraintValue::Integer(min_len as i64)));
        }
        if let Some(max_len) = json_value.get("maxLength").and_then(|v| v.as_u64()) {
            constraints.push(Constraint::Max(ConstraintValue::Integer(max_len as i64)));
        }
        if let Some(pattern) = json_value.get("pattern").and_then(|v| v.as_str()) {
            constraints.push(Constraint::Pattern(pattern.to_string()));
        }
        if json_value.get("format").and_then(|v| v.as_str()) == Some("email") {
            constraints.push(Constraint::Email);
        }
        if json_value.get("format").and_then(|v| v.as_str()) == Some("uri") {
            constraints.push(Constraint::Url);
        }
        if let Some(enum_values) = json_value.get("enum").and_then(|v| v.as_array()) {
            let enum_strings: Vec<String> = enum_values
                .iter()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .collect();
            if !enum_strings.is_empty() {
                constraints.push(Constraint::Enum(enum_strings));
            }
        }

        // Extract items for arrays
        let items = if let Some(items_value) = json_value.get("items") {
            Some(Box::new(Self::parse_json_value(items_value)?))
        } else {
            None
        };

        Ok(JsonSchema {
            schema_type,
            properties,
            required,
            items,
            constraints,
        })
    }

    /// Parses property from JSON Schema
    fn parse_property(prop_value: &Value) -> Result<FieldInfo> {
        let field_type = prop_value
            .get("type")
            .and_then(|v| v.as_str())
            .unwrap_or("any")
            .to_string();

        let base_type = Self::base_type_from_string(&field_type);

        let mut constraints = Vec::new();

        // Extract constraints for field
        if let Some(min) = prop_value.get("minimum").and_then(|v| v.as_f64()) {
            constraints.push(Constraint::Min(ConstraintValue::Float(min)));
        }
        if let Some(max) = prop_value.get("maximum").and_then(|v| v.as_f64()) {
            constraints.push(Constraint::Max(ConstraintValue::Float(max)));
        }
        if let Some(min_len) = prop_value.get("minLength").and_then(|v| v.as_u64()) {
            constraints.push(Constraint::Min(ConstraintValue::Integer(min_len as i64)));
        }
        if let Some(max_len) = prop_value.get("maxLength").and_then(|v| v.as_u64()) {
            constraints.push(Constraint::Max(ConstraintValue::Integer(max_len as i64)));
        }
        if let Some(pattern) = prop_value.get("pattern").and_then(|v| v.as_str()) {
            constraints.push(Constraint::Pattern(pattern.to_string()));
        }

        // Check nested schema (for objects)
        let nested_schema = if field_type == "object" {
            Some(Box::new(Self::parse_json_value(prop_value)?))
        } else {
            None
        };

        Ok(FieldInfo {
            field_type,
            base_type,
            optional: true, // Will be set later based on required
            constraints,
            nested_schema,
        })
    }

    /// Converts string type to BaseType
    fn base_type_from_string(type_str: &str) -> BaseType {
        match type_str.to_lowercase().as_str() {
            "str" | "string" => BaseType::String,
            "int" | "integer" => BaseType::Integer,
            "number" | "float" | "double" => BaseType::Number,
            "bool" | "boolean" => BaseType::Boolean,
            "list" | "array" => BaseType::Array,
            "dict" | "object" => BaseType::Object,
            "null" | "none" => BaseType::Null,
            _ => BaseType::Unknown,
        }
    }
}
