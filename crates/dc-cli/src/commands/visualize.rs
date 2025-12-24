use crate::config::{Config, DynamicRoutesConfig, EndpointConfig, RouterGeneratorConfig};
use anyhow::Result;
use dc_adapter_fastapi::{
    DynamicRoutesConfig as AdapterDynamicRoutesConfig, EndpointConfig as AdapterEndpointConfig,
    FastApiCallGraphBuilder, RouterGeneratorConfig as AdapterRouterGeneratorConfig,
};
use dc_core::call_graph::{CallEdge, CallGraph, CallNode};
use dc_typescript::TypeScriptCallGraphBuilder;
use indicatif::{ProgressBar, ProgressStyle};
use std::fs;
use std::path::PathBuf;
use tracing::error;

/// Visualizes call graphs (optional function)
pub fn execute_visualize(config_path: &str) -> Result<()> {
    let config = Config::load(config_path)?;

    // Build graphs for all adapters
    let mut all_graphs = Vec::new();

    let pb = ProgressBar::new(config.adapters.len() as u64);
    pb.set_style(
        ProgressStyle::default_bar()
            .template("{spinner:.green} [{elapsed_precise}] [{bar:40.cyan/blue}] {pos}/{len} adapters {msg}")
            .expect("Failed to create progress bar template")
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

                let mut builder = FastApiCallGraphBuilder::new(app_path)
                    .with_strict_imports(config.strict_imports.unwrap_or(false));
                // Convert and set dynamic routes config
                let adapter_dynamic_routes = config
                    .dynamic_routes
                    .as_ref()
                    .map(|dr| convert_dynamic_routes_config(dr));
                builder = builder.with_dynamic_routes_config(adapter_dynamic_routes);
                let graph = builder.build_graph()?;
                let unique_id = format!("{}_{}", adapter_config.adapter_type, idx);
                all_graphs.push((unique_id, graph));
            }
            "typescript" => {
                let src_paths = adapter_config
                    .src_paths
                    .as_ref()
                    .ok_or_else(|| anyhow::anyhow!("TypeScript adapter requires src_paths"))?;
                let src_paths: Vec<PathBuf> = src_paths.iter().map(PathBuf::from).collect();

                let builder = TypeScriptCallGraphBuilder::new(src_paths);
                let graph = builder.build_graph()?;
                let unique_id = format!("{}_{}", adapter_config.adapter_type, idx);
                all_graphs.push((unique_id, graph));
            }
            _ => {
                error!(
                    adapter_type = %adapter_config.adapter_type,
                    "Unknown adapter type"
                );
            }
        }
        pb.inc(1);
    }

    pb.finish_with_message("Graphs built");

    // Generate DOT for each graph
    let pb = ProgressBar::new(all_graphs.len() as u64);
    pb.set_style(
        ProgressStyle::default_bar()
            .template("{spinner:.green} [{elapsed_precise}] [{bar:40.cyan/blue}] {pos}/{len} graphs {msg}")
            .unwrap()
            .progress_chars("#>-"),
    );
    pb.set_message("Generating DOT files...");

    let adapter_count = config.adapters.len();
    for (adapter_name, graph) in all_graphs {
        pb.set_message(format!("Generating DOT for {}...", adapter_name));
        let dot_content = generate_dot(&graph, &adapter_name)?;

        // Determine output path
        let output_path = if config.output.path.ends_with(".dot") {
            let base_path = PathBuf::from(&config.output.path);
            if adapter_count > 1 {
                let stem = base_path
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("graph");
                let safe_adapter_name =
                    adapter_name.replace(|c: char| !c.is_ascii_alphanumeric(), "_");
                let parent = base_path
                    .parent()
                    .map(PathBuf::from)
                    .unwrap_or_else(|| PathBuf::from("."));
                parent.join(format!("{}-{}.dot", stem, safe_adapter_name))
            } else {
                base_path
            }
        } else {
            PathBuf::from(&config.output.path).join(format!("{}.dot", adapter_name))
        };

        // Create directory if needed
        if let Some(parent) = output_path.parent() {
            fs::create_dir_all(parent)?;
        }

        // Save DOT file
        fs::write(&output_path, dot_content)?;
        pb.inc(1);
    }

    pb.finish_with_message("DOT files generated");

    println!("Visualization completed. DOT files saved.");

    Ok(())
}

/// Generates DOT format from graph
fn generate_dot(graph: &CallGraph, graph_name: &str) -> Result<String> {
    let mut dot = String::new();

    // DOT header
    dot.push_str(&format!("digraph {} {{\n", graph_name.replace("-", "_")));
    dot.push_str("  rankdir=LR;\n");
    dot.push_str("  node [shape=box];\n\n");

    // Create mapping of node indices to string identifiers
    let mut node_map = std::collections::HashMap::new();
    let mut node_counter = 0;

    // Add nodes
    for node_idx in graph.node_indices() {
        if let Some(node) = graph.node_weight(node_idx) {
            let node_id = format!("node_{}", node_counter);
            node_map.insert(node_idx, node_id.clone());
            node_counter += 1;

            let label = format_node_label(node);
            // Escape special characters for DOT
            let escaped_label = escape_dot_string(&label);
            dot.push_str(&format!("  {} [label=\"{}\"];\n", node_id, escaped_label));
        }
    }

    dot.push('\n');

    // Add edges
    for edge_idx in graph.edge_indices() {
        if let Some((source, target)) = graph.edge_endpoints(edge_idx) {
            if let (Some(source_id), Some(target_id), Some(edge)) = (
                node_map.get(&source),
                node_map.get(&target),
                graph.edge_weight(edge_idx),
            ) {
                let edge_label = format_edge_label(edge);
                let escaped_label = escape_dot_string(&edge_label);
                dot.push_str(&format!(
                    "  {} -> {} [label=\"{}\"];\n",
                    source_id, target_id, escaped_label
                ));
            }
        }
    }

    dot.push_str("}\n");
    Ok(dot)
}

/// Formats node label for DOT
fn format_node_label(node: &CallNode) -> String {
    match node {
        CallNode::Module { path } => {
            format!(
                "Module: {}",
                path.file_name().unwrap_or_default().to_string_lossy()
            )
        }
        CallNode::Function { name, line, .. } => {
            format!("Function: {}\n(line {})", name, line)
        }
        CallNode::Class { name, .. } => {
            format!("Class: {}", name)
        }
        CallNode::Method { name, .. } => {
            format!("Method: {}", name)
        }
        CallNode::Route { path, method, .. } => {
            let method_str = match method {
                dc_core::call_graph::HttpMethod::Get => "GET",
                dc_core::call_graph::HttpMethod::Post => "POST",
                dc_core::call_graph::HttpMethod::Put => "PUT",
                dc_core::call_graph::HttpMethod::Patch => "PATCH",
                dc_core::call_graph::HttpMethod::Delete => "DELETE",
                dc_core::call_graph::HttpMethod::Options => "OPTIONS",
                dc_core::call_graph::HttpMethod::Head => "HEAD",
            };
            format!("Route: {} {}", method_str, path)
        }
        CallNode::Schema { schema } => {
            format!("Schema: {} ({:?})", schema.name, schema.schema_type)
        }
    }
}

/// Formats edge label for DOT
fn format_edge_label(edge: &CallEdge) -> String {
    match edge {
        CallEdge::DataFlow {
            from_schema,
            to_schema,
            transformation,
            ..
        } => {
            let transform_str = transformation
                .as_ref()
                .map(|t| format!(" ({:?})", t))
                .unwrap_or_default();
            format!(
                "{} → {} [label=\"{}{}\", style=dashed, color=purple]",
                from_schema.name, to_schema.name, "DataFlow", transform_str
            )
        }
        CallEdge::Import { import_path, .. } => {
            format!("import: {}", import_path)
        }
        CallEdge::Call {
            argument_mapping, ..
        } => {
            if argument_mapping.is_empty() {
                "calls".to_string()
            } else {
                format!("calls ({} args)", argument_mapping.len())
            }
        }
        CallEdge::Return { return_value, .. } => {
            format!("returns: {}", return_value)
        }
    }
}

/// Escapes special characters for DOT
fn escape_dot_string(s: &str) -> String {
    s.replace("\\", "\\\\")
        .replace("\"", "\\\"")
        .replace("\n", "\\n")
        .replace("\r", "\\r")
        .replace("\t", "\\t")
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
