use anyhow::Result;
use dc_core::call_graph::{CallGraph, CallGraphBuilder};
use std::path::{Path, PathBuf};

/// Call graph builder for FastAPI application
pub struct FastApiCallGraphBuilder {
    core_builder: CallGraphBuilder,
    app_path: PathBuf,
}

impl FastApiCallGraphBuilder {
    /// Creates a new builder
    pub fn new(app_path: PathBuf) -> Self {
        Self {
            core_builder: CallGraphBuilder::new(),
            app_path,
        }
    }

    /// Sets the maximum recursion depth
    pub fn with_max_depth(mut self, max_depth: Option<usize>) -> Self {
        self.core_builder = self.core_builder.with_max_depth(max_depth);
        self
    }

    /// Builds graph for FastAPI application
    /// Consumes self, as it calls into_graph() on core_builder
    pub fn build_graph(self) -> Result<CallGraph> {
        // Determine project root
        let project_root = Self::find_project_root(&self.app_path);

        // Find entry point
        let entry_point = if self.app_path.exists() && self.app_path.is_file() {
            // If app_path points to specific file, use it
            self.app_path.clone()
        } else {
            // Otherwise search for standard entry point
            self.core_builder.find_entry_point(&project_root)?
        };

        // Build call graph from entry point
        // CallGraphBuilder will automatically handle:
        // - Imports
        // - Functions and classes
        // - Function calls
        // - FastAPI decorators (@app.get, @app.post, etc.)
        let mut core_builder = self.core_builder;
        core_builder.build_from_entry(&entry_point)?;

        // Return built graph
        Ok(core_builder.into_graph())
    }

    /// Finds project root by going up from app_path and searching for project markers
    fn find_project_root(app_path: &Path) -> PathBuf {
        let markers = ["pyproject.toml", "setup.py", "requirements.txt", ".git"];
        let mut current = app_path.to_path_buf();

        // If app_path is a file, start from its parent
        if current.is_file() {
            if let Some(parent) = current.parent() {
                current = parent.to_path_buf();
            }
        }

        // Go up until marker is found
        while let Some(parent) = current.parent() {
            // Check for markers
            for marker in &markers {
                let marker_path = parent.join(marker);
                // Handle access errors gracefully
                if marker_path.exists() {
                    return parent.to_path_buf();
                }
            }
            current = parent.to_path_buf();
        }

        // Fallback: return parent of app_path or app_path itself
        app_path
            .parent()
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| app_path.to_path_buf())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn test_find_project_root_with_pyproject() {
        let temp_dir = TempDir::new().unwrap();
        let project_root = temp_dir.path();
        let app_path = project_root.join("src").join("app.py");

        // Create project structure
        fs::create_dir_all(app_path.parent().unwrap()).unwrap();
        fs::write(project_root.join("pyproject.toml"), "[project]").unwrap();
        fs::write(&app_path, "from fastapi import FastAPI").unwrap();

        let found_root = FastApiCallGraphBuilder::find_project_root(&app_path);
        assert_eq!(found_root, project_root);
    }

    #[test]
    fn test_find_project_root_with_git() {
        let temp_dir = TempDir::new().unwrap();
        let project_root = temp_dir.path();
        let app_path = project_root.join("backend").join("api").join("main.py");

        fs::create_dir_all(app_path.parent().unwrap()).unwrap();
        fs::create_dir_all(project_root.join(".git")).unwrap();
        fs::write(&app_path, "from fastapi import FastAPI").unwrap();

        let found_root = FastApiCallGraphBuilder::find_project_root(&app_path);
        assert_eq!(found_root, project_root);
    }

    #[test]
    fn test_find_project_root_fallback() {
        let temp_dir = TempDir::new().unwrap();
        let app_path = temp_dir.path().join("app.py");
        fs::write(&app_path, "from fastapi import FastAPI").unwrap();

        let found_root = FastApiCallGraphBuilder::find_project_root(&app_path);
        // Should return parent of app_path
        assert_eq!(found_root, app_path.parent().unwrap());
    }
}
