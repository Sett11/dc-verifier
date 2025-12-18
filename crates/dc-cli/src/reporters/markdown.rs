use anyhow::Result;
use dc_core::models::{ChainType, DataChain, LinkType, MismatchType, SchemaType, Severity};
use std::fs;
use std::path::Path;

/// Markdown report generator
pub struct MarkdownReporter;

impl MarkdownReporter {
    /// Generates report in .chain_verification_report.md format
    pub fn generate(&self, chains: &[DataChain], output_path: &str) -> Result<()> {
        let mut report = String::new();

        // Header
        report.push_str("# Data Chain Verification Report\n\n");
        report.push_str(&format!(
            "## Verification Date\n{}\n\n",
            chrono::Utc::now().format("%Y-%m-%d %H:%M:%S UTC")
        ));

        // Statistics - count chains, not contracts
        let total_chains = chains.len();
        let chains_with_critical = chains
            .iter()
            .filter(|chain| {
                chain
                    .contracts
                    .iter()
                    .any(|c| c.severity == Severity::Critical)
            })
            .count();
        let chains_with_warnings = chains
            .iter()
            .filter(|chain| {
                // Chains without Critical, but with at least one Warning
                !chain
                    .contracts
                    .iter()
                    .any(|c| c.severity == Severity::Critical)
                    && chain
                        .contracts
                        .iter()
                        .any(|c| c.severity == Severity::Warning)
            })
            .count();
        let valid_chains = total_chains - chains_with_critical - chains_with_warnings;

        // Count API endpoints (routes)
        let route_count = chains
            .iter()
            .filter(|chain| {
                chain.links.iter().any(|link| {
                    link.schema_ref.schema_type == SchemaType::Pydantic
                        || link.schema_ref.name.contains("route")
                })
            })
            .count();

        // Count chain types
        let full_chains = chains
            .iter()
            .filter(|c| c.chain_type == ChainType::Full)
            .count();
        let frontend_internal = chains
            .iter()
            .filter(|c| c.chain_type == ChainType::FrontendInternal)
            .count();
        let backend_internal = chains
            .iter()
            .filter(|c| c.chain_type == ChainType::BackendInternal)
            .count();

        report.push_str("## Verification Statistics\n\n");
        report.push_str(&format!("- **Total Chains**: {}\n", total_chains));
        report.push_str(&format!("  - Full Chains: {}\n", full_chains));
        report.push_str(&format!("  - Internal Frontend: {}\n", frontend_internal));
        report.push_str(&format!("  - Internal Backend: {}\n", backend_internal));
        report.push_str(&format!("- **API Endpoints**: {}\n", route_count));
        report.push_str(&format!(
            "- **Critical Issues**: {}\n",
            chains_with_critical
        ));
        report.push_str(&format!("- **Warnings**: {}\n", chains_with_warnings));
        report.push_str(&format!("- **Valid Chains**: {}\n\n", valid_chains));

        // Add schema summary section
        let schemas = Self::collect_all_schemas(chains);
        if !schemas.is_empty() {
            report.push_str("## Detected Schemas\n\n");
            report.push_str(&format!("Total schemas detected: {}\n\n", schemas.len()));

            // Group by schema type
            let mut by_type: std::collections::HashMap<
                SchemaType,
                Vec<&dc_core::models::SchemaReference>,
            > = std::collections::HashMap::new();
            for schema in &schemas {
                by_type.entry(schema.schema_type).or_default().push(schema);
            }

            for (schema_type, type_schemas) in &by_type {
                report.push_str(&format!(
                    "### {}\n\n",
                    Self::format_schema_type(schema_type)
                ));
                for schema in type_schemas {
                    report.push_str(&format!(
                        "- `{}` at `{}:{}`\n",
                        schema.name, schema.location.file, schema.location.line
                    ));
                }
                report.push('\n');
            }
            report.push_str("---\n\n");
        } else {
            report.push_str("---\n\n");
        }

        // Chain details
        for (idx, chain) in chains.iter().enumerate() {
            report.push_str(&format!("### Chain {}: {}\n\n", idx + 1, chain.name));
            report.push_str(&format!("#### ID: `{}`\n\n", chain.id));
            report.push_str(&format!(
                "#### Direction: {}\n\n",
                match chain.direction {
                    dc_core::models::ChainDirection::FrontendToBackend => {
                        "Frontend → Backend → Database"
                    }
                    dc_core::models::ChainDirection::BackendToFrontend => {
                        "Database → Backend → Frontend"
                    }
                }
            ));
            report.push_str(&format!(
                "#### Type: {}\n\n",
                match chain.chain_type {
                    ChainType::Full => "Full Chain (Frontend → Backend → Database)",
                    ChainType::FrontendInternal => "Internal Frontend Call (Frontend → Frontend)",
                    ChainType::BackendInternal => "Internal Backend Call (Backend → Backend)",
                }
            ));

            // Detailed data path description
            report.push_str("#### Data Path:\n\n");
            let path_description = Self::build_path_description(&chain.links);
            report.push_str(&path_description);
            report.push('\n');

            // Checked junctions with detailed analysis
            report.push_str("#### Checked Junctions:\n\n");
            for (i, contract) in chain.contracts.iter().enumerate() {
                let from_link = chain
                    .links
                    .iter()
                    .find(|l| l.id == contract.from_link_id)
                    .unwrap();
                let to_link = chain
                    .links
                    .iter()
                    .find(|l| l.id == contract.to_link_id)
                    .unwrap();

                report.push_str(&format!(
                    "##### Junction {}: {} → {}\n\n",
                    i + 1,
                    Self::format_link_name(from_link),
                    Self::format_link_name(to_link)
                ));

                // Schema information
                report.push_str("**Source Schema:**\n");
                report.push_str(&Self::format_schema_info(&contract.from_schema));
                report.push_str("\n\n**Target Schema:**\n");
                report.push_str(&Self::format_schema_info(&contract.to_schema));
                report.push('\n');

                if contract.mismatches.is_empty() {
                    report.push_str("**Status:** ✅ **CORRECT** - All fields match\n\n");
                } else {
                    report.push_str(&format!(
                        "**Status:** ⚠️ **ISSUES DETECTED** (Severity: {:?})\n\n",
                        contract.severity
                    ));
                    report.push_str("**Mismatches:**\n\n");
                    for mismatch in &contract.mismatches {
                        report.push_str(&format!(
                            "- **{:?}** at path `{}`\n",
                            mismatch.mismatch_type, mismatch.path
                        ));
                        report.push_str(&format!("  - Message: {}\n", mismatch.message));
                        report.push_str(&format!(
                            "  - Location: {}:{}\n",
                            mismatch.location.file, mismatch.location.line
                        ));
                        report.push('\n');
                    }
                }
                report.push_str("---\n\n");
            }

            // Chain result summary
            let has_critical = chain
                .contracts
                .iter()
                .any(|c| c.severity == Severity::Critical);
            let has_warnings = chain
                .contracts
                .iter()
                .any(|c| c.severity == Severity::Warning);
            if has_critical {
                report.push_str("#### Chain Result: 🔴 **CRITICAL ISSUES**\n\n");
            } else if has_warnings {
                report.push_str("#### Chain Result: 🟡 **WARNINGS**\n\n");
            } else {
                report.push_str("#### Chain Result: ✅ **CORRECT**\n\n");
            }

            report.push_str("---\n\n");
        }

        // Recommendations section
        report.push_str("## Recommendations\n\n");
        let recommendations = Self::generate_recommendations(chains);
        if recommendations.is_empty() {
            report.push_str(
                "✅ **No issues detected. All data chains are correctly configured.**\n\n",
            );
        } else {
            // Group by severity
            let critical_recs: Vec<_> = recommendations
                .iter()
                .filter(|r| r.0 == Severity::Critical)
                .collect();
            let warning_recs: Vec<_> = recommendations
                .iter()
                .filter(|r| r.0 == Severity::Warning)
                .collect();

            if !critical_recs.is_empty() {
                report.push_str("### 🔴 Critical Issues\n\n");
                for (_, rec) in critical_recs {
                    report.push_str(&format!("- {}\n", rec));
                }
                report.push('\n');
            }

            if !warning_recs.is_empty() {
                report.push_str("### 🟡 Warnings\n\n");
                for (_, rec) in warning_recs {
                    report.push_str(&format!("- {}\n", rec));
                }
                report.push('\n');
            }
        }

        // Final conclusions
        report.push_str("## Final Conclusions\n\n");
        if chains_with_critical == 0 && chains_with_warnings == 0 {
            report.push_str("### ✅ Overall Assessment: **CORRECT**\n\n");
            report.push_str("All data chains have been verified and no issues were detected. The data flow between frontend, backend, and database layers is consistent.\n\n");
        } else {
            report.push_str("### ⚠️ Overall Assessment: **REQUIRES ATTENTION**\n\n");
            if chains_with_critical > 0 {
                report.push_str(&format!(
                    "**{} chain(s)** have critical issues that must be addressed before deployment.\n\n",
                    chains_with_critical
                ));
            }
            if chains_with_warnings > 0 {
                report.push_str(&format!(
                    "**{} chain(s)** have warnings that should be reviewed.\n\n",
                    chains_with_warnings
                ));
            }
        }

        fs::write(Path::new(output_path), report)?;
        Ok(())
    }

    /// Builds a human-readable description of the data path
    fn build_path_description(links: &[dc_core::models::Link]) -> String {
        let mut parts = Vec::new();
        for link in links {
            let description = match link.link_type {
                LinkType::Source => {
                    format!(
                        "**{}** ({})",
                        Self::format_link_name(link),
                        Self::format_schema_type(&link.schema_ref.schema_type)
                    )
                }
                LinkType::Transformer => {
                    format!(
                        "**{}** ({})",
                        Self::format_link_name(link),
                        Self::format_schema_type(&link.schema_ref.schema_type)
                    )
                }
                LinkType::Sink => {
                    format!(
                        "**{}** ({})",
                        Self::format_link_name(link),
                        Self::format_schema_type(&link.schema_ref.schema_type)
                    )
                }
            };
            parts.push(description);
        }
        parts.join(" → ")
    }

    /// Formats a link name for display
    fn format_link_name(link: &dc_core::models::Link) -> String {
        let schema_name = if !link.schema_ref.name.is_empty() {
            &link.schema_ref.name
        } else {
            &link.id
        };
        schema_name.to_string()
    }

    /// Formats schema information for display
    fn format_schema_info(schema: &dc_core::models::SchemaReference) -> String {
        let mut info = format!("- **Name**: `{}`\n", schema.name);
        info.push_str(&format!(
            "- **Type**: {}\n",
            Self::format_schema_type(&schema.schema_type)
        ));
        info.push_str(&format!(
            "- **Location**: `{}:{}`\n",
            schema.location.file, schema.location.line
        ));

        // Check for missing schema
        if schema.metadata.contains_key("missing_schema") {
            let reason = schema
                .metadata
                .get("reason")
                .map(|r| r.as_str())
                .unwrap_or("Schema validation missing");

            info.push_str(&format!("- **Status**: ⚠️ **WARNING** - {}\n", reason));

            // Add recommendation for creating Pydantic schema
            if schema.name == "Object" {
                info.push_str(
                    "- **Recommendation**: Create a Pydantic model to replace `dict[str, Any]` or `any` type.\n",
                );
                info.push_str("  Example:\n");
                info.push_str("  ```python\n");
                info.push_str("  from pydantic import BaseModel\n");
                info.push_str("  \n");
                info.push_str(&format!(
                    "  class {}Model(BaseModel):\n",
                    schema
                        .location
                        .file
                        .rsplit('/')
                        .next()
                        .or_else(|| schema.location.file.rsplit('\\').next())
                        .unwrap_or("Your")
                        .rsplit('.')
                        .next()
                        .unwrap_or("Your")
                ));
                info.push_str("      # Add fields here\n");
                info.push_str("      pass\n");
                info.push_str("  ```\n");
            }
        } else if schema.metadata.is_empty() {
            info.push_str("- **Status**: Schema definition not found\n");
        } else {
            // Add JSON schema preview if available
            if let Some(json_schema) = schema.metadata.get("json_schema") {
                let preview = Self::truncate_json_schema(json_schema, 200);
                info.push_str(&format!("- **Schema Preview**: `{}`\n", preview));
            }

            // Add field information if available
            if let Some(fields_str) = schema.metadata.get("fields") {
                info.push_str("- **Fields**:\n");
                for field in fields_str.split(',') {
                    let field = field.trim();
                    if !field.is_empty() {
                        info.push_str(&format!("  - `{}`\n", field));
                    }
                }
            }

            // Add other metadata
            let mut other_metadata = Vec::new();
            for (key, value) in &schema.metadata {
                if key != "json_schema" && key != "fields" && key != "missing_schema" {
                    other_metadata.push((key, value));
                }
            }
            if !other_metadata.is_empty() {
                info.push_str("- **Additional Metadata**:\n");
                for (key, value) in other_metadata {
                    let display_value = Self::truncate_string_safe(value, 100);
                    info.push_str(&format!("  - `{}`: `{}`\n", key, display_value));
                }
            }
        }

        // For Object schemas show base_type from metadata and additional details
        if schema.name == "Object" || schema.schema_type == dc_core::models::SchemaType::JsonSchema
        {
            if let Some(base_type) = schema.metadata.get("base_type") {
                info.push_str(&format!("- **Base Type**: `{}`\n", base_type));
            }

            // Show additional context for Object schemas
            if schema.name == "Object" && !schema.metadata.contains_key("json_schema") {
                info.push_str(
                    "- **Note**: This is a generic Object type without detailed schema definition.\n",
                );
                if schema.location.line > 0 {
                    info.push_str(&format!(
                        "  Consider defining a Pydantic model at `{}:{}` to provide type safety.\n",
                        schema.location.file, schema.location.line
                    ));
                }
            }
        }

        info
    }

    /// Truncates a string safely at UTF-8 character boundaries
    fn truncate_string_safe(s: &str, max_len: usize) -> String {
        if s.len() <= max_len {
            return s.to_string();
        }

        // Find the last valid char boundary at or before max_len
        let mut truncate_at = max_len;
        while truncate_at > 0 && !s.is_char_boundary(truncate_at) {
            truncate_at -= 1;
        }

        // If we couldn't find a boundary, use the string as-is (shouldn't happen for valid UTF-8)
        if truncate_at == 0 {
            return s.to_string();
        }

        format!("{}...", &s[..truncate_at])
    }

    /// Truncates JSON schema string for preview
    fn truncate_json_schema(json_schema: &str, max_len: usize) -> String {
        Self::truncate_string_safe(json_schema, max_len)
    }

    /// Formats schema type for display
    fn format_schema_type(schema_type: &SchemaType) -> &'static str {
        match schema_type {
            SchemaType::Pydantic => "Pydantic Model",
            SchemaType::Zod => "Zod Schema",
            SchemaType::TypeScript => "TypeScript Type",
            SchemaType::OpenAPI => "OpenAPI Schema",
            SchemaType::JsonSchema => "JSON Schema",
        }
    }

    /// Generates recommendations based on mismatches
    fn generate_recommendations(chains: &[DataChain]) -> Vec<(Severity, String)> {
        let mut recommendations = Vec::new();
        let mut seen_recommendations = std::collections::HashSet::new();

        for chain in chains {
            for contract in &chain.contracts {
                for mismatch in &contract.mismatches {
                    let rec = match mismatch.mismatch_type {
                        MismatchType::TypeMismatch => {
                            format!(
                                "Fix type mismatch at `{}` in chain '{}': expected {}, got {}",
                                mismatch.path,
                                chain.name,
                                Self::format_type_info(&mismatch.expected),
                                Self::format_type_info(&mismatch.actual)
                            )
                        }
                        MismatchType::MissingField => {
                            format!(
                                "Add missing required field `{}` in chain '{}' at {}:{}",
                                mismatch.path,
                                chain.name,
                                mismatch.location.file,
                                mismatch.location.line
                            )
                        }
                        MismatchType::ExtraField => {
                            format!(
                                "Remove or make optional the extra field `{}` in chain '{}' at {}:{}",
                                mismatch.path,
                                chain.name,
                                mismatch.location.file,
                                mismatch.location.line
                            )
                        }
                        MismatchType::ValidationMismatch => {
                            format!(
                                "Align validation rules for field `{}` in chain '{}' at {}:{}",
                                mismatch.path,
                                chain.name,
                                mismatch.location.file,
                                mismatch.location.line
                            )
                        }
                        MismatchType::UnnormalizedData => {
                            format!(
                                "Normalize data format for field `{}` in chain '{}' at {}:{}",
                                mismatch.path,
                                chain.name,
                                mismatch.location.file,
                                mismatch.location.line
                            )
                        }
                        MismatchType::MissingSchema => {
                            format!(
                                "Add schema validation for `{}` in chain '{}' at {}:{} (currently using dict[str, Any] or any)",
                                mismatch.path,
                                chain.name,
                                mismatch.location.file,
                                mismatch.location.line
                            )
                        }
                    };

                    if seen_recommendations.insert(rec.clone()) {
                        recommendations.push((contract.severity, rec));
                    }
                }
            }
        }

        recommendations
    }

    /// Formats type information for display
    fn format_type_info(type_info: &dc_core::models::TypeInfo) -> String {
        let base = match type_info.base_type {
            dc_core::models::BaseType::String => "string",
            dc_core::models::BaseType::Number => "number",
            dc_core::models::BaseType::Integer => "integer",
            dc_core::models::BaseType::Boolean => "boolean",
            dc_core::models::BaseType::Object => "object",
            dc_core::models::BaseType::Array => "array",
            dc_core::models::BaseType::Null => "null",
            dc_core::models::BaseType::Any => "any",
            dc_core::models::BaseType::Unknown => "unknown",
        };
        let mut result = base.to_string();
        if type_info.optional {
            result.push('?');
        }
        if let Some(ref schema_ref) = type_info.schema_ref {
            result.push_str(&format!(" ({})", schema_ref.name));
        }
        result
    }

    /// Collects all unique schemas from chains
    fn collect_all_schemas(chains: &[DataChain]) -> Vec<&dc_core::models::SchemaReference> {
        let mut schemas = std::collections::HashSet::new();
        let mut result = Vec::new();

        for chain in chains {
            for link in &chain.links {
                // Use name and location as unique key
                let key = format!(
                    "{}:{}:{}",
                    link.schema_ref.name,
                    link.schema_ref.location.file,
                    link.schema_ref.location.line
                );
                if !schemas.contains(&key) {
                    schemas.insert(key);
                    result.push(&link.schema_ref);
                }
            }
        }

        result
    }
}
