use anyhow::{Context, Result};
use serde::Deserialize;
use std::fs;
use std::path::Path;

/// Project configuration
#[derive(Debug, Deserialize)]
#[allow(dead_code)]
pub struct Config {
    pub project_name: String,
    pub entry_point: Option<String>,
    pub adapters: Vec<AdapterConfig>,
    pub rules: Option<RulesConfig>,
    pub output: OutputConfig,
    /// Maximum recursion depth for graph building (None = unlimited)
    pub max_recursion_depth: Option<usize>,
}

/// Adapter configuration
#[derive(Debug, Deserialize)]
pub struct AdapterConfig {
    #[serde(rename = "type")]
    pub adapter_type: String,
    pub app_path: Option<String>,
    pub src_paths: Option<Vec<String>>,
}

/// Rules configuration
#[derive(Debug, Deserialize)]
#[allow(dead_code)]
pub struct RulesConfig {
    pub type_mismatch: Option<String>,
    pub missing_field: Option<String>,
    pub unnormalized_data: Option<String>,
}

/// Output configuration
#[derive(Debug, Deserialize)]
#[allow(dead_code)]
pub struct OutputConfig {
    pub format: String,
    pub path: String,
}

impl Config {
    /// Loads configuration from a file
    pub fn load(path: &str) -> Result<Self> {
        let content = fs::read_to_string(Path::new(path))
            .with_context(|| format!("Failed to read config file: {}", path))?;
        let config: Config = toml::from_str(&content)
            .with_context(|| format!("Failed to parse config file: {}", path))?;
        config.validate()?;
        Ok(config)
    }

    /// Validates the configuration
    pub fn validate(&self) -> Result<()> {
        // Validate project_name
        if self.project_name.is_empty() {
            anyhow::bail!("project_name cannot be empty");
        }

        // Validate adapters
        if self.adapters.is_empty() {
            anyhow::bail!("At least one adapter must be configured");
        }

        for (idx, adapter) in self.adapters.iter().enumerate() {
            // Validate adapter_type
            match adapter.adapter_type.as_str() {
                "fastapi" => {
                    // For FastAPI, app_path is required
                    let app_path = adapter.app_path.as_ref().ok_or_else(|| {
                        anyhow::anyhow!("Adapter {}: FastAPI adapter requires app_path", idx)
                    })?;
                    let path = Path::new(app_path);
                    if !path.exists() {
                        anyhow::bail!("Adapter {}: app_path does not exist: {}", idx, app_path);
                    }
                    if !path.is_file() {
                        anyhow::bail!("Adapter {}: app_path must be a file: {}", idx, app_path);
                    }
                }
                "typescript" => {
                    // For TypeScript, src_paths is required
                    let src_paths = adapter.src_paths.as_ref().ok_or_else(|| {
                        anyhow::anyhow!("Adapter {}: TypeScript adapter requires src_paths", idx)
                    })?;
                    if src_paths.is_empty() {
                        anyhow::bail!("Adapter {}: src_paths cannot be empty", idx);
                    }
                    for (path_idx, src_path) in src_paths.iter().enumerate() {
                        let path = Path::new(src_path);
                        if !path.exists() {
                            anyhow::bail!(
                                "Adapter {}: src_paths[{}] does not exist: {}",
                                idx,
                                path_idx,
                                src_path
                            );
                        }
                        if !path.is_dir() {
                            anyhow::bail!(
                                "Adapter {}: src_paths[{}] must be a directory: {}",
                                idx,
                                path_idx,
                                src_path
                            );
                        }
                    }
                }
                "nestjs" => {
                    // For NestJS, src_paths is required
                    let src_paths = adapter.src_paths.as_ref().ok_or_else(|| {
                        anyhow::anyhow!("Adapter {}: NestJS adapter requires src_paths", idx)
                    })?;
                    if src_paths.is_empty() {
                        anyhow::bail!("Adapter {}: src_paths cannot be empty", idx);
                    }
                    for (path_idx, src_path) in src_paths.iter().enumerate() {
                        let path = Path::new(src_path);
                        if !path.exists() {
                            anyhow::bail!(
                                "Adapter {}: src_paths[{}] does not exist: {}",
                                idx,
                                path_idx,
                                src_path
                            );
                        }
                    }
                }
                _ => {
                    anyhow::bail!(
                        "Adapter {}: Unknown adapter type: {}. Supported types: fastapi, typescript, nestjs",
                        idx,
                        adapter.adapter_type
                    );
                }
            }
        }

        // Validate output format
        match self.output.format.as_str() {
            "markdown" | "json" => {}
            _ => {
                anyhow::bail!(
                    "Invalid output format: {}. Supported formats: markdown, json",
                    self.output.format
                );
            }
        }

        // Validate output path
        if self.output.path.is_empty() {
            anyhow::bail!("output.path cannot be empty");
        }

        Ok(())
    }
}
