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
    /// Global OpenAPI schema path (optional, can be overridden per adapter)
    pub openapi_path: Option<String>,
    /// Configuration for dynamic route generators
    pub dynamic_routes: Option<DynamicRoutesConfig>,
    /// Strict import resolution: fail on unresolved imports (if true)
    pub strict_imports: Option<bool>,
}

/// Adapter configuration
#[derive(Debug, Deserialize)]
pub struct AdapterConfig {
    #[serde(rename = "type")]
    pub adapter_type: String,
    pub app_path: Option<String>,
    pub src_paths: Option<Vec<String>>,
    /// OpenAPI schema path (optional, overrides global openapi_path if set)
    pub openapi_path: Option<String>,
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

/// Configuration for dynamic route generators
#[derive(Debug, Deserialize, Clone)]
#[allow(dead_code)]
pub struct DynamicRoutesConfig {
    /// List of router generator configurations
    pub generators: Vec<RouterGeneratorConfig>,
}

/// Configuration for a single router generator
#[derive(Debug, Deserialize, Clone)]
#[allow(dead_code)]
pub struct RouterGeneratorConfig {
    /// Module path (e.g., "fastapi_users")
    pub module: String,
    /// Method name (e.g., "get_register_router")
    pub method: String,
    /// List of endpoints this generator creates
    pub endpoints: Vec<EndpointConfig>,
    /// Schema parameter mapping (which argument is request/response schema)
    pub schema_params: Vec<String>,
}

/// Configuration for a single endpoint
#[derive(Debug, Deserialize, Clone)]
#[allow(dead_code)]
pub struct EndpointConfig {
    /// Endpoint path (e.g., "/register")
    pub path: String,
    /// HTTP method (e.g., "GET", "POST")
    pub method: String,
    /// Index of argument for request schema (if any)
    pub request_schema_param: Option<usize>,
    /// Index of argument for response schema (if any)
    pub response_schema_param: Option<usize>,
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

        // Validate global openapi_path if specified
        if let Some(ref openapi_path) = self.openapi_path {
            Self::validate_openapi_path(openapi_path, "Global openapi_path")?;
        }

        // Validate adapter openapi_path if specified
        for (idx, adapter) in self.adapters.iter().enumerate() {
            if let Some(ref openapi_path) = adapter.openapi_path {
                Self::validate_openapi_path(
                    openapi_path,
                    &format!("Adapter {}: openapi_path", idx),
                )?;
            }
        }

        Ok(())
    }

    /// Validates that a path exists and is a readable file
    fn validate_openapi_path(path_str: &str, context: &str) -> Result<()> {
        let path = Path::new(path_str);
        if !path.exists() {
            anyhow::bail!("{} does not exist: {}", context, path_str);
        }
        if !path.is_file() {
            anyhow::bail!("{} must be a file: {}", context, path_str);
        }
        // Check readability
        fs::metadata(path).with_context(|| format!("{} is not readable: {}", context, path_str))?;
        Ok(())
    }

    /// Automatically searches for OpenAPI schema files in the project
    /// Returns the path if found, None otherwise
    pub fn auto_find_openapi(config_file_path: &str) -> Option<String> {
        let config_path = Path::new(config_file_path);
        let project_root = config_path.parent().unwrap_or_else(|| {
            tracing::warn!(
                config_path = ?config_path,
                "Config path has no parent, using current directory"
            );
            Path::new(".")
        });

        // Common OpenAPI schema file names
        let openapi_files = [
            "openapi.json",
            "openapi.yaml",
            "openapi.yml",
            "swagger.json",
        ];

        // Create long-lived PathBufs for project root and common subdirectories
        let project_root_dir = project_root.to_path_buf();
        let backend_dir = project_root.join("backend");
        let fastapi_backend_dir = project_root.join("fastapi_backend");
        let api_dir = project_root.join("api");
        let app_dir = project_root.join("app");

        // Search in project root and common subdirectories
        let search_dirs = [
            &project_root_dir,
            &backend_dir,
            &fastapi_backend_dir,
            &api_dir,
            &app_dir,
        ];

        for dir in &search_dirs {
            for file_name in &openapi_files {
                let file_path = dir.join(file_name);
                if file_path.exists() && file_path.is_file() {
                    // Try to validate it's a valid JSON/YAML
                    if let Ok(content) = fs::read_to_string(&file_path) {
                        // Basic validation: check if it looks like OpenAPI
                        if content.contains("\"openapi\"")
                            || content.contains("openapi:")
                            || content.contains("\"swagger\"")
                            || content.contains("swagger:")
                        {
                            return Some(file_path.to_string_lossy().to_string());
                        }
                    }
                }
            }
        }

        None
    }

    /// Auto-fills missing openapi_path fields by searching the project
    pub fn auto_fill_openapi(&mut self, config_file_path: &str) {
        // Only search if global openapi_path is not set
        if self.openapi_path.is_none() {
            if let Some(found_path) = Self::auto_find_openapi(config_file_path) {
                self.openapi_path = Some(found_path);
            }
        }

        // Auto-fill adapter-specific openapi_path if not set
        for adapter in &mut self.adapters {
            if adapter.openapi_path.is_none() && self.openapi_path.is_none() {
                if let Some(found_path) = Self::auto_find_openapi(config_file_path) {
                    adapter.openapi_path = Some(found_path);
                }
            }
        }
    }
}
