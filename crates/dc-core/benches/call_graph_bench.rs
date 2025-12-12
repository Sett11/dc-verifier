use criterion::{black_box, criterion_group, criterion_main, Criterion};
use dc_core::call_graph::{CallGraph, CallNode};
use dc_core::models::NodeId;

fn build_trivial_graph() -> CallGraph {
    let mut graph = CallGraph::default();
    // First create handler node
    let handler = NodeId::from(graph.add_node(CallNode::Function {
        name: "health_check".into(),
        file: std::path::PathBuf::from("app/routes/health.py"),
        line: 1,
        parameters: Vec::new(),
        return_type: None,
    }));
    // Then create Route node with already created handler
    let route = graph.add_node(CallNode::Route {
        path: "/health".into(),
        method: dc_core::call_graph::HttpMethod::Get,
        handler,
        location: dc_core::models::Location {
            file: "app/routes/health.py".into(),
            line: 1,
            column: None,
        },
    });
    // Reverse edge is not required, but we return index
    // so benchmark has something to measure.
    black_box(route);
    graph
}

fn bench_call_graph_building(c: &mut Criterion) {
    c.bench_function("call_graph_trivial_build", |b| {
        b.iter(|| {
            black_box(build_trivial_graph());
        });
    });
}

criterion_group!(benches, bench_call_graph_building);
criterion_main!(benches);
