use crate::models::NodeId;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Edge in call graph - represents connection between nodes
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum CallEdge {
    /// Module/function import
    Import {
        /// Node that performs import
        from: NodeId,
        /// Node that is imported
        to: NodeId,
        /// Import path (e.g., "fastapi" or "db.crud")
        import_path: String,
        /// File from which import is made
        file: PathBuf,
    },
    /// Function/method call
    Call {
        /// Node that calls (caller)
        caller: NodeId,
        /// Node that is called (callee)
        callee: NodeId,
        /// Argument mapping: (parameter_name, variable_name)
        argument_mapping: Vec<(String, String)>,
        /// Call location in code
        location: crate::models::Location,
    },
    /// Return value
    Return {
        /// Node that returns value
        from: NodeId,
        /// Node that receives value (caller)
        to: NodeId,
        /// Return variable name
        return_value: String,
    },
}
