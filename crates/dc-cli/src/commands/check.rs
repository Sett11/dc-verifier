use crate::config::{Config, DynamicRoutesConfig, EndpointConfig, RouterGeneratorConfig};
use crate::reporters::{JsonReporter, MarkdownReporter};
use crate::ReportFormat;
use anyhow::Result;
use dc_adapter_fastapi::{
    DynamicRoutesConfig as AdapterDynamicRoutesConfig, EndpointConfig as AdapterEndpointConfig,
    FastApiCallGraphBuilder, RouterGeneratorConfig as AdapterRouterGeneratorConfig,
};
use dc_adapter_nestjs::NestJSCallGraphBuilder;
use dc_core::analyzers::{ChainBuilder, ContractChecker};
use dc_core::data_flow::DataFlowTracker;
use dc_core::models::Severity;
use dc_core::openapi::{OpenAPILinker, OpenAPIParser};
use dc_typescript::TypeScriptCallGraphBuilder;
use indicatif::{ProgressBar, ProgressStyle};
use std::path::{Path, PathBuf};
use tracing::{error, warn};

/// Executes data chain verification
pub fn execute_check(config_path: &str, format: ReportFormat, verbose: bool) -> Result<()> {
    // 1. Load configuration
    // Determine base path from config file location
    let config_file_path = Path::new(config_path);
    let base_path = config_file_path.parent();
    let mut config = Config::load(config_path, base_path)?;

    // 2. Auto-fill missing OpenAPI paths
    config.auto_fill_openapi(config_path);

    // 2. Parse global OpenAPI schema if specified
    let _global_openapi = config.openapi_path.as_ref().and_then(|path| {
        OpenAPIParser::parse_file(std::path::Path::new(path))
            .map_err(|e| {
                warn!(
                    path = %path,
                    error = %e,
                    "Failed to parse global OpenAPI schema"
                );
            })
            .ok()
    });

    // 3. Initialize adapters and build graphs
    let mut all_chains = Vec::new();

    // Create progress bar
    let pb = ProgressBar::new(config.adapters.len() as u64);
    pb.set_style(
        ProgressStyle::default_bar()
            .template("{spinner:.green} [{elapsed_precise}] [{bar:40.cyan/blue}] {pos}/{len} adapters {msg}")
            .expect("Failed to create progress bar template")
            .progress_chars("#>-"),
    );
    pb.set_message("Building graphs...");

    let mut skipped_adapters = Vec::new();

    for (idx, adapter_config) in config.adapters.iter().enumerate() {
        pb.set_message(format!(
            "Processing adapter {} ({})...",
            idx + 1,
            adapter_config.adapter_type
        ));
        // Determine OpenAPI path for this adapter
        let openapi_path = adapter_config
            .openapi_path
            .as_ref()
            .or(config.openapi_path.as_ref())
            .map(PathBuf::from);

        match adapter_config.adapter_type.as_str() {
            "fastapi" => {
                let app_path = adapter_config
                    .app_path
                    .as_ref()
                    .ok_or_else(|| anyhow::anyhow!("FastAPI adapter requires app_path"))?;
                let app_path = PathBuf::from(app_path);

                // Build call graph for FastAPI
                let mut builder = FastApiCallGraphBuilder::new(app_path)
                    .with_strict_imports(config.strict_imports.unwrap_or(false))
                    .with_verbose(verbose)
                    .with_openapi_schema(openapi_path);
                // Set max recursion depth from config
                if let Some(max_depth) = config.max_recursion_depth {
                    builder = builder.with_max_depth(Some(max_depth));
                }
                // Convert and set dynamic routes config
                let adapter_dynamic_routes = config
                    .dynamic_routes
                    .as_ref()
                    .map(|dr| convert_dynamic_routes_config(dr));
                builder = builder.with_dynamic_routes_config(adapter_dynamic_routes);
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

                // Resolve OpenAPI path for this adapter (can be adapter-specific or global)
                let openapi_path = adapter_config
                    .openapi_path
                    .as_ref()
                    .or(config.openapi_path.as_ref())
                    .map(PathBuf::from);

                // Build optional OpenAPILinker for Zod → Pydantic chains
                let ts_openapi_linker = if let Some(ref path) = openapi_path {
                    match OpenAPIParser::parse_file(path) {
                        Ok(schema) => Some(OpenAPILinker::new(schema)),
                        Err(e) => {
                            warn!(
                                path = ?path,
                                error = %e,
                                "Failed to parse OpenAPI schema for TypeScript adapter"
                            );
                            None
                        }
                    }
                } else {
                    None
                };

                // Build call graph for TypeScript
                let builder = TypeScriptCallGraphBuilder::new(src_paths)
                    .with_max_depth(config.max_recursion_depth)
                    .with_verbose(verbose)
                    .with_openapi_schema(openapi_path);
                let graph = builder.build_graph()?;

                // Create DataFlowTracker and ChainBuilder
                let tracker = DataFlowTracker::new(&graph);
                let chain_builder = ChainBuilder::new(&graph, &tracker);

                // Find all standard chains
                let chains = chain_builder.find_all_chains()?;
                all_chains.extend(chains);

                // Additionally, build Zod → Pydantic chains using OpenAPI linker if available
                if let Some(ref linker) = ts_openapi_linker {
                    let zod_chains = chain_builder.find_zod_to_pydantic_chains(Some(linker))?;
                    all_chains.extend(zod_chains);
                }
            }
            "nestjs" => {
                let src_paths = adapter_config
                    .src_paths
                    .as_ref()
                    .ok_or_else(|| anyhow::anyhow!("NestJS adapter requires src_paths"))?;
                let src_paths: Vec<PathBuf> = src_paths.iter().map(PathBuf::from).collect();

                // Build call graph for NestJS
                let mut builder = NestJSCallGraphBuilder::new(src_paths).with_verbose(verbose);
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
            _ => {
                let adapter_type = adapter_config.adapter_type.clone();
                error!(
                    adapter_type = %adapter_type,
                    "Unknown adapter type"
                );
                skipped_adapters.push(adapter_type);
            }
        }
        pb.inc(1);
    }

    if !skipped_adapters.is_empty() {
        warn!(
            count = skipped_adapters.len(),
            adapters = ?skipped_adapters,
            "Some adapters were skipped due to unknown type"
        );
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

/// Converts CLI config types to adapter config types
fn convert_dynamic_routes_config(config: &DynamicRoutesConfig) -> AdapterDynamicRoutesConfig {
    AdapterDynamicRoutesConfig {
        generators: config
            .generators
            .iter()
            .map(convert_router_generator_config)
            .collect(),
    }
}

fn convert_router_generator_config(config: &RouterGeneratorConfig) -> AdapterRouterGeneratorConfig {
    AdapterRouterGeneratorConfig {
        module: config.module.clone(),
        method: config.method.clone(),
        endpoints: config
            .endpoints
            .iter()
            .map(convert_endpoint_config)
            .collect(),
        schema_params: config.schema_params.clone(),
    }
}

fn convert_endpoint_config(config: &EndpointConfig) -> AdapterEndpointConfig {
    AdapterEndpointConfig {
        path: config.path.clone(),
        method: config.method.clone(),
        request_schema_param: config.request_schema_param,
        response_schema_param: config.response_schema_param,
    }
}
