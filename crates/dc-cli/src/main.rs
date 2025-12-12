use anyhow::Result;
use clap::Parser;

mod commands;
mod config;
mod reporters;

#[derive(Parser)]
#[command(name = "dc-verifier")]
#[command(about = "Data Chains Verifier - data chain integrity verification")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(clap::ValueEnum, Clone, Copy, Debug)]
pub enum ReportFormat {
    Markdown,
    Json,
}

#[derive(clap::Subcommand)]
enum Commands {
    /// Check data chains
    Check {
        /// Path to configuration file
        #[arg(short, long, default_value = "dc-verifier.toml")]
        config: String,
        /// Report format (markdown or json)
        #[arg(short, long, value_enum, default_value_t = ReportFormat::Markdown)]
        format: ReportFormat,
        /// Enable verbose debug output
        #[arg(short, long)]
        verbose: bool,
    },
    /// Create configuration file
    Init {
        /// Path for creating config
        #[arg(default_value = "dc-verifier.toml")]
        path: String,
    },
    /// Visualize data chain graphs
    Visualize {
        /// Path to configuration file
        #[arg(short, long, default_value = "dc-verifier.toml")]
        config: String,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Check {
            config,
            format,
            verbose,
        } => {
            commands::check::execute_check(&config, format, verbose)?;
        }
        Commands::Init { path } => {
            commands::init::execute_init(&path)?;
        }
        Commands::Visualize { config } => {
            commands::visualize::execute_visualize(&config)?;
        }
    }

    Ok(())
}
