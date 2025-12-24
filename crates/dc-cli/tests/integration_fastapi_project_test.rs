use std::fs;
use std::path::PathBuf;

use anyhow::Result;

use dc_cli::commands::check::execute_check;
use dc_cli::ReportFormat;

#[test]
fn basic_fastapi_project_generates_expected_report() -> Result<()> {
    let project_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/integration/projects/basic-fastapi");
    assert!(
        project_root.exists(),
        "basic-fastapi project root not found at {:?}",
        project_root
    );

    let config_path = project_root.join("dc-verifier.toml");
    assert!(
        config_path.exists(),
        "dc-verifier config not found at {:?}",
        config_path
    );

    let report_path = project_root.join(".chain_verification_report.md");
    if report_path.exists() {
        fs::remove_file(&report_path)?;
    }

    execute_check(
        config_path
            .to_str()
            .expect("config path should be valid UTF-8"),
        ReportFormat::Markdown,
        false,
    )?;

    let report = fs::read_to_string(&report_path)?;

    // Basic sanity checks: report was successfully generated and contains expected chains.
    assert!(
        report.contains("Total Chains"),
        "report should contain verification statistics with total chains"
    );
    assert!(
        report.contains("POST /items/"),
        "report should describe POST /items/ chain"
    );
    assert!(
        report.contains("GET /items/"),
        "report should describe GET /items/ chain"
    );
    assert!(
        report.contains("GET /items/{item_id}"),
        "report should describe GET /items with id path parameter chain"
    );

    Ok(())
}
