use std::fs;

use anyhow::Result;

use dc_cli::reporters::{JsonReporter, MarkdownReporter};
use dc_core::call_graph::{graph::CallGraph, CallNode};
use dc_core::models::{
    BaseType, ChainDirection, ChainType, Contract, DataChain, Link, LinkType, Location, Mismatch,
    MismatchType, NodeId, SchemaReference, SchemaType, Severity, SeverityLevel, TypeInfo,
};

fn loc(file: &str, line: u32) -> Location {
    Location {
        file: file.to_string(),
        line: line as usize,
        column: None,
    }
}

fn schema(name: &str, schema_type: SchemaType, file: &str, line: u32) -> SchemaReference {
    SchemaReference {
        name: name.to_string(),
        schema_type,
        location: loc(file, line),
        metadata: std::collections::HashMap::new(),
    }
}

fn make_node(path: &str) -> NodeId {
    let mut g = CallGraph::new();
    let idx = g.add_node(CallNode::Module { path: path.into() });
    NodeId::from(idx)
}

fn build_regression_chains() -> Vec<DataChain> {
    // Chain 1: Frontend Zod → TypeScript → OpenAPI with a type mismatch
    let zod_link = Link {
        id: "zod-src".to_string(),
        link_type: LinkType::Source,
        location: loc("frontend/schemas/item.ts", 5),
        node_id: make_node("frontend/schemas/item.ts"),
        schema_ref: schema(
            "ItemCreateZod",
            SchemaType::Zod,
            "frontend/schemas/item.ts",
            5,
        ),
        transformation: None,
    };

    let ts_link = Link {
        id: "ts-type".to_string(),
        link_type: LinkType::Transformer,
        location: loc("frontend/types/item.ts", 3),
        node_id: make_node("frontend/types/item.ts"),
        schema_ref: schema(
            "ItemCreate",
            SchemaType::TypeScript,
            "frontend/types/item.ts",
            3,
        ),
        transformation: None,
    };

    let openapi_link = Link {
        id: "openapi-target".to_string(),
        link_type: LinkType::Sink,
        location: loc("openapi.json", 1),
        node_id: make_node("openapi.json"),
        schema_ref: schema("ItemCreate", SchemaType::OpenAPI, "openapi.json", 1),
        transformation: None,
    };

    let frontend_mismatch = Mismatch {
        mismatch_type: MismatchType::TypeMismatch,
        path: "discount".to_string(),
        expected: TypeInfo {
            base_type: BaseType::Number,
            schema_ref: None,
            constraints: Vec::new(),
            optional: false,
        },
        actual: TypeInfo {
            base_type: BaseType::String,
            schema_ref: None,
            constraints: Vec::new(),
            optional: false,
        },
        location: loc("frontend/schemas/item.ts", 8),
        message: "Field `discount` has mismatched type between Zod and OpenAPI".to_string(),
        severity_level: SeverityLevel::High,
    };

    let frontend_contract = Contract {
        from_link_id: zod_link.id.clone(),
        to_link_id: openapi_link.id.clone(),
        from_schema: zod_link.schema_ref.clone(),
        to_schema: openapi_link.schema_ref.clone(),
        mismatches: vec![frontend_mismatch],
        severity: Severity::Warning,
    };

    let frontend_chain = DataChain {
        id: "chain-frontend-zod-openapi".to_string(),
        name: "Frontend Zod → TS → OpenAPI".to_string(),
        links: vec![zod_link, ts_link, openapi_link],
        contracts: vec![frontend_contract],
        direction: ChainDirection::FrontendToBackend,
        chain_type: ChainType::Full,
    };

    // Chain 2: Backend Pydantic ↔ ORM with missing required field
    let pydantic_link = Link {
        id: "pydantic-src".to_string(),
        link_type: LinkType::Source,
        location: loc("backend/schemas.py", 10),
        node_id: make_node("backend/schemas.py"),
        schema_ref: schema("ItemRead", SchemaType::Pydantic, "backend/schemas.py", 10),
        transformation: None,
    };

    let orm_link = Link {
        id: "orm-target".to_string(),
        link_type: LinkType::Sink,
        location: loc("backend/models.py", 15),
        node_id: make_node("backend/models.py"),
        schema_ref: schema("Item", SchemaType::OrmModel, "backend/models.py", 15),
        transformation: None,
    };

    let backend_mismatch = Mismatch {
        mismatch_type: MismatchType::MissingField,
        path: "title".to_string(),
        expected: TypeInfo {
            base_type: BaseType::String,
            schema_ref: None,
            constraints: Vec::new(),
            optional: false,
        },
        actual: TypeInfo {
            base_type: BaseType::Unknown,
            schema_ref: None,
            constraints: Vec::new(),
            optional: false,
        },
        location: loc("backend/models.py", 20),
        message: "Field `title` is missing in ORM model".to_string(),
        severity_level: SeverityLevel::Critical,
    };

    let backend_contract = Contract {
        from_link_id: pydantic_link.id.clone(),
        to_link_id: orm_link.id.clone(),
        from_schema: pydantic_link.schema_ref.clone(),
        to_schema: orm_link.schema_ref.clone(),
        mismatches: vec![backend_mismatch],
        severity: Severity::Critical,
    };

    let backend_chain = DataChain {
        id: "chain-backend-pydantic-orm".to_string(),
        name: "Backend Pydantic ↔ ORM".to_string(),
        links: vec![pydantic_link, orm_link],
        contracts: vec![backend_contract],
        direction: ChainDirection::BackendToFrontend,
        chain_type: ChainType::BackendInternal,
    };

    vec![frontend_chain, backend_chain]
}

#[test]
fn json_reporter_summary_is_stable_for_regression_chains() -> Result<()> {
    let tmp_dir = tempfile::tempdir()?;
    let output_path = tmp_dir.path().join("regression.json");

    let reporter = JsonReporter;
    let chains = build_regression_chains();
    reporter.generate(&chains, output_path.to_str().unwrap())?;

    let content = fs::read_to_string(&output_path)?;
    let json: serde_json::Value = serde_json::from_str(&content)?;

    let summary = json
        .get("summary")
        .expect("summary section must be present in JSON report");

    // Snapshot-like check: summary fields and schema type counts must stay stable.
    let chains_by_type = summary
        .get("chains_by_type")
        .and_then(|v| v.as_object())
        .expect("chains_by_type must be an object");

    assert_eq!(chains_by_type.get("full").and_then(|v| v.as_u64()), Some(1));
    assert_eq!(
        chains_by_type
            .get("backend_internal")
            .and_then(|v| v.as_u64()),
        Some(1)
    );

    let schemas = summary
        .get("schemas")
        .expect("schemas section must be present in summary");
    let by_type = schemas
        .get("by_type")
        .and_then(|v| v.as_object())
        .expect("schemas.by_type must be an object");

    // We expect exactly one schema of each involved type.
    assert_eq!(by_type.get("zod").and_then(|v| v.as_u64()), Some(1));
    assert_eq!(by_type.get("typescript").and_then(|v| v.as_u64()), Some(1));
    assert_eq!(by_type.get("openapi").and_then(|v| v.as_u64()), Some(1));
    assert_eq!(by_type.get("pydantic").and_then(|v| v.as_u64()), Some(1));
    assert_eq!(by_type.get("orm_model").and_then(|v| v.as_u64()), Some(1));

    Ok(())
}

#[test]
fn markdown_reporter_contains_mismatch_sections_for_regression_chains() -> Result<()> {
    let tmp_dir = tempfile::tempdir()?;
    let output_path = tmp_dir.path().join("regression.md");

    let reporter = MarkdownReporter;
    let chains = build_regression_chains();
    reporter.generate(&chains, output_path.to_str().unwrap())?;

    let content = fs::read_to_string(&output_path)?;

    // Basic snapshot-style checks: we rely on specific, stable fragments of the report.
    assert!(
        content.contains("Frontend Zod → TS → OpenAPI"),
        "markdown report should mention the frontend chain name"
    );
    assert!(
        content.contains("Backend Pydantic ↔ ORM"),
        "markdown report should mention the backend chain name"
    );
    assert!(
        content.contains("Type mismatches"),
        "markdown report should include a Type mismatches recommendation section for Zod/OpenAPI"
    );
    assert!(
        content.contains("Missing required fields"),
        "markdown report should include a Missing required fields recommendation section for Pydantic/ORM"
    );
    assert!(
        content.contains("Zod Schema"),
        "markdown report should display Zod schema type label"
    );
    assert!(
        content.contains("Pydantic Model"),
        "markdown report should display Pydantic schema type label"
    );
    assert!(
        content.contains("ORM Model"),
        "markdown report should display ORM schema type label"
    );

    Ok(())
}
