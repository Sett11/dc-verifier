use crate::config::Config;
use crate::reporters::{JsonReporter, MarkdownReporter};
use crate::ReportFormat;
use anyhow::Result;
use dc_adapter_fastapi::FastApiCallGraphBuilder;
use dc_core::analyzers::{ChainBuilder, ContractChecker};
use dc_core::data_flow::DataFlowTracker;
use dc_core::models::Severity;
use dc_typescript::TypeScriptCallGraphBuilder;
use indicatif::{ProgressBar, ProgressStyle};
use std::path::PathBuf;

/// Executes data chain verification
pub fn execute_check(config_path: &str, format: ReportFormat, verbose: bool) -> Result<()> {
    // Set verbose mode via environment variable so core modules can check it
    if verbose {
        std::env::set_var("DC_VERIFIER_VERBOSE", "1");
    }
    // 1. Load configuration
    let config = Config::load(config_path)?;

    // 2. Initialize adapters and build graphs
    let mut all_chains = Vec::new();

    // Create progress bar
    let pb = ProgressBar::new(config.adapters.len() as u64);
    pb.set_style(
        ProgressStyle::default_bar()
            .template("{spinner:.green} [{elapsed_precise}] [{bar:40.cyan/blue}] {pos}/{len} adapters {msg}")
            .unwrap()
            .progress_chars("#>-"),
    );
    pb.set_message("Building graphs...");

    for (idx, adapter_config) in config.adapters.iter().enumerate() {
        pb.set_message(format!(
            "Processing adapter {} ({})...",
            idx + 1,
            adapter_config.adapter_type
        ));
        match adapter_config.adapter_type.as_str() {
            "fastapi" => {
                let app_path = adapter_config
                    .app_path
                    .as_ref()
                    .ok_or_else(|| anyhow::anyhow!("FastAPI adapter requires app_path"))?;
                let app_path = PathBuf::from(app_path);

                // Build call graph for FastAPI
                let mut builder = FastApiCallGraphBuilder::new(app_path);
                // Set max recursion depth from config
                if let Some(max_depth) = config.max_recursion_depth {
                    builder = builder.with_max_depth(Some(max_depth));
                }
                let graph = builder.build_graph()?;

                // Create DataFlowTracker and ChainBuilder
                let tracker = DataFlowTracker::new(&graph);
                let chain_builder = ChainBuilder::new(&graph, &tracker);

                // Find all chains
                let chains = chain_builder.find_all_chains()?;
                all_chains.extend(chains);
            }
            "typescript" => {
                let src_paths = adapter_config
                    .src_paths
                    .as_ref()
                    .ok_or_else(|| anyhow::anyhow!("TypeScript adapter requires src_paths"))?;
                let src_paths: Vec<PathBuf> = src_paths.iter().map(PathBuf::from).collect();

                // Build call graph for TypeScript
                let builder = TypeScriptCallGraphBuilder::new(src_paths)
                    .with_max_depth(config.max_recursion_depth);
                let graph = builder.build_graph()?;

                // Create DataFlowTracker and ChainBuilder
                let tracker = DataFlowTracker::new(&graph);
                let chain_builder = ChainBuilder::new(&graph, &tracker);

                // Find all chains
                let chains = chain_builder.find_all_chains()?;
                all_chains.extend(chains);
            }
            _ => {
                eprintln!("Unknown adapter type: {}", adapter_config.adapter_type);
            }
        }
        pb.inc(1);
    }

    pb.set_message("Finding chains...");
    pb.finish_with_message("Graphs built");

    // 3. Check contracts at all junctions
    let pb = ProgressBar::new(all_chains.len() as u64);
    pb.set_style(
        ProgressStyle::default_bar()
            .template("{spinner:.green} [{elapsed_precise}] [{bar:40.cyan/blue}] {pos}/{len} chains {msg}")
            .unwrap()
            .progress_chars("#>-"),
    );
    pb.set_message("Checking contracts...");

    let checker = ContractChecker::new();
    for chain in &mut all_chains {
        for contract in &mut chain.contracts {
            let mismatches = checker.check_contract(contract);
            contract.mismatches = mismatches.clone();

            // Determine severity based on Mismatch types
            contract.severity = if mismatches
                .iter()
                .any(|m| matches!(m.mismatch_type, dc_core::models::MismatchType::TypeMismatch))
            {
                Severity::Critical
            } else if !mismatches.is_empty() {
                Severity::Warning
            } else {
                Severity::Info
            };
        }
        pb.inc(1);
    }

    pb.finish_with_message("Contracts checked");

    // 4. Generate report
    let pb = ProgressBar::new_spinner();
    pb.set_message("Generating report...");
    pb.enable_steady_tick(std::time::Duration::from_millis(100));
    match format {
        ReportFormat::Json => {
            JsonReporter.generate(&all_chains, &config.output.path)?;
        }
        ReportFormat::Markdown => {
            MarkdownReporter.generate(&all_chains, &config.output.path)?;
        }
    }

    pb.finish_with_message("Report generated");

    println!(
        "Verification completed. Report saved to {}",
        config.output.path
    );

    Ok(())
}
