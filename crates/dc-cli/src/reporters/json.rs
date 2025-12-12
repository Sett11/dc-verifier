use anyhow::Result;
use dc_core::models::DataChain;
use std::fs;
use std::path::Path;

/// JSON report generator
pub struct JsonReporter;

impl JsonReporter {
    /// Generates a JSON report
    pub fn generate(&self, chains: &[DataChain], output_path: &str) -> Result<()> {
        let report = serde_json::json!({
            "version": "1.0.0",
            "timestamp": chrono::Utc::now().to_rfc3339(),
            "summary": {
                "total_chains": chains.len(),
                "critical_issues": chains.iter()
                    .flat_map(|c| &c.contracts)
                    .filter(|c| c.severity == dc_core::models::Severity::Critical)
                    .count(),
                "warnings": chains.iter()
                    .flat_map(|c| &c.contracts)
                    .filter(|c| c.severity == dc_core::models::Severity::Warning)
                    .count(),
            },
            "chains": chains,
        });

        let json_string = serde_json::to_string_pretty(&report)?;
        fs::write(Path::new(output_path), json_string)?;
        Ok(())
    }
}
