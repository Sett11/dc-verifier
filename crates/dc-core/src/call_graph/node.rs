use crate::models::{Location, NodeId, SchemaReference, TypeInfo};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Node in call graph - represents function, class, method or route
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum CallNode {
    /// Module (separate Python/TypeScript file)
    Module {
        /// Path to module file
        path: PathBuf,
    },
    /// Function
    Function {
        /// Function name
        name: String,
        /// File where function is defined
        file: PathBuf,
        /// Definition line number
        line: usize,
        /// Function parameters
        parameters: Vec<Parameter>,
        /// Return type (if known)
        return_type: Option<TypeInfo>,
    },
    /// Class
    Class {
        /// Class name
        name: String,
        /// File where class is defined
        file: PathBuf,
        /// References to class methods
        methods: Vec<NodeId>,
    },
    /// Class method
    Method {
        /// Method name
        name: String,
        /// Reference to owner class
        class: NodeId,
        /// Method parameters
        parameters: Vec<Parameter>,
        /// Return type
        return_type: Option<TypeInfo>,
    },
    /// API Route (FastAPI, Express, etc.)
    Route {
        /// Route path (e.g., "/api/auth/login")
        path: String,
        /// HTTP method
        method: HttpMethod,
        /// Reference to handler function
        handler: NodeId,
        /// Location in code
        location: Location,
        /// Request body schema (if any)
        request_schema: Option<SchemaReference>,
        /// Response schema (if any)
        response_schema: Option<SchemaReference>,
    },
    /// Schema (Pydantic, Zod, TypeScript, OpenAPI, etc.)
    Schema {
        /// Schema reference
        schema: SchemaReference,
    },
}

/// Function/method parameter
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Parameter {
    /// Parameter name
    pub name: String,
    /// Parameter type
    pub type_info: TypeInfo,
    /// Whether it is optional
    pub optional: bool,
    /// Default value (if any)
    pub default_value: Option<String>,
}

/// HTTP method
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum HttpMethod {
    Get,
    Post,
    Put,
    Patch,
    Delete,
    Options,
    Head,
}

impl HttpMethod {
    /// Convenient method to get Option
    pub fn from_str_opt(s: &str) -> Option<Self> {
        s.parse().ok()
    }
}

impl std::str::FromStr for HttpMethod {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        // Use eq_ignore_ascii_case to avoid allocation
        if s.eq_ignore_ascii_case("GET") {
            Ok(HttpMethod::Get)
        } else if s.eq_ignore_ascii_case("POST") {
            Ok(HttpMethod::Post)
        } else if s.eq_ignore_ascii_case("PUT") {
            Ok(HttpMethod::Put)
        } else if s.eq_ignore_ascii_case("PATCH") {
            Ok(HttpMethod::Patch)
        } else if s.eq_ignore_ascii_case("DELETE") {
            Ok(HttpMethod::Delete)
        } else if s.eq_ignore_ascii_case("OPTIONS") {
            Ok(HttpMethod::Options)
        } else if s.eq_ignore_ascii_case("HEAD") {
            Ok(HttpMethod::Head)
        } else {
            Err(())
        }
    }
}
