use anyhow::Result;
use dc_core::models::{ChainType, DataChain, SchemaReference, SchemaType, Severity};
use std::collections::HashMap;
use std::fs;
use std::path::Path;

/// JSON report generator
pub struct JsonReporter;

impl JsonReporter {
    /// Generates a JSON report
    pub fn generate(&self, chains: &[DataChain], output_path: &str) -> Result<()> {
        let summary = Self::build_summary(chains);

        let report = serde_json::json!({
            "version": "1.0.0",
            "timestamp": chrono::Utc::now().to_rfc3339(),
            "summary": summary,
            "chains": chains,
        });

        let json_string = serde_json::to_string_pretty(&report)?;
        fs::write(Path::new(output_path), json_string)?;
        Ok(())
    }

    fn build_summary(chains: &[DataChain]) -> serde_json::Value {
        let total_chains = chains.len();

        let critical_issues = chains
            .iter()
            .flat_map(|c| &c.contracts)
            .filter(|c| c.severity == Severity::Critical)
            .count();

        let warnings = chains
            .iter()
            .flat_map(|c| &c.contracts)
            .filter(|c| c.severity == Severity::Warning)
            .count();

        // Chains by type
        let mut chains_by_type: HashMap<String, usize> = HashMap::new();
        for chain in chains {
            let key = match chain.chain_type {
                ChainType::Full => "full",
                ChainType::FrontendInternal => "frontend_internal",
                ChainType::BackendInternal => "backend_internal",
            };
            *chains_by_type.entry(key.to_string()).or_insert(0) += 1;
        }

        // Schemas by type
        let all_schemas = Self::collect_all_schemas(chains);
        let mut schemas_by_type: HashMap<String, usize> = HashMap::new();
        for schema in &all_schemas {
            let key = match schema.schema_type {
                SchemaType::Pydantic => "pydantic",
                SchemaType::Zod => "zod",
                SchemaType::TypeScript => "typescript",
                SchemaType::OpenAPI => "openapi",
                SchemaType::JsonSchema => "json_schema",
                SchemaType::OrmModel => "orm_model",
            };
            *schemas_by_type.entry(key.to_string()).or_insert(0) += 1;
        }

        // Simple coverage metrics (best-effort, без ожиданий из конфига)
        let schemas_found = all_schemas.len();

        serde_json::json!({
            "total_chains": total_chains,
            "critical_issues": critical_issues,
            "warnings": warnings,
            "chains_by_type": chains_by_type,
            "schemas": {
                "total": schemas_found,
                "by_type": schemas_by_type,
            }
        })
    }

    /// Collects all unique schemas from chains
    fn collect_all_schemas(chains: &[DataChain]) -> Vec<&SchemaReference> {
        let mut seen = std::collections::HashSet::new();
        let mut result = Vec::new();

        for chain in chains {
            for link in &chain.links {
                let key = format!(
                    "{}:{}:{}",
                    link.schema_ref.name,
                    link.schema_ref.location.file,
                    link.schema_ref.location.line
                );
                if !seen.contains(&key) {
                    seen.insert(key);
                    result.push(&link.schema_ref);
                }
            }
        }

        result
    }
}
