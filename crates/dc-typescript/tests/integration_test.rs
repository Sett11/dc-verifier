use dc_typescript::TypeScriptCallGraphBuilder;
use tempfile::TempDir;

#[test]
fn test_build_graph_simple() {
    let temp_dir = TempDir::new().unwrap();
    let test_file = temp_dir.path().join("test.ts");

    let source = r#"
import { helper } from './helper';

export function processData(data: string): string {
    return helper(data);
}
"#;

    std::fs::write(&test_file, source).unwrap();

    let helper_file = temp_dir.path().join("helper.ts");
    let helper_source = r#"
export function helper(data: string): string {
    return data.toUpperCase();
}
"#;
    std::fs::write(&helper_file, helper_source).unwrap();

    let builder = TypeScriptCallGraphBuilder::new(vec![temp_dir.path().to_path_buf()]);
    let graph = builder.build_graph().unwrap();

    // Check that graph contains nodes
    assert!(graph.node_count() > 0);

    // Check for modules
    let module_nodes: Vec<_> = graph
        .node_indices()
        .filter_map(|idx| graph.node_weight(idx))
        .filter(|node| matches!(node, dc_core::call_graph::CallNode::Module { .. }))
        .collect();
    assert!(module_nodes.len() >= 2); // test.ts and helper.ts
}

#[test]
fn test_build_graph_with_functions() {
    let temp_dir = TempDir::new().unwrap();
    let test_file = temp_dir.path().join("service.ts");

    let source = r#"
export class UserService {
    async getUser(id: string): Promise<User> {
        return this.fetchUser(id);
    }
    
    private fetchUser(id: string): Promise<User> {
        return Promise.resolve({ id, name: "Test" } as User);
    }
}

export function createService(): UserService {
    return new UserService();
}
"#;

    std::fs::write(&test_file, source).unwrap();

    let builder = TypeScriptCallGraphBuilder::new(vec![temp_dir.path().to_path_buf()]);
    let graph = builder.build_graph().unwrap();

    // Check for functions and classes
    let function_nodes: Vec<_> = graph
        .node_indices()
        .filter_map(|idx| graph.node_weight(idx))
        .filter(|node| matches!(node, dc_core::call_graph::CallNode::Function { .. }))
        .collect();

    let class_nodes: Vec<_> = graph
        .node_indices()
        .filter_map(|idx| graph.node_weight(idx))
        .filter(|node| matches!(node, dc_core::call_graph::CallNode::Class { .. }))
        .collect();

    assert!(!function_nodes.is_empty());
    assert!(!class_nodes.is_empty());
}

#[test]
fn test_build_graph_with_imports() {
    let temp_dir = TempDir::new().unwrap();

    let main_file = temp_dir.path().join("main.ts");
    let main_source = r#"
import { processUser } from './user';
import { validate } from './validator';

export function main() {
    const user = processUser({ name: "Test" });
    validate(user);
}
"#;
    std::fs::write(&main_file, main_source).unwrap();

    let user_file = temp_dir.path().join("user.ts");
    let user_source = r#"
export function processUser(data: any): any {
    return data;
}
"#;
    std::fs::write(&user_file, user_source).unwrap();

    let validator_file = temp_dir.path().join("validator.ts");
    let validator_source = r#"
export function validate(data: any): void {
    // validation
}
"#;
    std::fs::write(&validator_file, validator_source).unwrap();

    let builder = TypeScriptCallGraphBuilder::new(vec![temp_dir.path().to_path_buf()]);
    let graph = builder.build_graph().unwrap();

    // Check for imports (may or may not be present, depending on implementation)
    let _import_edges: Vec<_> = graph
        .edge_indices()
        .filter_map(|idx| graph.edge_weight(idx))
        .filter(|edge| matches!(edge, dc_core::call_graph::CallEdge::Import { .. }))
        .collect();

    // Imports may be processed as Call edges or Import edges
    // Check that graph contains connections between modules
    assert!(graph.edge_count() > 0);

    // Check for calls
    let call_edges: Vec<_> = graph
        .edge_indices()
        .filter_map(|idx| graph.edge_weight(idx))
        .filter(|edge| matches!(edge, dc_core::call_graph::CallEdge::Call { .. }))
        .collect();

    assert!(!call_edges.is_empty());
}

#[test]
fn test_build_graph_with_typescript_schemas() {
    let temp_dir = TempDir::new().unwrap();
    let test_file = temp_dir.path().join("types.ts");

    let source = r#"
interface User {
    name: string;
    age: number;
    email?: string;
}

type UserId = string;

export function getUser(id: UserId): User {
    return { name: "Test", age: 30 } as User;
}
"#;

    std::fs::write(&test_file, source).unwrap();

    let builder = TypeScriptCallGraphBuilder::new(vec![temp_dir.path().to_path_buf()]);
    let graph = builder.build_graph().unwrap();

    // Check that graph is built
    assert!(graph.node_count() > 0);

    // Check for functions
    let function_nodes: Vec<_> = graph
        .node_indices()
        .filter_map(|idx| graph.node_weight(idx))
        .filter(|node| matches!(node, dc_core::call_graph::CallNode::Function { .. }))
        .collect();

    assert!(!function_nodes.is_empty());
}
