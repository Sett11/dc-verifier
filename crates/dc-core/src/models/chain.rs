use crate::models::{Contract, Location, SchemaReference};
use petgraph::graph::NodeIndex;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::ops::Deref;

/// Data chain direction
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ChainDirection {
    /// Frontend → Backend → Database
    FrontendToBackend,
    /// Database → Backend → Frontend
    BackendToFrontend,
}

/// Type of data chain
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ChainType {
    /// Full chain: Frontend → Backend → Database (or reverse)
    /// Contains all layers of the application
    Full,
    /// Internal frontend call: Frontend → Frontend
    /// Only frontend components, no backend interaction
    FrontendInternal,
    /// Internal backend call: Backend → Backend
    /// Only backend components, no database interaction
    BackendInternal,
}

/// Main data chain model
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DataChain {
    /// Unique chain identifier (e.g., "auth-login")
    pub id: String,
    /// Human-readable chain name (e.g., "User authentication flow")
    pub name: String,
    /// Chain links (sequence of nodes in graph)
    pub links: Vec<Link>,
    /// Contracts between links (checks at junctions)
    pub contracts: Vec<Contract>,
    /// Data flow direction
    pub direction: ChainDirection,
    /// Type of chain (full or internal)
    pub chain_type: ChainType,
}

/// Chain link - one node in call graph
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Link {
    /// Unique link identifier (e.g., "frontend-form")
    pub id: String,
    /// Link type in chain
    pub link_type: LinkType,
    /// Location in code (file and line)
    pub location: Location,
    /// Reference to node in call graph
    pub node_id: NodeId,
    /// Data schema at this link
    pub schema_ref: SchemaReference,
}

/// Link type in chain
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum LinkType {
    /// Data source (e.g., form on frontend)
    Source,
    /// Data transformer (e.g., validation, normalization)
    Transformer,
    /// Data sink (e.g., saving to database)
    Sink,
}

/// Node identifier in call graph
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct NodeId(pub NodeIndex<u32>);

impl From<NodeIndex<u32>> for NodeId {
    fn from(idx: NodeIndex<u32>) -> Self {
        NodeId(idx)
    }
}

impl From<NodeId> for NodeIndex<u32> {
    fn from(node_id: NodeId) -> Self {
        node_id.0
    }
}

impl NodeId {
    pub fn index(&self) -> usize {
        self.0.index()
    }
}

impl Deref for NodeId {
    type Target = NodeIndex<u32>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl Serialize for NodeId {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_u32(self.0.index() as u32)
    }
}

impl<'de> Deserialize<'de> for NodeId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let idx = u32::deserialize(deserializer)?;
        Ok(NodeId(NodeIndex::new(idx as usize)))
    }
}
