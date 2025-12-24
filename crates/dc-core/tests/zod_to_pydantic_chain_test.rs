use dc_core::analyzers::ChainBuilder;
use dc_core::call_graph::{CallGraph, CallNode, HttpMethod};
use dc_core::data_flow::DataFlowTracker;
use dc_core::models::{
    BaseType, ChainDirection, Location, NodeId, PydanticFieldInfo, SchemaReference, SchemaType,
    TypeInfo, ZodField, ZodUsage,
};
use dc_core::openapi::{OpenAPILinker, OpenAPIParser};

/// Helper to build a minimal OpenAPI schema string with a single endpoint and components.
fn build_openapi_schema_str() -> String {
    r##"
{
  "openapi": "3.0.0",
  "info": { "title": "Test API", "version": "1.0.0" },
  "paths": {
    "/items": {
      "get": {
        "operationId": "getItems",
        "requestBody": {
          "content": {
            "application/json": {
              "schema": { "$ref": "#/components/schemas/ItemRequest" }
            }
          }
        },
        "responses": {
          "200": {
            "description": "OK",
            "content": {
              "application/json": {
                "schema": { "$ref": "#/components/schemas/ItemResponse" }
              }
            }
          }
        }
      }
    }
  },
  "components": {
    "schemas": {
      "ItemRequest": {
        "type": "object",
        "properties": {
          "id": { "type": "string" }
        }
      },
      "ItemResponse": {
        "type": "object",
        "properties": {
          "id": { "type": "string" }
        }
      }
    }
  }
}
"##
    .to_string()
}

#[test]
fn test_build_zod_to_pydantic_chain_via_openapi() {
    // 1. Build minimal OpenAPI linker
    let schema_str = build_openapi_schema_str();
    let openapi_schema =
        OpenAPIParser::parse_str(&schema_str).expect("Failed to parse test OpenAPI schema");
    let openapi_linker = OpenAPILinker::new(openapi_schema);

    // 2. Build a minimal call graph with:
    //    - Zod schema node
    //    - Route node (TS API call)
    //    - Pydantic schema node
    let mut graph = CallGraph::new();

    // Zod schema with one string field "id"
    let zod_fields = vec![ZodField {
        name: "id".to_string(),
        type_name: "string".to_string(),
        optional: false,
        nullable: false,
    }];
    let zod_fields_json =
        serde_json::to_string(&zod_fields).expect("Failed to serialize Zod fields");

    let api_location = Location {
        file: "frontend/api.ts".to_string(),
        line: 10,
        column: None,
    };

    let zod_usage = ZodUsage {
        schema_name: "ItemRequestZod".to_string(),
        method: "safeParse".to_string(),
        location: Location {
            file: "frontend/api.ts".to_string(),
            line: 8,
            column: None,
        },
        api_call_location: Some(api_location.clone()),
    };
    let zod_usages_json =
        serde_json::to_string(&vec![zod_usage]).expect("Failed to serialize Zod usages");

    let zod_schema_ref = SchemaReference {
        name: "ItemRequestZod".to_string(),
        schema_type: SchemaType::Zod,
        location: Location {
            file: "frontend/schemas.ts".to_string(),
            line: 1,
            column: None,
        },
        metadata: {
            let mut m = std::collections::HashMap::new();
            m.insert("fields".to_string(), zod_fields_json);
            m.insert("usages".to_string(), zod_usages_json);
            m
        },
    };

    let _zod_node_idx = graph.add_node(CallNode::Schema {
        schema: zod_schema_ref,
    });

    // Route node that matches OpenAPI endpoint /items GET
    let handler_type = TypeInfo {
        base_type: BaseType::Object,
        schema_ref: None,
        constraints: Vec::new(),
        optional: false,
    };
    let handler_node_idx = graph.add_node(CallNode::Function {
        name: "getItemsHandler".to_string(),
        file: std::path::PathBuf::from("frontend/api.ts"),
        line: 9,
        parameters: Vec::new(),
        return_type: Some(handler_type),
    });
    let handler_node_id = NodeId::from(handler_node_idx);

    let _route_node_idx = graph.add_node(CallNode::Route {
        path: "/items".to_string(),
        method: HttpMethod::Get,
        handler: handler_node_id,
        location: api_location,
        request_schema: None,
        response_schema: None,
    });

    // Pydantic model that should be resolved from OpenAPI "ItemRequest"
    let pydantic_fields = vec![PydanticFieldInfo {
        name: "id".to_string(),
        type_name: "str".to_string(),
        inner_type: None,
        optional: false,
        constraints: Vec::new(),
        default_value: None,
    }];
    let pydantic_fields_json =
        serde_json::to_string(&pydantic_fields).expect("Failed to serialize Pydantic fields");

    let pydantic_schema_ref = SchemaReference {
        name: "ItemRequest".to_string(),
        schema_type: SchemaType::Pydantic,
        location: Location {
            file: "backend/models.py".to_string(),
            line: 1,
            column: None,
        },
        metadata: {
            let mut m = std::collections::HashMap::new();
            m.insert("fields".to_string(), pydantic_fields_json);
            m
        },
    };

    let _pydantic_node_idx = graph.add_node(CallNode::Schema {
        schema: pydantic_schema_ref,
    });

    // 3. Build ChainBuilder and request Zod → Pydantic chains
    let tracker = DataFlowTracker::new(&graph);
    let chain_builder = ChainBuilder::new(&graph, &tracker);

    let chains = chain_builder
        .find_zod_to_pydantic_chains(Some(&openapi_linker))
        .expect("Failed to build Zod → Pydantic chains");

    // 4. Validate that we have exactly one chain and it has expected structure
    assert_eq!(chains.len(), 1, "Expected exactly one Zod → Pydantic chain");

    let chain = &chains[0];
    assert_eq!(
        chain.direction,
        ChainDirection::FrontendToBackend,
        "Chain direction should be FrontendToBackend"
    );
    assert_eq!(
        chain.links.len(),
        3,
        "Expected Zod → Route → Pydantic links"
    );

    // First link: Zod source
    let zod_link = &chain.links[0];
    assert_eq!(zod_link.schema_ref.schema_type, SchemaType::Zod);

    // Second link: Route transformer
    let route_link = &chain.links[1];
    assert!(matches!(
        route_link.link_type,
        dc_core::models::LinkType::Transformer
    ));

    // Third link: Pydantic sink
    let pydantic_link = &chain.links[2];
    assert_eq!(pydantic_link.schema_ref.schema_type, SchemaType::Pydantic);

    // There should be at least one contract, and mismatches array should be consistent
    assert!(
        !chain.contracts.is_empty(),
        "Expected at least one contract in Zod → Pydantic chain"
    );
}
