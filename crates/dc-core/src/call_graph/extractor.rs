use crate::models::SchemaReference;
use anyhow::Result;
use std::path::Path;

/// Trait for extracting JSON schemas from Pydantic models
pub trait PydanticSchemaExtractor: Send + Sync {
    /// Extracts JSON schema for a Pydantic model from a file
    ///
    /// Returns:
    /// - `Ok(Some(json_schema))` when a schema is successfully extracted
    /// - `Ok(None)` when the model is not found or extraction yields no schema
    /// - `Err(...)` for extraction failures (I/O errors, parsing errors, etc.)
    fn extract_json_schema(&self, model_name: &str, file_path: &Path) -> Result<Option<String>>;

    /// Enriches a SchemaReference with JSON schema if available
    fn enrich_schema(&self, schema: &mut SchemaReference) -> Result<()> {
        if let Some(json_schema) =
            self.extract_json_schema(&schema.name, Path::new(&schema.location.file))?
        {
            schema
                .metadata
                .insert("json_schema".to_string(), json_schema);
        }
        Ok(())
    }
}
