/// Log format options
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LogFormat {
    /// Human-readable text format
    #[default]
    Text,
    /// Structured JSON format
    Json,
}
