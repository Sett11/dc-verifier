use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// OpenAPI schema representation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpenAPISchema {
    pub openapi: String,
    pub info: OpenAPIInfo,
    pub paths: HashMap<String, PathItem>,
    #[serde(default)]
    pub components: Option<Components>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpenAPIInfo {
    pub title: String,
    pub version: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PathItem {
    #[serde(flatten)]
    pub operations: HashMap<String, Operation>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Operation {
    pub operation_id: Option<String>,
    pub summary: Option<String>,
    pub tags: Option<Vec<String>>,
    #[serde(default)]
    pub request_body: Option<RequestBody>,
    #[serde(default)]
    pub responses: HashMap<String, Response>,
    #[serde(default)]
    pub parameters: Vec<Parameter>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RequestBody {
    pub required: Option<bool>,
    pub content: HashMap<String, MediaType>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MediaType {
    pub schema: Option<SchemaRef>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Response {
    pub description: Option<String>,
    pub content: Option<HashMap<String, MediaType>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Parameter {
    pub name: String,
    #[serde(rename = "in")]
    pub location: String, // "query", "path", "header", "cookie"
    pub required: Option<bool>,
    pub schema: Option<SchemaRef>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Components {
    pub schemas: Option<HashMap<String, SchemaRef>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum SchemaRef {
    Ref(Reference),
    Inline(Box<Schema>),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Reference {
    #[serde(rename = "$ref")]
    pub ref_path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Schema {
    Object(ObjectSchema),
    Array(ArraySchema),
    Primitive(PrimitiveSchema),
    AllOf(AllOfSchema),
    OneOf(OneOfSchema),
    AnyOf(AnyOfSchema),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ObjectSchema {
    #[serde(rename = "type")]
    pub schema_type: Option<String>,
    pub properties: Option<HashMap<String, SchemaRef>>,
    pub required: Option<Vec<String>>,
    #[serde(rename = "additionalProperties")]
    pub additional_properties: Option<bool>,
    pub title: Option<String>,
    pub description: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArraySchema {
    #[serde(rename = "type")]
    pub schema_type: Option<String>,
    pub items: Option<Box<SchemaRef>>,
    pub title: Option<String>,
    pub description: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PrimitiveSchema {
    #[serde(rename = "type")]
    pub schema_type: Option<String>,
    pub format: Option<String>,
    pub title: Option<String>,
    pub description: Option<String>,
    #[serde(default)]
    pub enum_values: Option<Vec<serde_json::Value>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AllOfSchema {
    #[serde(rename = "allOf")]
    pub all_of: Vec<SchemaRef>,
    pub title: Option<String>,
    pub description: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OneOfSchema {
    #[serde(rename = "oneOf")]
    pub one_of: Vec<SchemaRef>,
    pub title: Option<String>,
    pub description: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnyOfSchema {
    #[serde(rename = "anyOf")]
    pub any_of: Vec<SchemaRef>,
    pub title: Option<String>,
    pub description: Option<String>,
}

/// Represents an OpenAPI endpoint
#[derive(Debug, Clone)]
pub struct OpenAPIEndpoint {
    pub path: String,
    pub method: String, // "get", "post", "put", "delete", etc.
    pub operation_id: Option<String>,
    pub request_schema: Option<String>, // Schema name from components/schemas
    pub response_schema: Option<String>, // Schema name from components/schemas
    pub response_code: Option<String>,  // "200", "201", etc.
}

/// Represents an OpenAPI schema component
#[derive(Debug, Clone)]
pub struct OpenAPISchemaComponent {
    pub name: String,
    pub schema: Schema,
    pub properties: Vec<(String, String)>, // (property_name, schema_name_or_type)
}
