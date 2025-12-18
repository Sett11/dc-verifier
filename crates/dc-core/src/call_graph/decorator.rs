use crate::models::Location;
use std::collections::HashMap;

/// Function decorator (for Python)
#[derive(Debug, Clone)]
pub struct Decorator {
    /// Decorator name (e.g., "app.post")
    pub name: String,
    /// Decorator positional arguments
    pub arguments: Vec<String>,
    /// Decorator keyword arguments (e.g., response_model=User)
    /// Values are stringified representations of Python expressions
    pub keyword_arguments: HashMap<String, String>,
    /// Location in code
    pub location: Location,
    /// Name of function to which decorator is applied
    pub target_function: Option<String>,
}

impl Default for Decorator {
    fn default() -> Self {
        Self {
            name: String::new(),
            arguments: Vec::new(),
            keyword_arguments: HashMap::new(),
            location: Location {
                file: String::new(),
                line: 0,
                column: None,
            },
            target_function: None,
        }
    }
}

impl Decorator {
    /// Creates a new Decorator with the given name and location
    pub fn new(name: String, location: Location) -> Self {
        Self {
            name,
            arguments: Vec::new(),
            keyword_arguments: HashMap::new(),
            location,
            target_function: None,
        }
    }
}
