use crate::data_flow::Variable;
use crate::models::NodeId;
use serde::{Deserialize, Serialize};

/// Errors related to DataPath
#[derive(Debug, Clone)]
pub enum DataPathError {
    /// Attempt to set an empty list of nodes
    EmptyNodes,
    /// Attempt to set a path shorter than two nodes
    InsufficientNodes,
}

/// Data path through the call graph
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DataPath {
    /// Sequence of nodes in the path
    nodes: Vec<NodeId>,
    /// Variable that is passed along the path
    variable: Variable,
}

impl DataPath {
    /// Creates a new data path
    /// Requires a non-empty list of nodes (minimum from and to)
    pub fn new(from: NodeId, to: NodeId, variable: Variable) -> Self {
        Self {
            nodes: vec![from, to],
            variable,
        }
    }

    /// Returns the starting node (data source)
    pub fn from(&self) -> Option<&NodeId> {
        self.nodes.first()
    }

    /// Returns the ending node (data sink)
    pub fn to(&self) -> Option<&NodeId> {
        self.nodes.last()
    }

    /// Returns the sequence of nodes
    pub fn nodes(&self) -> &[NodeId] {
        &self.nodes
    }

    /// Returns the variable
    pub fn variable(&self) -> &Variable {
        &self.variable
    }

    /// Adds an intermediate node to the path
    pub fn push_node(&mut self, node: NodeId) {
        if !self.nodes.contains(&node) {
            self.nodes.push(node);
        }
    }

    /// Sets the sequence of nodes while maintaining consistency
    ///
    /// # Errors
    ///
    /// - Returns `Err(DataPathError::EmptyNodes)` if an empty list of nodes is passed.
    /// - Returns `Err(DataPathError::InsufficientNodes)` if a list with one node is passed (minimum two nodes required).
    pub fn set_nodes(&mut self, nodes: Vec<NodeId>) -> Result<(), DataPathError> {
        if nodes.is_empty() {
            return Err(DataPathError::EmptyNodes);
        }
        if nodes.len() < 2 {
            return Err(DataPathError::InsufficientNodes);
        }
        self.nodes = nodes;
        Ok(())
    }
}
