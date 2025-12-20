use anyhow::Result;
use std::fs;
use std::path::Path;

/// Creates the configuration file
pub fn execute_init(path: &str) -> Result<()> {
    let config_content = r#"project_name = "MyApp"
entry_point = "backend/api/main.py"

# Maximum recursion depth for graph building (optional, None = unlimited)
# max_recursion_depth = 100

# OpenAPI schema path (optional, can be overridden per adapter)
# openapi_path = "local-shared-data/openapi.json"

[[adapters]]
type = "fastapi"
app_path = "backend/api/main.py"
# openapi_path = "custom-openapi.json"  # Optional override

[[adapters]]
type = "typescript"
src_paths = ["frontend/src"]
# Uses global openapi_path or can override
# openapi_path = "frontend/openapi.json"

# Example NestJS adapter configuration:
# [[adapters]]
# type = "nestjs"
# src_paths = ["backend/src"]
# # openapi_path = "backend/openapi.json"  # Optional override

[rules]
type_mismatch = "critical"
missing_field = "warning"

[output]
format = "markdown"
path = ".chain_verification_report.md"
"#;

    let config_path = Path::new(path);
    if config_path.exists() {
        anyhow::bail!("Config file already exists: {}", path);
    }

    fs::write(config_path, config_content)?;
    println!("Created config file: {}", path);

    Ok(())
}
