use crate::call_graph::CallGraph;
use crate::data_flow::{DataPath, Variable};
use crate::models::NodeId;
use std::collections::HashMap;

/// Data flow tracker through call graph
pub struct DataFlowTracker<'a> {
    /// Call graph
    graph: &'a CallGraph,
    /// Variables by nodes
    variables: HashMap<NodeId, Vec<Variable>>,
}

impl<'a> DataFlowTracker<'a> {
    /// Creates a new tracker
    pub fn new(graph: &'a CallGraph) -> Self {
        Self {
            graph,
            variables: HashMap::new(),
        }
    }

    /// Adds a variable to a node
    pub fn add_variable(&mut self, node: NodeId, variable: Variable) {
        self.variables
            .entry(node)
            .or_default()
            .push(variable);
    }

    /// Tracks a variable through the graph
    pub fn track_variable(&self, var: &Variable, from: NodeId) -> Vec<DataPath> {
        let mut paths = Vec::new();

        // Find all nodes that can receive this variable
        let mut visited = std::collections::HashSet::new();
        self.track_variable_recursive(var, from, &mut paths, &mut visited);

        paths
    }

    fn track_variable_recursive(
        &self,
        var: &Variable,
        current: NodeId,
        paths: &mut Vec<DataPath>,
        visited: &mut std::collections::HashSet<NodeId>,
    ) {
        if visited.contains(&current) {
            return;
        }
        visited.insert(current);

        // Go through outgoing edges (calls)
        for neighbor in crate::call_graph::outgoing_nodes(self.graph, current) {
            // Check if variable is used in this node
            if let Some(vars) = self.variables.get(&neighbor) {
                for v in vars {
                    if v.name == var.name {
                        let mut path = DataPath::new(current, neighbor, var.clone());
                        path.push_node(current);
                        paths.push(path);
                    }
                }
            }

            // Recursively continue tracking
            self.track_variable_recursive(var, neighbor, paths, visited);
        }
    }

    /// Tracks function parameter through calls
    pub fn track_parameter(&self, param_name: &str, func: NodeId) -> Vec<DataPath> {
        let mut paths = Vec::new();
        let mut visited = std::collections::HashSet::new();

        // Create variable for parameter
        let param_var = Self::create_param_variable(param_name);

        // Find all nodes that call this function
        let callers = crate::call_graph::incoming_nodes(self.graph, func);

        for caller in callers {
            // Check if parameter is passed through call
            if let Some(edge) = self.graph.edges_connecting(*caller, *func).next() {
                if let crate::call_graph::CallEdge::Call {
                    argument_mapping, ..
                } = edge.weight()
                {
                    // Search for parameter in argument mapping
                    for (param, var_name) in argument_mapping {
                        if param == param_name || var_name == param_name {
                            let mut path = DataPath::new(caller, func, param_var.clone());
                            path.push_node(caller);
                            paths.push(path);

                            // Recursively track further
                            self.track_parameter_recursive(
                                param_name,
                                func,
                                &mut paths,
                                &mut visited,
                            );
                        }
                    }
                }
            }
        }

        paths
    }

    fn track_parameter_recursive(
        &self,
        param_name: &str,
        current: NodeId,
        paths: &mut Vec<DataPath>,
        visited: &mut std::collections::HashSet<NodeId>,
    ) {
        if visited.contains(&current) {
            return;
        }
        visited.insert(current);

        // Go through outgoing edges (calls from this function)
        for neighbor in crate::call_graph::outgoing_nodes(self.graph, current) {
            // Check if parameter is used in call
            if let Some(edge) = self.graph.edges_connecting(*current, *neighbor).next() {
                if let crate::call_graph::CallEdge::Call {
                    argument_mapping, ..
                } = edge.weight()
                {
                    for (_param, var_name) in argument_mapping {
                        if var_name == param_name {
                            let param_var = Self::create_param_variable(param_name);
                            let mut path = DataPath::new(current, neighbor, param_var);
                            path.push_node(current);
                            paths.push(path);
                        }
                    }
                }
            }

            // Recursively continue
            self.track_parameter_recursive(param_name, neighbor, paths, visited);
        }
    }

    /// Tracks return value
    pub fn track_return(&self, func: NodeId) -> Vec<DataPath> {
        let mut paths = Vec::new();
        let mut visited = std::collections::HashSet::new();

        // Create variable for return value
        let return_var = Self::create_return_variable();

        // Find all nodes that call this function
        let callers = crate::call_graph::incoming_nodes(self.graph, func);

        for caller in callers {
            // Create path from function to calling node
            let mut path = DataPath::new(func, caller, return_var.clone());
            path.push_node(func);
            paths.push(path);
        }

        // Also track return value usage further
        self.track_return_recursive(func, &mut paths, &mut visited);

        paths
    }

    fn track_return_recursive(
        &self,
        current: NodeId,
        paths: &mut Vec<DataPath>,
        visited: &mut std::collections::HashSet<NodeId>,
    ) {
        if visited.contains(&current) {
            return;
        }
        visited.insert(current);

        // Go through incoming edges (who calls this function)
        for caller in crate::call_graph::incoming_nodes(self.graph, current) {
            // Go through outgoing edges of calling node (where return value is passed)
            for next_node in crate::call_graph::outgoing_nodes(self.graph, caller) {
                if next_node != current {
                    let return_var = Self::create_return_variable();
                    let mut path = DataPath::new(caller, next_node, return_var);
                    path.push_node(caller);
                    paths.push(path);
                }
            }
        }

        // Recursively continue for all nodes that call this function
        for caller in crate::call_graph::incoming_nodes(self.graph, current) {
            self.track_return_recursive(caller, paths, visited);
        }
    }

    /// Creates variable for parameter
    fn create_param_variable(param_name: &str) -> Variable {
        Variable {
            name: param_name.to_string(),
            type_info: crate::models::TypeInfo {
                base_type: crate::models::BaseType::Unknown,
                schema_ref: None,
                constraints: Vec::new(),
                optional: false,
            },
            location: crate::models::Location {
                file: String::new(),
                line: 0,
                column: None,
            },
            source: crate::data_flow::VariableSource::Parameter,
        }
    }

    /// Creates variable for return value
    fn create_return_variable() -> Variable {
        Variable {
            name: "return".to_string(),
            type_info: crate::models::TypeInfo {
                base_type: crate::models::BaseType::Unknown,
                schema_ref: None,
                constraints: Vec::new(),
                optional: false,
            },
            location: crate::models::Location {
                file: String::new(),
                line: 0,
                column: None,
            },
            source: crate::data_flow::VariableSource::Return,
        }
    }
}
