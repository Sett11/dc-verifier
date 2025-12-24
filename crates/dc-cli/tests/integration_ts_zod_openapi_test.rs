use std::fs;
use std::path::PathBuf;

use anyhow::Result;
use serde_json::Value;

use dc_cli::commands::check::execute_check;
use dc_cli::ReportFormat;

fn basic_fastapi_project_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/integration/projects/basic-fastapi")
}

#[test]
fn ts_zod_openapi_chain_analysis_produces_json_summary() -> Result<()> {
    let project_root = basic_fastapi_project_root();
    assert!(
        project_root.exists(),
        "basic-fastapi project root not found at {:?}",
        project_root
    );

    // Local copy of config with JSON output for this test.
    let config_path = project_root.join("dc-verifier.ts-zod-openapi.toml");
    // Always rewrite to avoid depending on source file format and line breaks.
    let config_contents = r#"project_name = "Basic FastAPI TS/Zod/OpenAPI Example"
entry_point = "backend/main.py"
openapi_path = "openapi.json"

[[adapters]]
type = "fastapi"
app_path = "backend/main.py"
openapi_path = "openapi.json"

[[adapters]]
type = "typescript"
src_paths = ["frontend/src"]
openapi_path = "openapi.json"

[output]
format = "json"
path = "report_ts_zod_openapi.json"
"#;
    fs::write(&config_path, config_contents)?;

    let report_path = project_root.join("report_ts_zod_openapi.json");
    if report_path.exists() {
        fs::remove_file(&report_path)?;
    }

    execute_check(
        config_path
            .to_str()
            .expect("config path should be valid UTF-8"),
        ReportFormat::Json,
        false,
    )?;

    let content = fs::read_to_string(&report_path)?;
    let json: Value = serde_json::from_str(&content)?;

    // Verify that summary.schemas.by_type is present and has expected structure.
    let summary = json
        .get("summary")
        .expect("summary section must be present in JSON report");
    let schemas = summary
        .get("schemas")
        .expect("schemas section must be present in JSON summary");
    let by_type = schemas
        .get("by_type")
        .expect("schemas.by_type must be present in JSON summary");

    // For now, doing a soft check: keys exist and it's not an empty object.
    assert!(
        by_type.as_object().map(|m| !m.is_empty()).unwrap_or(false),
        "schemas.by_type should contain at least one schema type entry"
    );

    // Cleanup test artifacts
    if config_path.exists() {
        fs::remove_file(&config_path)?;
    }
    if report_path.exists() {
        fs::remove_file(&report_path)?;
    }

    Ok(())
}
