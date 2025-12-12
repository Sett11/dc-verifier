use anyhow::Result;
use pyo3::prelude::*;
use pyo3::types::PyAny;
use std::path::PathBuf;

/// Extracts FastAPI application and routes
pub struct FastApiExtractor {
    app_path: PathBuf,
}

impl FastApiExtractor {
    /// Creates a new extractor
    pub fn new(app_path: PathBuf) -> Self {
        Self { app_path }
    }

    /// Loads FastAPI app via PyO3
    pub fn load_app(&self) -> Result<Py<PyAny>> {
        Python::attach(|py| {
            // Dynamic import of FastAPI app using importlib
            let importlib = py.import("importlib.util")?;
            let spec_from_file = importlib.getattr("spec_from_file_location")?;
            let module_from_spec = importlib.getattr("module_from_spec")?;

            // Create spec from file
            let app_path_str = self.app_path.to_str().ok_or_else(|| {
                anyhow::anyhow!("App path contains invalid UTF-8: {:?}", self.app_path)
            })?;
            let spec = spec_from_file.call1(("app", app_path_str))?;

            // Get loader before moving spec
            let loader = spec.getattr("loader")?;
            let module = module_from_spec.call1((spec,))?;

            // Load module
            loader.call_method1("exec_module", (module.clone(),))?;

            // Get app
            let app = module.getattr("app")?;
            Ok(app.into())
        })
    }

    /// Extracts routes from FastAPI app
    pub fn extract_routes(&self, app: &Bound<'_, PyAny>) -> Result<Vec<FastApiRoute>> {
        Python::attach(|py| {
            // Get routes from app
            let routes = app.getattr("routes")?;
            let routes_list: Vec<Py<PyAny>> = routes.extract()?;

            let mut result = Vec::new();

            // Import APIRoute for type checking
            let fastapi_routing = py.import("fastapi.routing")?;
            let api_route_class = fastapi_routing.getattr("APIRoute")?;

            // Import inspect to get file information
            let inspect = py.import("inspect")?;
            let getfile = inspect.getattr("getfile")?;
            let getsourcelines = inspect.getattr("getsourcelines")?;

            for route in routes_list {
                let route_bound = route.bind(py);

                // Check if this is APIRoute
                let is_api_route = route_bound.is_instance(api_route_class.as_ref())?;
                if !is_api_route {
                    continue;
                }

                // Extract path
                let path: String = route_bound.getattr("path")?.extract()?;

                // Extract methods
                let methods_attr = route_bound.getattr("methods")?;
                let methods: Option<Vec<String>> = methods_attr.extract().ok();
                let method = methods
                    .and_then(|m| m.first().cloned())
                    .unwrap_or_else(|| "GET".to_string());

                // Extract endpoint
                let endpoint = route_bound.getattr("endpoint")?;
                let handler: String = endpoint.getattr("__name__")?.extract()?;

                // Get file and line information
                let (handler_file, handler_line) = match getfile.call1((endpoint.clone(),)) {
                    Ok(file_obj) => {
                        let file_str: String = file_obj.extract()?;
                        let file_path = PathBuf::from(file_str);

                        // Get line number
                        let line = match getsourcelines.call1((endpoint,)) {
                            Ok(lines_tuple) => {
                                let lines: (Vec<String>, usize) = lines_tuple.extract()?;
                                lines.1
                            }
                            Err(_) => 0,
                        };

                        (file_path, line)
                    }
                    Err(_) => {
                        // If unable to get file, use app_path
                        (self.app_path.clone(), 0)
                    }
                };

                result.push(FastApiRoute {
                    path,
                    method,
                    handler,
                    handler_file,
                    handler_line,
                });
            }

            Ok(result)
        })
    }
}

/// FastAPI route
#[derive(Debug, Clone)]
pub struct FastApiRoute {
    pub path: String,
    pub method: String,
    pub handler: String,
    pub handler_file: PathBuf,
    pub handler_line: usize,
}
