use anyhow::Result;
use dc_core::models::DataChain;
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
            chrono::Utc::now().format("%Y-%m-%d")
        ));

        // Statistics - count chains, not contracts
        let total_chains = chains.len();
        let chains_with_critical = chains
            .iter()
            .filter(|chain| {
                chain
                    .contracts
                    .iter()
                    .any(|c| c.severity == dc_core::models::Severity::Critical)
            })
            .count();
        let chains_with_warnings = chains
            .iter()
            .filter(|chain| {
                // Chains without Critical, but with at least one Warning
                !chain
                    .contracts
                    .iter()
                    .any(|c| c.severity == dc_core::models::Severity::Critical)
                    && chain
                        .contracts
                        .iter()
                        .any(|c| c.severity == dc_core::models::Severity::Warning)
            })
            .count();
        let valid_chains = total_chains - chains_with_critical - chains_with_warnings;

        report.push_str("## Verification Statistics\n");
        report.push_str(&format!("- **Total Chains**: {}\n", total_chains));
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
            report.push_str(&format!("#### ID: {}\n\n", chain.id));

            // Data path
            report.push_str("#### Data Path:\n```\n");
            for (idx, link) in chain.links.iter().enumerate() {
                if idx > 0 {
                    report.push_str(" → ");
                }
                report.push_str(&link.id);
            }
            report.push_str("\n```\n\n");

            // Checked junctions
            report.push_str("#### Checked Junctions:\n\n");
            for (i, contract) in chain.contracts.iter().enumerate() {
                if contract.mismatches.is_empty() {
                    report.push_str(&format!(
                        "{}. ✅ **{} → {}**\n",
                        i + 1,
                        contract.from_link_id,
                        contract.to_link_id
                    ));
                    report.push_str("   - ✅ **Correct**: all fields match\n\n");
                } else {
                    report.push_str(&format!(
                        "{}. ⚠️ **{} → {}**\n",
                        i + 1,
                        contract.from_link_id,
                        contract.to_link_id
                    ));
                    for mismatch in &contract.mismatches {
                        report.push_str(&format!(
                            "   - ⚠️ **{:?}**: {}\n",
                            mismatch.mismatch_type, mismatch.message
                        ));
                    }
                    report.push_str("\n");
                }
            }

            // Result
            let has_errors = chain.contracts.iter().any(|c| !c.mismatches.is_empty());
            if has_errors {
                report.push_str("#### Result: ⚠️ **REQUIRES ATTENTION**\n\n");
            } else {
                report.push_str("#### Result: ✅ **CORRECT**\n\n");
            }

            report.push_str("---\n\n");
        }

        // Final conclusions
        report.push_str("## Final Conclusions\n\n");
        if chains_with_critical == 0 && chains_with_warnings == 0 {
            report.push_str("### ✅ Overall Assessment: **CORRECT**\n\n");
        } else {
            report.push_str("### ⚠️ Overall Assessment: **REQUIRES ATTENTION**\n\n");
        }

        fs::write(Path::new(output_path), report)?;
        Ok(())
    }
}
