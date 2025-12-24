use serde::{Deserialize, Serialize};

/// Error that can occur during import resolution in Python projects
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ImportError {
    /// Module is an external dependency, but was not found in project tree or virtual environment
    ExternalDependency {
        /// Name of the imported module (e.g., "fastapi", "pydantic")
        module: String,
        /// Human-readable suggestion, e.g. "Install with: pip install fastapi"
        suggestion: String,
    },
    /// Generic failure to resolve import (path not found, I/O issues, etc.)
    ResolutionFailed {
        /// Original import path string
        import: String,
        /// Description of the failure
        reason: String,
    },
}
