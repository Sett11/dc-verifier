use crate::openapi::schema::*;
use anyhow::{Context, Result};
use serde_json;
use serde_yaml;
use std::collections::HashMap;
use std::path::Path;

/// Parser for OpenAPI schema files
pub struct OpenAPIParser;

impl OpenAPIParser {
    /// Parses an OpenAPI file (JSON or YAML)
    pub fn parse_file(openapi_path: &Path) -> Result<OpenAPISchema> {
        let content = std::fs::read_to_string(openapi_path)
            .with_context(|| format!("Failed to read OpenAPI file: {:?}", openapi_path))?;

        // Определить формат по расширению файла
        let extension = openapi_path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("");

        match extension {
            "yaml" | "yml" => Self::parse_yaml_str(&content),
            "json" => Self::parse_json_str(&content),
            _ => {
                // Попробовать определить по содержимому
                let trimmed = content.trim_start();
                if trimmed.starts_with('{') {
                    // JSON начинается с {
                    Self::parse_json_str(&content)
                } else if trimmed.starts_with("---")
                    || trimmed.starts_with("openapi:")
                    || trimmed.starts_with("swagger:")
                {
                    // YAML может начинаться с --- или с openapi:/swagger:
                    Self::parse_yaml_str(&content)
                } else {
                    // По умолчанию попробовать JSON, затем YAML
                    Self::parse_json_str(&content)
                        .or_else(|_| Self::parse_yaml_str(&content))
                        .with_context(|| {
                            format!(
                                "Failed to parse OpenAPI file: {:?}. Tried both JSON and YAML formats.",
                                openapi_path
                            )
                        })
                }
            }
        }
    }

    /// Parses an OpenAPI string (JSON or YAML)
    /// Automatically detects format by content
    pub fn parse_str(content: &str) -> Result<OpenAPISchema> {
        let trimmed = content.trim_start();

        // Определить формат по содержимому
        if trimmed.starts_with('{') {
            // JSON начинается с {
            Self::parse_json_str(content)
        } else if trimmed.starts_with("---")
            || trimmed.starts_with("openapi:")
            || trimmed.starts_with("swagger:")
        {
            // YAML может начинаться с --- или с openapi:/swagger:
            Self::parse_yaml_str(content)
        } else {
            // По умолчанию попробовать JSON, затем YAML
            Self::parse_json_str(content)
                .or_else(|_| Self::parse_yaml_str(content))
                .context("Failed to parse OpenAPI string. Tried both JSON and YAML formats.")
        }
    }

    /// Parses an OpenAPI JSON string
    fn parse_json_str(content: &str) -> Result<OpenAPISchema> {
        let schema: OpenAPISchema =
            serde_json::from_str(content).context("Failed to parse OpenAPI JSON")?;

        Ok(schema)
    }

    /// Parses an OpenAPI YAML string
    fn parse_yaml_str(content: &str) -> Result<OpenAPISchema> {
        let schema: OpenAPISchema =
            serde_yaml::from_str(content).context("Failed to parse OpenAPI YAML")?;

        Ok(schema)
    }

    /// Extracts all endpoints from an OpenAPI schema
    pub fn extract_endpoints(schema: &OpenAPISchema) -> Vec<OpenAPIEndpoint> {
        let mut endpoints = Vec::new();

        for (path, path_item) in &schema.paths {
            for (method, operation) in &path_item.operations {
                let request_schema = Self::extract_request_schema(operation);
                let (response_schema, response_code) = Self::extract_response_schema(operation);

                endpoints.push(OpenAPIEndpoint {
                    path: path.clone(),
                    method: method.clone(),
                    operation_id: operation.operation_id.clone(),
                    request_schema,
                    response_schema,
                    response_code,
                });
            }
        }

        endpoints
    }

    /// Extracts request schema name from an operation
    fn extract_request_schema(operation: &Operation) -> Option<String> {
        operation.request_body.as_ref().and_then(|body| {
            body.content
                .values()
                .next()
                .and_then(|media| media.schema.as_ref())
                .and_then(Self::extract_schema_name)
        })
    }

    /// Extracts response schema name from an operation
    /// Returns (schema_name, response_code)
    fn extract_response_schema(operation: &Operation) -> (Option<String>, Option<String>) {
        // Prefer 200, then 201, then first success code (2xx)
        let mut success_codes: Vec<_> = operation
            .responses
            .iter()
            .filter(|(code, _)| code.starts_with('2'))
            .collect();

        success_codes.sort_by_key(|(code, _)| {
            if *code == "200" {
                0
            } else if *code == "201" {
                1
            } else {
                2
            }
        });

        if let Some((code, response)) = success_codes.first() {
            if let Some(content) = &response.content {
                if let Some(media) = content.values().next() {
                    if let Some(schema_ref) = &media.schema {
                        if let Some(schema_name) = Self::extract_schema_name(schema_ref) {
                            return (Some(schema_name), Some((*code).clone()));
                        }
                    }
                }
            }
        }

        (None, None)
    }

    /// Extracts schema name from a schema reference
    /// Handles both $ref and inline schemas
    fn extract_schema_name(schema_ref: &SchemaRef) -> Option<String> {
        match schema_ref {
            SchemaRef::Ref(reference) => {
                // Extract name from #/components/schemas/ItemRead
                if reference.ref_path.starts_with("#/components/schemas/") {
                    Some(
                        reference
                            .ref_path
                            .trim_start_matches("#/components/schemas/")
                            .to_string(),
                    )
                } else {
                    None
                }
            }
            SchemaRef::Inline(schema) => {
                // For inline schemas, try to extract title or use a generic name
                match schema.as_ref() {
                    Schema::Object(obj) => obj.title.clone(),
                    Schema::Array(arr) => arr.title.clone(),
                    Schema::Primitive(prim) => prim.title.clone(),
                    Schema::AllOf(all) => all.title.clone(),
                    Schema::OneOf(one) => one.title.clone(),
                    Schema::AnyOf(any) => any.title.clone(),
                }
            }
        }
    }

    /// Extracts all schema components from an OpenAPI schema
    /// Resolves $ref references to include referenced schemas
    pub fn extract_schemas(schema: &OpenAPISchema) -> HashMap<String, OpenAPISchemaComponent> {
        let mut schemas = HashMap::new();

        if let Some(components) = &schema.components {
            if let Some(schema_map) = &components.schemas {
                for (name, schema_ref) in schema_map {
                    // Create a fresh visited HashSet for each schema to avoid cross-schema pollution
                    let mut visited = std::collections::HashSet::new();

                    let properties =
                        Self::extract_schema_properties(schema_ref, schema, &mut visited);

                    let resolved_schema = match schema_ref {
                        SchemaRef::Ref(reference) => {
                            // Resolve the reference using the same visited set
                            if let Some(resolved) =
                                Self::resolve_ref(schema, &reference.ref_path, &mut visited)
                            {
                                resolved.schema
                            } else {
                                // Log warning instead of silently skipping
                                tracing::warn!(
                                    schema_name = %name,
                                    ref_path = %reference.ref_path,
                                    "Failed to resolve schema reference, skipping schema"
                                );
                                continue;
                            }
                        }
                        SchemaRef::Inline(s) => (**s).clone(),
                    };

                    schemas.insert(
                        name.clone(),
                        OpenAPISchemaComponent {
                            name: name.clone(),
                            schema: resolved_schema,
                            properties,
                        },
                    );
                }
            }
        }

        schemas
    }

    /// Extracts properties from a schema, resolving $ref references recursively
    fn extract_schema_properties(
        schema_ref: &SchemaRef,
        schema: &OpenAPISchema,
        visited: &mut std::collections::HashSet<String>,
    ) -> Vec<(String, String)> {
        let mut properties = Vec::new();

        match schema_ref {
            SchemaRef::Ref(reference) => {
                // Resolve the reference
                let ref_path = &reference.ref_path;

                // Check for circular references
                if visited.contains(ref_path) {
                    // Circular reference detected, skip to avoid infinite loop
                    return properties;
                }

                visited.insert(ref_path.clone());

                // Resolve the reference
                let mut resolve_visited = visited.clone();
                if let Some(resolved_component) =
                    Self::resolve_ref(schema, ref_path, &mut resolve_visited)
                {
                    // Recursively extract properties from resolved schema
                    let resolved_schema_ref =
                        SchemaRef::Inline(Box::new(resolved_component.schema));
                    properties.extend(Self::extract_schema_properties(
                        &resolved_schema_ref,
                        schema,
                        visited,
                    ));
                }

                visited.remove(ref_path);
            }
            SchemaRef::Inline(inline_schema) => {
                match inline_schema.as_ref() {
                    Schema::Object(obj) => {
                        if let Some(props) = &obj.properties {
                            for (prop_name, prop_schema) in props {
                                let prop_type = Self::extract_schema_name(prop_schema)
                                    .unwrap_or_else(|| "unknown".to_string());
                                properties.push((prop_name.clone(), prop_type));

                                // If property is a reference, we don't need to extract nested properties
                                // as we're only tracking property names and types, not full nested structures
                            }
                        }
                    }
                    Schema::Array(arr) => {
                        if let Some(items) = &arr.items {
                            let item_type = Self::extract_schema_name(items)
                                .unwrap_or_else(|| "array".to_string());
                            properties.push(("items".to_string(), item_type));
                        }
                    }
                    Schema::AllOf(all) => {
                        // Extract properties from all schemas in allOf
                        for schema_ref in &all.all_of {
                            properties.extend(Self::extract_schema_properties(
                                schema_ref, schema, visited,
                            ));
                        }
                    }
                    Schema::OneOf(_) | Schema::AnyOf(_) | Schema::Primitive(_) => {
                        // These don't have direct properties
                    }
                }
            }
        }

        properties
    }

    /// Finds an endpoint by operation ID
    pub fn find_endpoint_by_operation_id(
        schema: &OpenAPISchema,
        operation_id: &str,
    ) -> Option<OpenAPIEndpoint> {
        Self::extract_endpoints(schema)
            .into_iter()
            .find(|endpoint| {
                endpoint
                    .operation_id
                    .as_ref()
                    .map(|id| id == operation_id)
                    .unwrap_or(false)
            })
    }

    /// Finds an endpoint by path and method
    pub fn find_endpoint_by_path_method(
        schema: &OpenAPISchema,
        path: &str,
        method: &str,
    ) -> Option<OpenAPIEndpoint> {
        Self::extract_endpoints(schema)
            .into_iter()
            .find(|endpoint| {
                endpoint.path == path && endpoint.method.to_lowercase() == method.to_lowercase()
            })
    }

    /// Gets a schema component by name directly from components
    /// This method works directly with the schema components without resolving references
    fn get_schema_component_direct<'a>(
        schema: &'a OpenAPISchema,
        schema_name: &str,
    ) -> Option<&'a SchemaRef> {
        schema
            .components
            .as_ref()?
            .schemas
            .as_ref()?
            .get(schema_name)
    }

    /// Gets a schema component by name
    pub fn get_schema_component(
        schema: &OpenAPISchema,
        schema_name: &str,
    ) -> Option<OpenAPISchemaComponent> {
        // First try to get the schema directly
        if let Some(schema_ref) = Self::get_schema_component_direct(schema, schema_name) {
            let mut visited = std::collections::HashSet::new();
            let properties = Self::extract_schema_properties(schema_ref, schema, &mut visited);

            let mut resolve_visited = std::collections::HashSet::new();
            let resolved_schema = match schema_ref {
                SchemaRef::Ref(reference) => {
                    // Resolve the reference recursively
                    if let Some(resolved) =
                        Self::resolve_ref(schema, &reference.ref_path, &mut resolve_visited)
                    {
                        resolved.schema
                    } else {
                        // If resolution fails, return None
                        return None;
                    }
                }
                SchemaRef::Inline(s) => (**s).clone(),
            };

            return Some(OpenAPISchemaComponent {
                name: schema_name.to_string(),
                schema: resolved_schema,
                properties,
            });
        }

        None
    }

    /// Resolves a $ref reference to an OpenAPISchemaComponent
    /// Supports internal references: #/components/schemas/Name
    /// Returns None for external references (not yet supported)
    fn resolve_ref(
        schema: &OpenAPISchema,
        ref_path: &str,
        visited: &mut std::collections::HashSet<String>,
    ) -> Option<OpenAPISchemaComponent> {
        // Check for circular references
        if visited.contains(ref_path) {
            // Circular reference detected, return None to avoid infinite loop
            return None;
        }

        // Handle internal references to components/schemas
        if ref_path.starts_with("#/components/schemas/") {
            let schema_name = ref_path.trim_start_matches("#/components/schemas/");

            visited.insert(ref_path.to_string());

            // Get the schema reference directly
            if let Some(schema_ref) = Self::get_schema_component_direct(schema, schema_name) {
                let mut props_visited = visited.clone();
                let properties =
                    Self::extract_schema_properties(schema_ref, schema, &mut props_visited);

                let resolved_schema = match schema_ref {
                    SchemaRef::Ref(reference) => {
                        // Recursively resolve nested references
                        if let Some(resolved) =
                            Self::resolve_ref(schema, &reference.ref_path, visited)
                        {
                            resolved.schema
                        } else {
                            visited.remove(ref_path);
                            return None;
                        }
                    }
                    SchemaRef::Inline(s) => (**s).clone(),
                };

                visited.remove(ref_path);

                return Some(OpenAPISchemaComponent {
                    name: schema_name.to_string(),
                    schema: resolved_schema,
                    properties,
                });
            }

            visited.remove(ref_path);
        }

        // TODO: Handle #/components/parameters/... (future enhancement)
        // TODO: Handle #/components/responses/... (future enhancement)
        // TODO: Handle external references (http://, file://) (future enhancement)

        None
    }
}
