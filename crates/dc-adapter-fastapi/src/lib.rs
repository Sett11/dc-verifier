use dc_core::models::DataChain;
use pyo3::prelude::*;

mod call_graph;
mod dynamic_routes;
mod extractor;
mod pydantic;
mod utils;

pub use call_graph::*;
pub use dynamic_routes::*;
pub use extractor::*;
pub use pydantic::*;

/// Python module for FastAPI adapter
#[pymodule]
fn dc_adapter_fastapi(_py: Python, m: Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<FastApiAdapter>()?;
    Ok(())
}

/// Adapter for FastAPI applications
#[pyclass]
#[allow(dead_code)]
pub struct FastApiAdapter {
    app_path: String,
}

#[pymethods]
impl FastApiAdapter {
    #[new]
    fn new(app_path: String) -> Self {
        Self { app_path }
    }

    /// Extracts data chains from FastAPI application
    /// Returns JSON string with data chains
    /// Note: Chain extraction is currently handled by the CLI, not through this Python interface
    fn extract_chains(&self, py: Python) -> PyResult<Py<PyAny>> {
        let chains: Vec<DataChain> = Vec::new();

        // Serialize to JSON and return as Python object
        let json_str = serde_json::to_string(&chains).map_err(|e| {
            PyErr::new::<pyo3::exceptions::PyValueError, _>(format!(
                "Failed to serialize chains: {}",
                e
            ))
        })?;

        // Parse JSON into Python dict/list
        let json_module = py.import("json")?;
        let json_dict = json_module.call_method1("loads", (json_str,))?;
        Ok(json_dict.into())
    }
}
