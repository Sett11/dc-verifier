use serde::{Deserialize, Serialize};

/// Location in code
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct Location {
    /// File path
    pub file: String,
    /// Line number (1-based)
    pub line: usize,
    /// Column number (optional, 1-based)
    pub column: Option<usize>,
}
