use crate::logging::formatter::LogFormat;
use std::path::PathBuf;

/// Configuration for logging system
#[derive(Debug, Clone)]
pub struct LoggingConfig {
    /// Log level (error, warn, info, debug, trace)
    pub level: String,
    /// Path to log file (None = no file logging)
    pub file: Option<PathBuf>,
    /// Log to console (true) or only to file (false)
    pub console: bool,
    /// Log format (text or json)
    pub format: LogFormat,
}

impl Default for LoggingConfig {
    fn default() -> Self {
        Self {
            level: std::env::var("RUST_LOG").unwrap_or_else(|_| "info".to_string()),
            file: std::env::var("DCV_LOG_FILE")
                .ok()
                .map(PathBuf::from),
            console: true,
            format: LogFormat::Text,
        }
    }
}

impl LoggingConfig {
    /// Create a new logging configuration
    pub fn new(level: String, file: Option<PathBuf>, console: bool, format: LogFormat) -> Self {
        Self {
            level,
            file,
            console,
            format,
        }
    }
}
