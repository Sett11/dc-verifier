use crate::call_graph::HttpMethod;
use crate::models::{PydanticFieldInfo, SchemaReference, TypeInfo};
use crate::openapi::schema::{OpenAPIEndpoint, OpenAPISchema, OpenAPISchemaComponent};
use crate::openapi::OpenAPIParser;
use serde_json;
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
    /// Converts HttpMethod to lowercase string
    fn http_method_to_str(method: HttpMethod) -> &'static str {
        match method {
            HttpMethod::Get => "get",
            HttpMethod::Post => "post",
            HttpMethod::Put => "put",
            HttpMethod::Patch => "patch",
            HttpMethod::Delete => "delete",
            HttpMethod::Options => "options",
            HttpMethod::Head => "head",
        }
    }

    /// Converts string to HttpMethod
    fn str_to_http_method(s: &str) -> Option<HttpMethod> {
        match s.to_lowercase().as_str() {
            "get" => Some(HttpMethod::Get),
            "post" => Some(HttpMethod::Post),
            "put" => Some(HttpMethod::Put),
            "patch" => Some(HttpMethod::Patch),
            "delete" => Some(HttpMethod::Delete),
            "options" => Some(HttpMethod::Options),
            "head" => Some(HttpMethod::Head),
            _ => None,
        }
    }

    /// Checks if Pydantic type is compatible with OpenAPI type
    fn types_compatible_openapi(pydantic_type: &str, openapi_type: &str) -> bool {
        // Normalize types to lowercase for comparison
        let pydantic_normalized = pydantic_type.to_lowercase();
        let openapi_normalized = openapi_type.to_lowercase();

        // Direct type mappings
        match (pydantic_normalized.as_str(), openapi_normalized.as_str()) {
            // String types
            ("str", "string") | ("string", "str") => true,

            // Integer types
            ("int", "integer") | ("integer", "int") => true,

            // Boolean types
            ("bool", "boolean") | ("boolean", "bool") => true,

            // Float/Number types
            ("float", "number") | ("number", "float") => true,
            ("double", "number") | ("number", "double") => true,

            // Date/Time types
            (a, b) if a.contains("date") && b.contains("date") => true,
            (a, b) if a.contains("time") && b.contains("time") => true,
            (a, b) if a.contains("datetime") && b.contains("datetime") => true,

            // UUID types
            (a, b) if a.contains("uuid") && b.contains("uuid") => true,

            // Array types
            (a, b) if a.starts_with("list[") && b == "array" => true,
            (a, b) if a.starts_with("array") && b == "array" => true,

            // Object types
            (a, b) if a.starts_with("dict[") && b == "object" => true,
            (a, b) if a == "object" && b == "object" => true,

            // Exact match (case-insensitive)
            (a, b) if a == b => true,

            _ => false,
        }
    }

    /// Extracts Pydantic fields from a SchemaReference
    /// Returns empty vector if fields cannot be extracted
    fn extract_pydantic_fields(schema_ref: &SchemaReference) -> Vec<PydanticFieldInfo> {
        // Try to extract from metadata
        if let Some(fields_json) = schema_ref.metadata.get("fields") {
            // Try to parse as JSON (new format)
            if let Ok(fields) = serde_json::from_str::<Vec<PydanticFieldInfo>>(fields_json) {
                return fields;
            }
        }

        Vec::new()
    }

    /// Matches schemas by comparing field structures
    /// Returns a similarity score between 0.0 and 1.0
    /// 1.0 = perfect match, 0.0 = no match
    fn match_schemas_by_fields(
        &self,
        pydantic_fields: &[PydanticFieldInfo],
        openapi_schema: &OpenAPISchemaComponent,
    ) -> f64 {
        let openapi_props = &openapi_schema.properties;

        // If either is empty, return 0.0
        if pydantic_fields.is_empty() || openapi_props.is_empty() {
            return 0.0;
        }

        // Create a map of OpenAPI properties for fast lookup
        let openapi_map: HashMap<&str, &str> = openapi_props
            .iter()
            .map(|(name, type_name)| (name.as_str(), type_name.as_str()))
            .collect();

        let mut matches = 0;

        // Check each Pydantic field against OpenAPI properties
        for pydantic_field in pydantic_fields {
            if let Some(openapi_type) = openapi_map.get(pydantic_field.name.as_str()) {
                // Field name matches, check type compatibility
                if Self::types_compatible_openapi(&pydantic_field.type_name, openapi_type) {
                    matches += 1;
                }
            }
        }

        // Calculate similarity score
        // Use the maximum of both lengths to penalize missing fields
        let total = pydantic_fields.len().max(openapi_props.len());

        if total == 0 {
            return 0.0;
        }

        matches as f64 / total as f64
    }

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
                .or_default()
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
        let method_str = Self::http_method_to_str(method);

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
    /// Uses conservative matching: exact match, case-insensitive, then common variants,
    /// and finally field-based matching if name-based matching fails
    pub fn link_pydantic_to_openapi(
        &self,
        pydantic_name: &str,
        pydantic_schema: Option<&SchemaReference>,
    ) -> Option<&OpenAPISchemaComponent> {
        // Strategy 1: Try exact match
        if let Some(schema) = self.schemas.get(pydantic_name) {
            return Some(schema);
        }

        // Strategy 2: Try case-insensitive match
        for (name, schema) in &self.schemas {
            if name.eq_ignore_ascii_case(pydantic_name) {
                return Some(schema);
            }
        }

        // Strategy 3: Try common Pydantic variants with suffixes/prefixes
        let variants = [
            format!("{}Schema", pydantic_name),
            format!("{}Model", pydantic_name),
            format!("{}Request", pydantic_name),
            format!("{}Response", pydantic_name),
            format!("{}DTO", pydantic_name),
            pydantic_name.trim_end_matches("Schema").to_string(),
            pydantic_name.trim_end_matches("Model").to_string(),
            pydantic_name.trim_end_matches("Request").to_string(),
            pydantic_name.trim_end_matches("Response").to_string(),
            pydantic_name.trim_end_matches("DTO").to_string(),
        ];

        for variant in &variants {
            if let Some(schema) = self.schemas.get(variant) {
                return Some(schema);
            }
            // Also try case-insensitive for variants
            for (name, schema) in &self.schemas {
                if name.eq_ignore_ascii_case(variant) {
                    return Some(schema);
                }
            }
        }

        // Strategy 4: Field-based matching (NEW)
        // Only if we have Pydantic schema reference with fields
        if let Some(pydantic_schema_ref) = pydantic_schema {
            let pydantic_fields = Self::extract_pydantic_fields(pydantic_schema_ref);

            if !pydantic_fields.is_empty() {
                // Extract threshold to named constant
                const FIELD_MATCH_THRESHOLD: f64 = 0.7;
                
                let mut candidates: Vec<(&OpenAPISchemaComponent, f64)> = Vec::new();
                let threshold = FIELD_MATCH_THRESHOLD;

                // Try all OpenAPI schemas
                for openapi_schema in self.schemas.values() {
                    let score = self.match_schemas_by_fields(&pydantic_fields, openapi_schema);

                    if score >= threshold {
                        candidates.push((openapi_schema, score));
                    }
                }

                if !candidates.is_empty() {
                    // Find max score
                    let max_score = candidates.iter().map(|(_, score)| *score).fold(0.0, f64::max);
                    
                    // Collect all schemas with max score
                    let max_candidates: Vec<_> = candidates
                        .into_iter()
                        .filter(|(_, score)| (*score - max_score).abs() < f64::EPSILON)
                        .collect();
                    
                    if max_candidates.len() > 1 {
                        // Tie-breaker: prefer lexicographically closest name
                        let pydantic_name_lower = pydantic_name.to_lowercase();
                        if let Some((schema, _)) = max_candidates
                            .iter()
                            .min_by_key(|(schema, _)| {
                                // Calculate "distance" as lexicographic comparison
                                schema.name.to_lowercase().cmp(&pydantic_name_lower)
                            })
                        {
                            tracing::debug!(
                                pydantic_name = %pydantic_name,
                                chosen_schema = %schema.name,
                                score = max_score,
                                "Tie-breaker applied for schema matching"
                            );
                            return Some(*schema);
                        }
                        return None;
                    } else {
                        return max_candidates.first().map(|(schema, _)| *schema);
                    }
                }
            }
        }

        None
    }

    /// Links a TypeScript type name to an OpenAPI schema
    /// Uses conservative matching: exact match, case-insensitive, then TypeScript-specific variants
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

        // Try common TypeScript variants with suffixes/prefixes
        let normalized = typescript_name
            .trim_start_matches("I")
            .trim_end_matches("Type")
            .trim_end_matches("Interface");

        let variants = [
            format!("{}Props", normalized),
            format!("{}State", normalized),
            format!("{}Component", normalized),
            format!("{}Schema", normalized),
            format!("{}Model", normalized),
            format!("{}Request", normalized),
            format!("{}Response", normalized),
            typescript_name.trim_end_matches("Props").to_string(),
            typescript_name.trim_end_matches("State").to_string(),
            typescript_name.trim_end_matches("Component").to_string(),
            typescript_name.trim_end_matches("Type").to_string(),
            typescript_name.trim_end_matches("Interface").to_string(),
        ];

        for variant in &variants {
            if let Some(schema) = self.schemas.get(variant) {
                return Some(schema);
            }
            // Also try case-insensitive for variants
            for (name, schema) in &self.schemas {
                if name.eq_ignore_ascii_case(variant) {
                    return Some(schema);
                }
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
            let Some(method) = Self::str_to_http_method(&endpoint.method) else {
                continue;
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
        for schema_name in self.schemas.keys() {
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
