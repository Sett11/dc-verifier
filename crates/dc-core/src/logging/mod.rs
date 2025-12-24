pub mod config;
pub mod file_writer;
pub mod formatter;

use anyhow::Result;
use config::LoggingConfig;
use std::path::PathBuf;

/// Initialize logging system with the given configuration
pub fn init(config: LoggingConfig) -> Result<()> {
    use tracing_subscriber::{
        fmt, layer::SubscriberExt, util::SubscriberInitExt, EnvFilter, Registry,
    };

    let env_filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(&config.level));

    match (config.console, &config.file) {
        (true, Some(log_file)) => {
            // Both console and file
            let file_writer = file_writer::FileWriter::new(log_file.clone());
            Registry::default()
                .with(env_filter)
                .with(
                    fmt::layer()
                        .with_target(true)
                        .with_file(true)
                        .with_line_number(true)
                        .with_thread_ids(false)
                        .with_thread_names(false)
                        .with_ansi(true),
                )
                .with(
                    fmt::layer()
                        .with_writer(file_writer)
                        .with_target(true)
                        .with_file(true)
                        .with_line_number(true)
                        .with_thread_ids(false)
                        .with_thread_names(false)
                        .with_ansi(false)
                        .with_timer(fmt::time::ChronoUtc::rfc_3339()),
                )
                .init();
        }
        (true, None) => {
            // Only console
            Registry::default()
                .with(env_filter)
                .with(
                    fmt::layer()
                        .with_target(true)
                        .with_file(true)
                        .with_line_number(true)
                        .with_thread_ids(false)
                        .with_thread_names(false)
                        .with_ansi(true),
                )
                .init();
        }
        (false, Some(log_file)) => {
            // Only file
            let file_writer = file_writer::FileWriter::new(log_file.clone());
            Registry::default()
                .with(env_filter)
                .with(
                    fmt::layer()
                        .with_writer(file_writer)
                        .with_target(true)
                        .with_file(true)
                        .with_line_number(true)
                        .with_thread_ids(false)
                        .with_thread_names(false)
                        .with_ansi(false)
                        .with_timer(fmt::time::ChronoUtc::rfc_3339()),
                )
                .init();
        }
        (false, None) => {
            // No output (shouldn't happen, but handle it)
            Registry::default().with(env_filter).init();
        }
    }

    Ok(())
}

/// Initialize logging with default configuration
pub fn init_default() -> Result<()> {
    init(LoggingConfig::default())
}

/// Initialize logging from environment variables and CLI arguments
pub fn init_from_args(
    log_level: Option<String>,
    log_file: Option<PathBuf>,
    verbose: bool,
) -> Result<()> {
    let level = if verbose {
        "debug".to_string()
    } else {
        log_level
            .unwrap_or_else(|| std::env::var("RUST_LOG").unwrap_or_else(|_| "info".to_string()))
    };

    let file = log_file.or_else(|| std::env::var("DCV_LOG_FILE").ok().map(PathBuf::from));

    let config = LoggingConfig {
        level,
        file,
        console: true,
        format: formatter::LogFormat::Text,
    };

    init(config)
}
