use dc_core::analyzers::{ChainBuilder, ContractChecker};
use dc_core::call_graph::{CallGraph, CallNode, Parameter};
use dc_core::data_flow::DataFlowTracker;
use dc_core::models::{BaseType, SchemaReference, SchemaType, TypeInfo};
use std::path::PathBuf;

#[test]
fn test_chain_with_typescript_function() {
    let mut graph = CallGraph::new();

    // Create function with TypeScript schema
    let function_node = graph.add_node(CallNode::Function {
        name: "processUser".to_string(),
        file: PathBuf::from("service.ts"),
        line: 10,
        parameters: vec![Parameter {
            name: "user".to_string(),
            type_info: TypeInfo {
                base_type: BaseType::Object,
                schema_ref: Some(SchemaReference {
                    name: "User".to_string(),
                    schema_type: SchemaType::TypeScript,
                    location: dc_core::models::Location {
                        file: "types.ts".to_string(),
                        line: 5,
                        column: None,
                    },
                    metadata: {
                        let mut m = std::collections::HashMap::new();
                        m.insert(
                            "fields".to_string(),
                            "name:string:required,age:number:required".to_string(),
                        );
                        m
                    },
                }),
                constraints: Vec::new(),
                optional: false,
            },
            optional: false,
            default_value: None,
        }],
        return_type: Some(TypeInfo {
            base_type: BaseType::Object,
            schema_ref: Some(SchemaReference {
                name: "UserResponse".to_string(),
                schema_type: SchemaType::TypeScript,
                location: dc_core::models::Location {
                    file: "types.ts".to_string(),
                    line: 10,
                    column: None,
                },
                metadata: {
                    let mut m = std::collections::HashMap::new();
                    m.insert(
                        "fields".to_string(),
                        "id:string:required,user:object:required".to_string(),
                    );
                    m
                },
            }),
            constraints: Vec::new(),
            optional: false,
        }),
    });

    let tracker = DataFlowTracker::new(&graph);
    let _builder = ChainBuilder::new(&graph, &tracker);

    // Check that function contains TypeScript schema in parameters
    let function_node_data = graph.node_weight(function_node).unwrap();
    if let CallNode::Function { parameters, .. } = function_node_data {
        assert_eq!(parameters.len(), 1);
        let param = &parameters[0];
        assert_eq!(param.name, "user");
        if let Some(schema) = &param.type_info.schema_ref {
            assert_eq!(schema.schema_type, SchemaType::TypeScript);
            assert_eq!(schema.name, "User");
        } else {
            panic!("Expected schema_ref in parameter");
        }
    } else {
        panic!("Expected Function node");
    }
}

#[test]
fn test_contract_checker_with_typescript_schemas() {
    use dc_core::models::Contract;

    let from_schema = SchemaReference {
        name: "UserRequest".to_string(),
        schema_type: SchemaType::TypeScript,
        location: dc_core::models::Location {
            file: "types.ts".to_string(),
            line: 1,
            column: None,
        },
        metadata: {
            let mut m = std::collections::HashMap::new();
            m.insert(
                "fields".to_string(),
                "name:string:required,age:number:required".to_string(),
            );
            m
        },
    };

    let to_schema = SchemaReference {
        name: "User".to_string(),
        schema_type: SchemaType::TypeScript,
        location: dc_core::models::Location {
            file: "types.ts".to_string(),
            line: 5,
            column: None,
        },
        metadata: {
            let mut m = std::collections::HashMap::new();
            m.insert(
                "fields".to_string(),
                "name:string:required,age:number:required,email:string:optional".to_string(),
            );
            m
        },
    };

    let contract = Contract {
        from_link_id: "link1".to_string(),
        to_link_id: "link2".to_string(),
        from_schema: from_schema.clone(),
        to_schema: to_schema.clone(),
        mismatches: Vec::new(),
        severity: dc_core::models::Severity::Info,
    };

    let checker = ContractChecker::new();
    let mismatches = checker.check_contract(&contract);

    // These schemas are expected to be compatible, so no mismatches should be present
    assert_eq!(contract.from_schema.name, "UserRequest");
    assert_eq!(contract.to_schema.name, "User");
    assert!(
        mismatches.is_empty(),
        "Expected no mismatches for compatible schemas, got {:?}",
        mismatches
    );
}

#[test]
fn test_schema_parser_typescript() {
    use dc_core::analyzers::SchemaParser;

    let schema_ref = SchemaReference {
        name: "User".to_string(),
        schema_type: SchemaType::TypeScript,
        location: dc_core::models::Location {
            file: "types.ts".to_string(),
            line: 1,
            column: None,
        },
        metadata: {
            let mut m = std::collections::HashMap::new();
            m.insert(
                "fields".to_string(),
                "name:string:required,age:number:required,email:string:optional".to_string(),
            );
            m
        },
    };

    let json_schema = SchemaParser::parse(&schema_ref).unwrap();

    assert_eq!(json_schema.schema_type, "object");
    assert_eq!(json_schema.properties.len(), 3);
    assert!(json_schema.properties.contains_key("name"));
    assert!(json_schema.properties.contains_key("age"));
    assert!(json_schema.properties.contains_key("email"));
    assert_eq!(json_schema.required.len(), 2); // name and age
    assert!(json_schema.required.contains(&"name".to_string()));
    assert!(json_schema.required.contains(&"age".to_string()));
}
