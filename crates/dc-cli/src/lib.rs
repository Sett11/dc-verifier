pub mod commands;
pub mod config;
pub mod reporters;

#[derive(clap::ValueEnum, Clone, Copy, Debug)]
pub enum ReportFormat {
    Markdown,
    Json,
}
