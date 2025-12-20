use anyhow::{Context, Result};
use serde_json::Value;
use std::path::{Path, PathBuf};

/// Resolves TypeScript path mappings from tsconfig.json
pub struct TypeScriptPathResolver {
    /// Map of path patterns to their resolved paths
    /// Example: "@/*" -> "src/*"
    mappings: Vec<(String, String)>,
    /// Base URL from tsconfig.json
    base_url: Option<PathBuf>,
    /// Project root directory
    project_root: PathBuf,
}

impl TypeScriptPathResolver {
    /// Creates a new path resolver by parsing tsconfig.json
    pub fn new(project_root: &Path) -> Self {
        let mut resolver = Self {
            mappings: Vec::new(),
            base_url: None,
            project_root: project_root.to_path_buf(),
        };

        // Try to find and parse tsconfig.json
        if let Err(err) = resolver.load_tsconfig(project_root) {
            // Silently fail if tsconfig.json is not found or invalid
            // This is expected for projects without TypeScript configuration
            eprintln!("[WARN] Could not load tsconfig.json: {}", err);
        }

        resolver
    }

    /// Loads path mappings from tsconfig.json
    fn load_tsconfig(&mut self, project_root: &Path) -> Result<()> {
        // Try to find tsconfig.json
        let tsconfig_paths = [
            project_root.join("tsconfig.json"),
            project_root.join("tsconfig.base.json"),
        ];

        let mut tsconfig_path = None;
        for path in &tsconfig_paths {
            if path.exists() {
                tsconfig_path = Some(path.clone());
                break;
            }
        }

        let tsconfig_path = tsconfig_path.context("tsconfig.json not found")?;

        // Read and parse JSON
        let content = std::fs::read_to_string(&tsconfig_path)
            .with_context(|| format!("Failed to read {:?}", tsconfig_path))?;

        let json: Value = serde_json::from_str(&content)
            .with_context(|| format!("Failed to parse {:?}", tsconfig_path))?;

        // Extract baseUrl
        if let Some(base_url_str) = json
            .get("compilerOptions")
            .and_then(|opts| opts.get("baseUrl"))
            .and_then(|v| v.as_str())
        {
            let base_url = if base_url_str == "." {
                project_root.to_path_buf()
            } else {
                project_root.join(base_url_str)
            };
            self.base_url = Some(base_url);
        }

        // Extract paths
        if let Some(paths) = json
            .get("compilerOptions")
            .and_then(|opts| opts.get("paths"))
            .and_then(|v| v.as_object())
        {
            for (pattern, targets) in paths {
                if let Some(targets_array) = targets.as_array() {
                    for target in targets_array {
                        if let Some(target_str) = target.as_str() {
                            self.mappings
                                .push((pattern.clone(), target_str.to_string()));
                        }
                    }
                }
            }
        }

        Ok(())
    }

    /// Resolves a path mapping (e.g., "@/app/..." -> "src/app/...")
    pub fn resolve_path_mapping(&self, import_path: &str) -> Option<PathBuf> {
        // Check if import path starts with a mapping pattern
        for (pattern, target) in &self.mappings {
            if self.matches_pattern(import_path, pattern) {
                return self.apply_mapping(import_path, pattern, target);
            }
        }

        None
    }

    /// Checks if import path matches a pattern
    fn matches_pattern(&self, import_path: &str, pattern: &str) -> bool {
        // Handle wildcard patterns like "@/*"
        if pattern.ends_with("/*") {
            let prefix = &pattern[..pattern.len() - 2];
            import_path.starts_with(prefix)
        } else {
            // Exact match
            import_path == pattern
        }
    }

    /// Applies a path mapping to resolve the actual file path
    fn apply_mapping(&self, import_path: &str, pattern: &str, target: &str) -> Option<PathBuf> {
        let base = self.base_url.as_ref().unwrap_or(&self.project_root);

        if pattern.ends_with("/*") {
            // Wildcard pattern: "@/*" -> "src/*"
            let prefix = &pattern[..pattern.len() - 2];
            if let Some(remaining) = import_path.strip_prefix(prefix) {
                // Remove leading slash if present
                let remaining = remaining.trim_start_matches('/');

                // Replace * in target with remaining path
                if target.ends_with("/*") {
                    let target_base = &target[..target.len() - 2];
                    return Some(base.join(target_base).join(remaining));
                } else {
                    // Target doesn't have wildcard, just append remaining
                    return Some(base.join(target).join(remaining));
                }
            }
        } else {
            // Exact match
            if import_path == pattern {
                return Some(base.join(target));
            }
        }

        None
    }

    /// Checks if a path uses path mappings (starts with @ or other known prefixes)
    pub fn is_path_mapping(&self, import_path: &str) -> bool {
        // Check if import path starts with any known pattern prefix
        for (pattern, _) in &self.mappings {
            if pattern.ends_with("/*") {
                let prefix = &pattern[..pattern.len() - 2];
                if import_path.starts_with(prefix) {
                    return true;
                }
            } else if import_path == pattern {
                return true;
            }
        }
        false
    }
}

impl Default for TypeScriptPathResolver {
    fn default() -> Self {
        Self {
            mappings: Vec::new(),
            base_url: None,
            project_root: PathBuf::from("."),
        }
    }
}
