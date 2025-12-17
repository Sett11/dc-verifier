use anyhow::Result;
use dc_core::call_graph::extractor::PydanticSchemaExtractor;
use dc_core::models::{Location, SchemaReference, SchemaType};
use dc_core::parsers::{LocationConverter, PythonParser};
use pyo3::prelude::*;
use pyo3::types::PyAny;
use rustpython_parser::{parse, Mode};
use std::fs;
use std::path::Path;

/// Extractor for Pydantic models
pub struct PydanticExtractor;

impl PydanticExtractor {
    fn serialize_schema(py: Python<'_>, model: &Bound<'_, PyAny>) -> Option<String> {
        let schema = model.call_method0("model_json_schema").ok()?;
        let json_module = py.import("json").ok()?;
        let json_dumps = json_module.getattr("dumps").ok()?;
        json_dumps.call1((schema,)).ok()?.extract::<String>().ok()
    }

    /// Creates a new extractor
    pub fn new() -> Self {
        Self
    }

    /// Extracts a Pydantic model from a function parameter
    ///
    /// Attempts to automatically determine the file path using `inspect.getfile()`.
    /// If `file_path` is provided, it will be used as a fallback if `inspect.getfile()` fails.
    /// If both methods fail, an empty string will be used for the file path.
    ///
    /// # Arguments
    /// * `param` - The Python parameter object (can be a Pydantic model class or instance)
    /// * `file_path` - Optional file path to use if automatic detection fails
    pub fn extract_from_parameter(
        &self,
        param: &Bound<'_, PyAny>,
        file_path: Option<&str>,
    ) -> Option<SchemaReference> {
        let py = param.py();

        // Try to get file path from Python object using inspect.getfile
        let resolved_file_path = if let Some(fp) = file_path {
            fp.to_string()
        } else {
            // Attempt to get file path from Python object
            let inspect = py.import("inspect").ok()?;
            let getfile = inspect.getattr("getfile").ok()?;
            getfile
                .call1((param,))
                .ok()
                .and_then(|file_obj| file_obj.extract::<String>().ok())
                .unwrap_or_default()
        };

        // Parameter can be:
        // 1. A Pydantic model class (type annotation)
        // 2. An instance of a Pydantic model

        // Import BaseModel for checking
        let pydantic = py.import("pydantic").ok()?;
        let base_model = pydantic.getattr("BaseModel").ok()?;

        // Check if param is a Pydantic BaseModel:
        // 1. It could be an instance of BaseModel (checked via is_instance)
        // 2. It could be a class that inherits from BaseModel (checked via issubclass)
        let is_base_model = if param.is_instance(base_model.as_ref()).unwrap_or(false) {
            // It's an instance of BaseModel
            true
        } else {
            // Check if it's a class that inherits from BaseModel
            // First check if it has model_json_schema or model_fields (Pydantic model attributes)
            let has_model_attrs = param.hasattr("model_json_schema").unwrap_or(false)
                || param.hasattr("model_fields").unwrap_or(false);

            if has_model_attrs {
                // It has Pydantic attributes, likely a class
                true
            } else {
                // Try to check via issubclass if it's a class
                let inspect = py.import("inspect").ok()?;
                let isclass = inspect.getattr("isclass").ok()?;
                let is_class: bool = isclass
                    .call1((param,))
                    .ok()?
                    .extract()
                    .ok()
                    .unwrap_or(false);

                if is_class {
                    // It's a class, check if it's a subclass of BaseModel
                    let builtins = py.import("builtins").ok()?;
                    let issubclass_fn = builtins.getattr("issubclass").ok()?;
                    let base_model_ref: &pyo3::Bound<'_, pyo3::PyAny> = base_model.as_ref();
                    issubclass_fn
                        .call1((param, base_model_ref))
                        .ok()?
                        .extract()
                        .ok()
                        .unwrap_or(false)
                } else {
                    false
                }
            }
        };

        if !is_base_model {
            return None;
        }

        // Extract model name
        let name = param
            .getattr("__name__")
            .and_then(|n| n.extract::<String>())
            .unwrap_or_else(|_| "Unknown".to_string());

        // Extract JSON schema
        let json_schema_str = Self::serialize_schema(py, param).unwrap_or_else(|| "{}".to_string());

        // Extract model fields
        let mut metadata = std::collections::HashMap::new();
        metadata.insert("json_schema".to_string(), json_schema_str);

        // Try to get model_fields
        if let Ok(model_fields) = param.getattr("model_fields") {
            if let Ok(fields_dict) = model_fields.cast::<pyo3::types::PyDict>() {
                let mut fields = Vec::new();
                for (key, value) in fields_dict.iter() {
                    if let Ok(field_name) = key.extract::<String>() {
                        let field_info = value;
                        let field_type = field_info
                            .getattr("annotation")
                            .and_then(|annotation| {
                                annotation.repr().and_then(|r| r.extract::<String>())
                            })
                            .unwrap_or_else(|_| "Any".to_string());
                        fields.push(format!("{}:{}", field_name, field_type));
                    }
                }
                if !fields.is_empty() {
                    metadata.insert("fields".to_string(), fields.join(","));
                }
            }
        }

        Some(SchemaReference {
            name,
            schema_type: SchemaType::Pydantic,
            location: Location {
                file: resolved_file_path,
                line: 0,
                column: None,
            },
            metadata,
        })
    }

    /// Extracts all Pydantic models from a file
    pub fn extract_from_file(&self, path: &Path) -> Result<Vec<SchemaReference>> {
        // Read file
        let source = fs::read_to_string(path)?;

        // Parse AST
        let ast = parse(&source, Mode::Module, path.to_string_lossy().as_ref())?;

        // Create LocationConverter for accurate byte offset conversion
        let converter = LocationConverter::new(source);

        // Use PythonParser to extract models
        let parser = PythonParser::new();
        let file_path = path.to_string_lossy().to_string();
        Ok(parser.extract_pydantic_models(&ast, &file_path, &converter))
    }

    /// Converts a Pydantic model to SchemaReference
    pub fn model_to_schema(
        &self,
        model: &Bound<'_, PyAny>,
        location: Location,
    ) -> Result<SchemaReference> {
        let py = model.py();

        let name_attr = model.getattr("__name__")?;
        let name = name_attr
            .extract::<String>()
            .unwrap_or_else(|_| "Unknown".to_string());

        let mut metadata = std::collections::HashMap::new();
        if let Some(schema_str) = Self::serialize_schema(py, model) {
            metadata.insert("json_schema".to_string(), schema_str);
        }

        Ok(SchemaReference {
            name,
            schema_type: SchemaType::Pydantic,
            location,
            metadata,
        })
    }
}

impl Default for PydanticExtractor {
    fn default() -> Self {
        Self::new()
    }
}

impl PydanticSchemaExtractor for PydanticExtractor {
    fn extract_json_schema(&self, model_name: &str, file_path: &Path) -> Result<Option<String>> {
        #[allow(deprecated)]
        Python::with_gil(|py| -> Result<Option<String>> {
            // Try to load the module from file_path
            let module_path = file_path.parent().and_then(|p| p.to_str()).unwrap_or(".");
            let file_stem = file_path.file_stem().and_then(|s| s.to_str()).unwrap_or("");

            // Add parent directory to sys.path if needed
            let sys = match py.import("sys") {
                Ok(s) => s,
                Err(_) => return Ok(None), // Python not available
            };
            let path_attr = sys.getattr("path")?;
            let path = match path_attr.cast::<pyo3::types::PyList>() {
                Ok(p) => p,
                Err(_) => return Ok(None),
            };
            if path.contains(module_path).unwrap_or(false) {
                // Already in path
            } else if path.insert(0, module_path).is_err() {
                return Ok(None);
            }

            // Try to import the module
            let module_name = file_stem;
            let module = match py.import(module_name) {
                Ok(m) => m,
                Err(_) => {
                    // Try to import from parent directory
                    if let Some(parent) = file_path.parent() {
                        if let Some(parent_name) = parent.file_name().and_then(|n| n.to_str()) {
                            let full_name = format!("{}.{}", parent_name, module_name);
                            match py.import(&full_name) {
                                Ok(m) => m,
                                Err(_) => return Ok(None),
                            }
                        } else {
                            return Ok(None);
                        }
                    } else {
                        return Ok(None);
                    }
                }
            };

            // Try to get the class from the module
            let model_class = match module.getattr(model_name) {
                Ok(cls) => cls,
                Err(_) => return Ok(None),
            };

            // Check if it's a Pydantic BaseModel
            let pydantic = match py.import("pydantic") {
                Ok(p) => p,
                Err(_) => return Ok(None), // Pydantic not available
            };
            let base_model = match pydantic.getattr("BaseModel") {
                Ok(bm) => bm,
                Err(_) => return Ok(None),
            };
            let is_base_model = model_class.hasattr("model_json_schema").unwrap_or(false)
                || model_class
                    .getattr("__mro__")
                    .and_then(|mro| {
                        let mro_tuple = mro.cast::<pyo3::types::PyTuple>()?;
                        // Check if base_model is in the MRO tuple
                        let base_model_ref: &pyo3::Bound<'_, pyo3::PyAny> = base_model.as_ref();
                        let mut found = false;
                        for item in mro_tuple.iter() {
                            if item.is(base_model_ref) {
                                found = true;
                                break;
                            }
                        }
                        Ok(found)
                    })
                    .unwrap_or(false);

            if !is_base_model {
                return Ok(None);
            }

            // Extract JSON schema
            if let Some(schema_str) = Self::serialize_schema(py, &model_class) {
                Ok(Some(schema_str))
            } else {
                Ok(None)
            }
        })
    }
}
