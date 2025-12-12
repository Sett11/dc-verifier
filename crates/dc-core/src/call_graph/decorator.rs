use crate::models::Location;

/// Function decorator (for Python)
#[derive(Debug, Clone)]
pub struct Decorator {
    /// Decorator name (e.g., "app.post")
    pub name: String,
    /// Decorator arguments
    pub arguments: Vec<String>,
    /// Location in code
    pub location: Location,
    /// Name of function to which decorator is applied
    pub target_function: Option<String>,
}
