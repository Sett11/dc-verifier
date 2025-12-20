use crate::call_graph::HttpMethod;
use crate::models::{SchemaReference, TypeInfo};
use crate::openapi::schema::{OpenAPIEndpoint, OpenAPISchema, OpenAPISchemaComponent};
use crate::openapi::OpenAPIParser;
use std::collections::HashMap;

/// Links OpenAPI schemas with code artifacts (routes, types, models)
pub struct OpenAPILinker {
    #[allow(dead_code)]
    schema: OpenAPISchema,
    endpoints: Vec<OpenAPIEndpoint>,
    schemas: HashMap<String, OpenAPISchemaComponent>,
    /// Index: path -> method -> endpoint
    endpoint_index: HashMap<String, HashMap<String, usize>>,
    /// Index: operation_id -> endpoint index
    operation_id_index: HashMap<String, usize>,
}

impl OpenAPILinker {
    /// Creates a new linker from OpenAPI schema
    pub fn new(schema: OpenAPISchema) -> Self {
        let endpoints = OpenAPIParser::extract_endpoints(&schema);
        let schemas = OpenAPIParser::extract_schemas(&schema);

        // Build indexes for fast lookup
        let mut endpoint_index: HashMap<String, HashMap<String, usize>> = HashMap::new();
        let mut operation_id_index: HashMap<String, usize> = HashMap::new();

        for (idx, endpoint) in endpoints.iter().enumerate() {
            // Index by path and method
            endpoint_index
                .entry(endpoint.path.clone())
                .or_insert_with(HashMap::new)
                .insert(endpoint.method.to_lowercase(), idx);

            // Index by operation_id if present
            if let Some(ref op_id) = endpoint.operation_id {
                operation_id_index.insert(op_id.clone(), idx);
            }
        }

        Self {
            schema,
            endpoints,
            schemas,
            endpoint_index,
            operation_id_index,
        }
    }

    /// Matches a route to an OpenAPI endpoint by path and method
    pub fn match_route_to_endpoint(
        &self,
        path: &str,
        method: HttpMethod,
    ) -> Option<&OpenAPIEndpoint> {
        let method_str = match method {
            HttpMethod::Get => "get",
            HttpMethod::Post => "post",
            HttpMethod::Put => "put",
            HttpMethod::Patch => "patch",
            HttpMethod::Delete => "delete",
            HttpMethod::Options => "options",
            HttpMethod::Head => "head",
        };

        self.endpoint_index
            .get(path)?
            .get(method_str)
            .and_then(|&idx| self.endpoints.get(idx))
    }

    /// Gets a schema component by name
    pub fn get_schema(&self, schema_name: &str) -> Option<&OpenAPISchemaComponent> {
        self.schemas.get(schema_name)
    }

    /// Links a Pydantic model name to an OpenAPI schema
    /// Tries exact match first, then case-insensitive, then partial match
    pub fn link_pydantic_to_openapi(&self, pydantic_name: &str) -> Option<&OpenAPISchemaComponent> {
        // Try exact match
        if let Some(schema) = self.schemas.get(pydantic_name) {
            return Some(schema);
        }

        // Try case-insensitive match
        for (name, schema) in &self.schemas {
            if name.eq_ignore_ascii_case(pydantic_name) {
                return Some(schema);
            }
        }

        // Try partial match (e.g., "ItemRead" matches "ItemRead" or "ItemReadSchema")
        for (name, schema) in &self.schemas {
            if name.contains(pydantic_name) || pydantic_name.contains(name) {
                return Some(schema);
            }
        }

        None
    }

    /// Links a TypeScript type name to an OpenAPI schema
    /// Similar to link_pydantic_to_openapi but handles TypeScript naming conventions
    pub fn link_typescript_to_openapi(
        &self,
        typescript_name: &str,
    ) -> Option<&OpenAPISchemaComponent> {
        // Try exact match
        if let Some(schema) = self.schemas.get(typescript_name) {
            return Some(schema);
        }

        // Try case-insensitive match
        for (name, schema) in &self.schemas {
            if name.eq_ignore_ascii_case(typescript_name) {
                return Some(schema);
            }
        }

        // Try removing common TypeScript suffixes/prefixes
        let normalized = typescript_name
            .trim_start_matches("I")
            .trim_end_matches("Type")
            .trim_end_matches("Interface");

        for (name, schema) in &self.schemas {
            if name.eq_ignore_ascii_case(normalized) {
                return Some(schema);
            }
        }

        // Try partial match
        for (name, schema) in self.schemas.iter() {
            if name.contains(typescript_name) || typescript_name.contains(name.as_str()) {
                return Some(schema);
            }
        }

        None
    }

    /// Finds an endpoint by operation ID
    pub fn find_endpoint_by_operation_id(&self, operation_id: &str) -> Option<&OpenAPIEndpoint> {
        self.operation_id_index
            .get(operation_id)
            .and_then(|&idx| self.endpoints.get(idx))
    }

    /// Gets all endpoints
    pub fn get_all_endpoints(&self) -> &[OpenAPIEndpoint] {
        &self.endpoints
    }

    /// Gets all schema names
    pub fn get_schema_names(&self) -> Vec<String> {
        self.schemas.keys().cloned().collect()
    }

    /// Validates routes against OpenAPI endpoints
    /// Returns (missing_in_openapi, missing_in_code)
    pub fn validate_routes(
        &self,
        discovered_routes: &[(String, HttpMethod)],
    ) -> (Vec<(String, HttpMethod)>, Vec<&OpenAPIEndpoint>) {
        let mut missing_in_openapi = Vec::new();
        let mut missing_in_code: Vec<&OpenAPIEndpoint> = Vec::new();

        // Check discovered routes against OpenAPI
        for (path, method) in discovered_routes {
            if self.match_route_to_endpoint(path, *method).is_none() {
                missing_in_openapi.push((path.clone(), *method));
            }
        }

        // Check OpenAPI endpoints against discovered routes
        let discovered_set: std::collections::HashSet<_> = discovered_routes
            .iter()
            .map(|(p, m)| (p.as_str(), *m))
            .collect();

        for endpoint in &self.endpoints {
            let method = match endpoint.method.to_lowercase().as_str() {
                "get" => HttpMethod::Get,
                "post" => HttpMethod::Post,
                "put" => HttpMethod::Put,
                "patch" => HttpMethod::Patch,
                "delete" => HttpMethod::Delete,
                "options" => HttpMethod::Options,
                "head" => HttpMethod::Head,
                _ => continue,
            };

            if !discovered_set.contains(&(endpoint.path.as_str(), method)) {
                missing_in_code.push(endpoint);
            }
        }

        (missing_in_openapi, missing_in_code)
    }

    /// Creates schema links between Frontend TypeScript types and Backend Pydantic models
    /// Returns a vector of schema links
    pub fn create_schema_links(
        &self,
        frontend_types: &HashMap<String, TypeInfo>,
        backend_models: &HashMap<String, SchemaReference>,
    ) -> Vec<SchemaLink> {
        let mut links = Vec::new();

        // For each OpenAPI schema, try to find corresponding Frontend and Backend schemas
        for (schema_name, _schema_component) in &self.schemas {
            // Try to find corresponding TypeScript type
            let typescript_type = frontend_types
                .iter()
                .find(|(name, _)| {
                    name.eq_ignore_ascii_case(schema_name)
                        || name.contains(schema_name)
                        || schema_name.contains(name.as_str())
                })
                .map(|(name, type_info)| (name.clone(), type_info.clone()));

            // Try to find corresponding Pydantic model
            let pydantic_model = backend_models
                .iter()
                .find(|(name, _)| {
                    name.eq_ignore_ascii_case(schema_name)
                        || name.contains(schema_name)
                        || schema_name.contains(name.as_str())
                })
                .map(|(name, schema_ref)| (name.clone(), schema_ref.clone()));

            if typescript_type.is_some() || pydantic_model.is_some() {
                links.push(SchemaLink {
                    openapi_schema: schema_name.clone(),
                    typescript_type,
                    pydantic_model,
                });
            }
        }

        links
    }
}

/// Represents a link between TypeScript type, OpenAPI schema, and Pydantic model
#[derive(Debug, Clone)]
pub struct SchemaLink {
    pub openapi_schema: String,
    pub typescript_type: Option<(String, TypeInfo)>,
    pub pydantic_model: Option<(String, SchemaReference)>,
}
