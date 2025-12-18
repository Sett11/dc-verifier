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
    pub keyword_arguments: HashMap<String, String>,
    /// Location in code
    pub location: Location,
    /// Name of function to which decorator is applied
    pub target_function: Option<String>,
}
