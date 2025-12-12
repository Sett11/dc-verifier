use crate::models::SchemaReference;
use anyhow::Result;
use serde_json::Value;

/// OpenAPI specification parser for linking frontend/backend
pub struct OpenApiParser {
    spec: Value,
}

impl OpenApiParser {
    /// Creates parser from JSON string
    pub fn from_json(spec_json: &str) -> Result<Self> {
        let spec: Value = serde_json::from_str(spec_json)?;
        Ok(Self { spec })
    }

    /// Extracts all endpoints from specification
    pub fn extract_endpoints(&self) -> Vec<ApiEndpoint> {
        let mut endpoints = Vec::new();

        // Valid HTTP methods (case-insensitive)
        let valid_methods = [
            "get", "put", "post", "delete", "options", "head", "patch", "trace",
        ];

        if let Some(paths) = self.spec.get("paths").and_then(|p| p.as_object()) {
            for (path, path_item) in paths {
                if let Some(path_item_obj) = path_item.as_object() {
                    for (method_key, operation) in path_item_obj {
                        // Filter only valid HTTP methods, skipping non-methods ($ref, summary, description, servers, parameters)
                        let method_lower = method_key.to_lowercase();
                        if !valid_methods.contains(&method_lower.as_str()) {
                            continue;
                        }

                        if let Some(operation_obj) = operation.as_object() {
                            endpoints.push(ApiEndpoint {
                                path: path.clone(),
                                method: method_key.to_uppercase(),
                                operation_id: operation_obj
                                    .get("operationId")
                                    .and_then(|id| id.as_str())
                                    .map(|s| s.to_string()),
                                request_schema: self.extract_request_schema(operation_obj),
                                response_schema: self.extract_response_schema(operation_obj),
                            });
                        }
                    }
                }
            }
        }

        endpoints
    }

    fn extract_request_schema(
        &self,
        operation: &serde_json::Map<String, Value>,
    ) -> Option<SchemaReference> {
        operation
            .get("requestBody")?
            .get("content")?
            .get("application/json")?
            .get("schema")?
            .get("$ref")
            .and_then(|r| r.as_str())
            .map(|s| SchemaReference {
                name: s.to_string(),
                schema_type: crate::models::SchemaType::OpenAPI,
                location: crate::models::Location {
                    file: String::new(),
                    line: 0,
                    column: None,
                },
                metadata: std::collections::HashMap::new(),
            })
    }

    fn extract_response_schema(
        &self,
        operation: &serde_json::Map<String, Value>,
    ) -> Option<SchemaReference> {
        operation
            .get("responses")?
            .get("200")?
            .get("content")?
            .get("application/json")?
            .get("schema")?
            .get("$ref")
            .and_then(|r| r.as_str())
            .map(|s| SchemaReference {
                name: s.to_string(),
                schema_type: crate::models::SchemaType::OpenAPI,
                location: crate::models::Location {
                    file: String::new(),
                    line: 0,
                    column: None,
                },
                metadata: std::collections::HashMap::new(),
            })
    }
}

/// API endpoint from OpenAPI specification
#[derive(Debug, Clone)]
pub struct ApiEndpoint {
    pub path: String,
    pub method: String,
    pub operation_id: Option<String>,
    pub request_schema: Option<SchemaReference>,
    pub response_schema: Option<SchemaReference>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_parse_minimal_valid_openapi() {
        let spec_json = json!({
            "openapi": "3.0.0",
            "paths": {
                "/users": {
                    "get": {
                        "operationId": "getUsers",
                        "responses": {
                            "200": {
                                "description": "Success"
                            }
                        }
                    }
                }
            }
        });

        let parser = OpenApiParser::from_json(&spec_json.to_string()).unwrap();
        let endpoints = parser.extract_endpoints();

        assert_eq!(endpoints.len(), 1);
        assert_eq!(endpoints[0].path, "/users");
        assert_eq!(endpoints[0].method, "GET");
        assert_eq!(endpoints[0].operation_id, Some("getUsers".to_string()));
    }

    #[test]
    fn test_multiple_http_methods() {
        let spec_json = json!({
            "openapi": "3.0.0",
            "paths": {
                "/users": {
                    "get": {
                        "operationId": "getUsers",
                        "responses": {"200": {"description": "Success"}}
                    },
                    "post": {
                        "operationId": "createUser",
                        "responses": {"200": {"description": "Success"}}
                    }
                }
            }
        });

        let parser = OpenApiParser::from_json(&spec_json.to_string()).unwrap();
        let endpoints = parser.extract_endpoints();

        assert_eq!(endpoints.len(), 2);
        let methods: Vec<&str> = endpoints.iter().map(|e| e.method.as_str()).collect();
        assert!(methods.contains(&"GET"));
        assert!(methods.contains(&"POST"));
    }

    #[test]
    fn test_missing_optional_fields() {
        let spec_json = json!({
            "openapi": "3.0.0",
            "paths": {
                "/users": {
                    "get": {
                        "responses": {"200": {"description": "Success"}}
                    }
                }
            }
        });

        let parser = OpenApiParser::from_json(&spec_json.to_string()).unwrap();
        let endpoints = parser.extract_endpoints();

        assert_eq!(endpoints.len(), 1);
        assert_eq!(endpoints[0].operation_id, None);
        assert_eq!(endpoints[0].request_schema, None);
    }

    #[test]
    fn test_malformed_json() {
        let result = OpenApiParser::from_json("{ invalid json }");
        assert!(result.is_err());
    }

    #[test]
    fn test_empty_paths() {
        let spec_json = json!({
            "openapi": "3.0.0",
            "paths": {}
        });

        let parser = OpenApiParser::from_json(&spec_json.to_string()).unwrap();
        let endpoints = parser.extract_endpoints();
        assert_eq!(endpoints.len(), 0);
    }

    #[test]
    fn test_empty_path_item() {
        let spec_json = json!({
            "openapi": "3.0.0",
            "paths": {
                "/users": {}
            }
        });

        let parser = OpenApiParser::from_json(&spec_json.to_string()).unwrap();
        let endpoints = parser.extract_endpoints();
        assert_eq!(endpoints.len(), 0);
    }

    #[test]
    fn test_filter_non_method_keys() {
        let spec_json = json!({
            "openapi": "3.0.0",
            "paths": {
                "/users": {
                    "summary": "Users endpoint",
                    "description": "User management",
                    "get": {
                        "operationId": "getUsers",
                        "responses": {"200": {"description": "Success"}}
                    },
                    "$ref": "#/components/pathItems/Users",
                    "parameters": []
                }
            }
        });

        let parser = OpenApiParser::from_json(&spec_json.to_string()).unwrap();
        let endpoints = parser.extract_endpoints();

        // Should be only one endpoint (get), non-methods should be skipped
        assert_eq!(endpoints.len(), 1);
        assert_eq!(endpoints[0].method, "GET");
    }
}
