use anyhow::Result;
use dc_core::call_graph::{CallGraph, CallNode, Parameter};
use dc_core::models::{NodeId, TypeInfo};
use dc_core::parsers::TypeScriptDecorator;
use std::path::Path;

/// Extractor for route parameters
pub struct ParameterExtractor {
    dto_extractor: Option<crate::dto::DTOExtractor>,
}

impl Default for ParameterExtractor {
    fn default() -> Self {
        Self::new()
    }
}

impl ParameterExtractor {
    /// Creates a new parameter extractor
    pub fn new() -> Self {
        Self {
            dto_extractor: None,
        }
    }

    /// Sets DTO extractor for resolving DTO types
    pub fn with_dto_extractor(mut self, dto_extractor: crate::dto::DTOExtractor) -> Self {
        self.dto_extractor = Some(dto_extractor);
        self
    }

    /// Extracts route parameters and returns request/response types
    pub fn extract_route_parameters(
        &mut self,
        graph: &CallGraph,
        method_node: NodeId,
        _method_decorators: &[&TypeScriptDecorator],
        parameter_decorators: &[&TypeScriptDecorator],
        parameters: &[Parameter],
    ) -> Result<(Option<TypeInfo>, Option<TypeInfo>)> {
        // 1. Find @Body() parameter → request type
        let request_type = if let Some(body_param_idx) =
            self.find_body_parameter(parameters, parameter_decorators)
        {
            if let Some(body_param) = parameters.get(body_param_idx) {
                let mut type_info = body_param.type_info.clone();

                // If type is a DTO class, try to resolve it
                if let Some(schema_ref) = &type_info.schema_ref {
                    if let Some(dto_extractor) = &self.dto_extractor {
                        if let Some(dto_schema) = dto_extractor.get_dto_schema(&schema_ref.name) {
                            // Update type_info with DTO schema reference
                            type_info.schema_ref = Some(dto_schema.clone());
                        }
                    }
                }

                Some(type_info)
            } else {
                None
            }
        } else {
            None
        };

        // 2. Extract return type from method node → response type
        let response_type = self.extract_method_return_type(graph, method_node)?;

        Ok((request_type, response_type))
    }

    /// Finds @Body() parameter
    fn find_body_parameter(
        &self,
        parameters: &[Parameter],
        parameter_decorators: &[&TypeScriptDecorator],
    ) -> Option<usize> {
        // Find parameter with @Body() decorator
        for decorator in parameter_decorators {
            if decorator.name == "Body" {
                if let dc_core::parsers::DecoratorTarget::Parameter { parameter, .. } =
                    &decorator.target
                {
                    // Find parameter index by name
                    return parameters.iter().position(|p| p.name == *parameter);
                }
            }
        }
        None
    }

    /// Extracts return type from method node
    fn extract_method_return_type(
        &self,
        graph: &CallGraph,
        method_node: NodeId,
    ) -> Result<Option<TypeInfo>> {
        if let Some(CallNode::Method { return_type, .. }) = graph.node_weight(method_node.0) {
            Ok(return_type.clone())
        } else {
            Ok(None)
        }
    }

    /// Resolves DTO type
    pub fn resolve_dto_type(
        &self,
        type_info: &TypeInfo,
        _file: &Path,
    ) -> Result<Option<dc_core::models::SchemaReference>> {
        // If type_info.schema_ref points to a class
        if let Some(schema_ref) = &type_info.schema_ref {
            // Check if it's a DTO
            if let Some(dto_extractor) = &self.dto_extractor {
                if let Some(dto_schema) = dto_extractor.get_dto_schema(&schema_ref.name) {
                    return Ok(Some(dto_schema.clone()));
                }
            }
        }
        Ok(None)
    }
}
