use std::fs;

use anyhow::Result;
use dc_cli::reporters::{JsonReporter, MarkdownReporter};
use dc_core::models::{
    ChainDirection, ChainType, Contract, DataChain, Link, LinkType, Location, NodeId,
    SchemaReference, SchemaType, Severity,
};

fn dummy_location() -> Location {
    Location {
        file: "dummy.py".to_string(),
        line: 1,
        column: None,
    }
}

fn dummy_schema(name: &str, schema_type: SchemaType) -> SchemaReference {
    SchemaReference {
        name: name.to_string(),
        schema_type,
        location: dummy_location(),
        metadata: std::collections::HashMap::new(),
    }
}

fn build_dummy_chain() -> DataChain {
    let link1 = Link {
        id: "link-1".to_string(),
        link_type: LinkType::Source,
        location: dummy_location(),
        node_id: NodeId::from(dc_core::call_graph::graph::CallGraph::new().add_node(
            dc_core::call_graph::CallNode::Module {
                path: "dummy.py".into(),
            },
        )),
        schema_ref: dummy_schema("RequestModel", SchemaType::Pydantic),
        transformation: None,
    };

    let link2 = Link {
        id: "link-2".to_string(),
        link_type: LinkType::Sink,
        location: dummy_location(),
        node_id: NodeId::from(dc_core::call_graph::graph::CallGraph::new().add_node(
            dc_core::call_graph::CallNode::Module {
                path: "dummy2.py".into(),
            },
        )),
        schema_ref: dummy_schema("ResponseModel", SchemaType::Pydantic),
        transformation: None,
    };

    let contract = Contract {
        from_link_id: link1.id.clone(),
        to_link_id: link2.id.clone(),
        from_schema: link1.schema_ref.clone(),
        to_schema: link2.schema_ref.clone(),
        mismatches: Vec::new(),
        severity: Severity::Info,
    };

    DataChain {
        id: "chain-1".to_string(),
        name: "Dummy Chain".to_string(),
        links: vec![link1, link2],
        contracts: vec![contract],
        direction: ChainDirection::FrontendToBackend,
        chain_type: ChainType::Full,
    }
}

#[test]
fn json_reporter_produces_extended_summary() -> Result<()> {
    let tmp_dir = tempfile::tempdir()?;
    let output_path = tmp_dir.path().join("report.json");

    let reporter = JsonReporter;
    let chains = vec![build_dummy_chain()];
    reporter.generate(&chains, output_path.to_str().unwrap())?;

    let content = fs::read_to_string(&output_path)?;
    let json: serde_json::Value = serde_json::from_str(&content)?;

    // Basic structure checks
    assert!(
        json.get("summary").is_some(),
        "summary section must be present"
    );
    let summary = &json["summary"];
    assert!(summary.get("total_chains").is_some());
    assert!(summary.get("chains_by_type").is_some());
    assert!(summary.get("schemas").is_some());

    Ok(())
}

#[test]
fn markdown_reporter_includes_statistics_and_coverage() -> Result<()> {
    let tmp_dir = tempfile::tempdir()?;
    let output_path = tmp_dir.path().join("report.md");

    let reporter = MarkdownReporter;
    let chains = vec![build_dummy_chain()];
    reporter.generate(&chains, output_path.to_str().unwrap())?;

    let content = fs::read_to_string(&output_path)?;

    assert!(
        content.contains("## Verification Statistics"),
        "markdown report should contain statistics section"
    );
    assert!(
        content.contains("Safe Chains Coverage"),
        "markdown report should contain coverage metric"
    );

    Ok(())
}
