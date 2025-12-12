use anyhow::Result;
use dc_core::models::{DataChain, LinkType, SchemaType, Severity, MismatchType};
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

        report.push_str("## Verification Statistics\n\n");
        report.push_str(&format!("- **Total Chains**: {}\n", total_chains));
        report.push_str(&format!("- **API Endpoints**: {}\n", route_count));
        report.push_str(&format!(
            "- **Critical Issues**: {}\n",
            chains_with_critical
        ));
        report.push_str(&format!("- **Warnings**: {}\n", chains_with_warnings));
        report.push_str(&format!("- **Valid Chains**: {}\n\n", valid_chains));
        report.push_str("---\n\n");

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
            report.push_str("✅ **No issues detected. All data chains are correctly configured.**\n\n");
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
        format!("{}", schema_name)
    }

    /// Formats schema information for display
    fn format_schema_info(schema: &dc_core::models::SchemaReference) -> String {
        let mut info = format!(
            "- **Name**: `{}`\n",
            schema.name
        );
        info.push_str(&format!(
            "- **Type**: {}\n",
            Self::format_schema_type(&schema.schema_type)
        ));
        info.push_str(&format!(
            "- **Location**: `{}:{}`\n",
            schema.location.file, schema.location.line
        ));
        if !schema.metadata.is_empty() {
            info.push_str("- **Metadata**:\n");
            for (key, value) in &schema.metadata {
                info.push_str(&format!("  - `{}`: `{}`\n", key, value));
            }
        }
        info
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
            result.push_str("?");
        }
        if let Some(ref schema_ref) = type_info.schema_ref {
            result.push_str(&format!(" ({})", schema_ref.name));
        }
        result
    }
}
