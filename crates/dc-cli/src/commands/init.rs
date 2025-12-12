use anyhow::Result;
use std::fs;
use std::path::Path;

/// Creates the configuration file
pub fn execute_init(path: &str) -> Result<()> {
    let config_content = r#"project_name = "MyApp"
entry_point = "backend/api/main.py"

# Maximum recursion depth for graph building (optional, None = unlimited)
# max_recursion_depth = 100

[[adapters]]
type = "fastapi"
app_path = "backend/api/main.py"

[[adapters]]
type = "typescript"
src_paths = ["frontend/src"]

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
