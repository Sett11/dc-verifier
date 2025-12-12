use thiserror::Error;

/// Errors that can occur during parsing
#[derive(Error, Debug)]
pub enum ParseError {
    #[error("Failed to parse file: {0}")]
    FileParse(String),
    #[error("Invalid syntax: {0}")]
    InvalidSyntax(String),
}

/// Errors that can occur during configuration loading/validation
#[derive(Error, Debug)]
pub enum ConfigError {
    #[error("Configuration error: {0}")]
    Validation(String),
    #[error("Failed to load config file: {0}")]
    LoadError(String),
    #[error("Invalid adapter type: {0}")]
    InvalidAdapterType(String),
    #[error("Missing required field: {0}")]
    MissingField(String),
}

/// Errors that can occur during graph building
#[derive(Error, Debug)]
pub enum GraphError {
    #[error("Graph building error: {0}")]
    BuildError(String),
    #[error("Maximum recursion depth ({0}) exceeded")]
    MaxDepthExceeded(usize),
    #[error("Failed to resolve import: {0}")]
    ImportResolution(String),
}

/// Errors that can occur during validation
#[derive(Error, Debug)]
pub enum ValidationError {
    #[error("Validation failed: {0}")]
    Failed(String),
    #[error("Schema validation error: {0}")]
    Schema(String),
}

/// Common error type for the library
#[derive(Error, Debug)]
pub enum DcError {
    #[error(transparent)]
    Parse(#[from] ParseError),
    #[error(transparent)]
    Config(#[from] ConfigError),
    #[error(transparent)]
    Graph(#[from] GraphError),
    #[error(transparent)]
    Validation(#[from] ValidationError),
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}
