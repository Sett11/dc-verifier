use crate::call_graph::{CallGraph, CallNode, Parameter};
use crate::data_flow::DataFlowTracker;
use crate::models::{
    BaseType, ChainDirection, ChainType, Contract, DataChain, FieldMismatch, Link, LinkType,
    Location, NodeId, PydanticFieldInfo, SchemaReference, SchemaType, Severity, TransformationType,
    TypeInfo, ZodField, ZodUsage,
};
use crate::openapi::OpenAPILinker;
use anyhow::{anyhow, bail, Result};
use petgraph::graph::NodeIndex;
use serde_json;
use std::collections::{HashMap, HashSet};
use std::path::Path;
use tracing::debug;

/// Data chain builder from call graph
pub struct ChainBuilder<'a> {
    /// Call graph
    graph: &'a CallGraph,
    /// Data flow tracker
    #[allow(dead_code)]
    data_flow: &'a DataFlowTracker<'a>,
}

impl<'a> ChainBuilder<'a> {
    /// Creates a new chain builder
    pub fn new(graph: &'a CallGraph, data_flow: &'a DataFlowTracker<'a>) -> Self {
        Self { graph, data_flow }
    }

    /// Builds chain from entry point to end point
    pub fn build_chain(&self, entry: NodeId, direction: ChainDirection) -> Result<DataChain> {
        match direction {
            ChainDirection::FrontendToBackend => self.build_forward_chain(entry),
            ChainDirection::BackendToFrontend => self.build_reverse_chain(entry),
        }
    }

    /// Finds all chains in project
    pub fn find_all_chains(&self) -> Result<Vec<DataChain>> {
        let mut chains = Vec::new();

        // Find all routes (API entry points)
        let routes =
            crate::call_graph::find_nodes(self.graph, |n| matches!(n, CallNode::Route { .. }));

        debug!(
            route_count = routes.len(),
            total_nodes = self.graph.node_count(),
            "Found route nodes in graph"
        );

        for (idx, route) in routes.iter().enumerate() {
            let route_index: NodeIndex<u32> = (*route).into();
            let route_node = self
                .graph
                .node_weight(route_index)
                .ok_or_else(|| anyhow!("Route node not found: {:?}", route))?;

            if let CallNode::Route { path, method, .. } = route_node {
                debug!(
                    route_index = idx + 1,
                    http_method = ?method,
                    route_path = %path,
                    node_index = route.0.index(),
                    "Processing route"
                );
            }

            // Build chain Frontend → Backend → Database
            match self.build_forward_chain(*route) {
                Ok(forward_chain) => {
                    debug!(
                        route_node_index = route.0.index(),
                        link_count = forward_chain.links.len(),
                        "Successfully built forward chain from route"
                    );
                    chains.push(forward_chain);
                }
                Err(e) => {
                    debug!(
                        route_node_index = route.0.index(),
                        error = %e,
                        "Failed to build forward chain from route"
                    );
                }
            }

            // Build chain Database → Backend → Frontend
            match self.build_reverse_chain(*route) {
                Ok(reverse_chain) => {
                    debug!(
                        route_node_index = route.0.index(),
                        link_count = reverse_chain.links.len(),
                        "Successfully built reverse chain from route"
                    );
                    chains.push(reverse_chain);
                }
                Err(e) => {
                    debug!(
                        route_node_index = route.0.index(),
                        error = %e,
                        "Failed to build reverse chain from route"
                    );
                }
            }
        }

        debug!(
            total_chains = chains.len(),
            route_count = routes.len(),
            "Total chains built"
        );

        Ok(chains)
    }

    /// Finds all chains from Zod schemas to Pydantic models
    pub fn find_zod_to_pydantic_chains(
        &self,
        openapi_linker: Option<&OpenAPILinker>,
    ) -> Result<Vec<DataChain>> {
        let mut chains = Vec::new();

        // Find all Zod schema nodes in the graph
        let zod_schemas = crate::call_graph::find_nodes(self.graph, |n| {
            matches!(
                n,
                CallNode::Schema { schema } if schema.schema_type == SchemaType::Zod
            )
        });

        debug!(
            zod_schema_count = zod_schemas.len(),
            "Found Zod schema nodes for Zod → Pydantic chains"
        );

        for zod_node_id in zod_schemas {
            match self.build_zod_to_pydantic_chain(zod_node_id, openapi_linker) {
                Ok(Some(chain)) => {
                    debug!(
                        chain_id = %chain.id,
                        link_count = chain.links.len(),
                        "Built Zod → Pydantic chain"
                    );
                    chains.push(chain);
                }
                Ok(None) => {
                    // No chain for this Zod schema (e.g., no usage or API call), skip
                }
                Err(e) => {
                    debug!(
                        zod_node_index = zod_node_id.index(),
                        error = %e,
                        "Failed to build Zod → Pydantic chain"
                    );
                }
            }
        }

        Ok(chains)
    }

    /// Builds chain Frontend → Backend → Database
    pub fn build_forward_chain(&self, start: NodeId) -> Result<DataChain> {
        self.ensure_node_exists(start)?;
        let path = self.collect_path(start, |node| {
            crate::call_graph::outgoing_nodes(self.graph, node)
        });

        if path.is_empty() {
            bail!("Failed to build forward chain: empty path");
        }

        let links = self.create_links_from_nodes(&path, ChainDirection::FrontendToBackend)?;
        let contracts = self.build_contracts(&links);
        let chain_type = self.determine_chain_type(&links);

        Ok(DataChain {
            id: format!("chain-{}", start.index()),
            name: self.generate_chain_name(start)?,
            links,
            contracts,
            direction: ChainDirection::FrontendToBackend,
            chain_type,
        })
    }

    /// Builds chain Database → Backend → Frontend
    pub fn build_reverse_chain(&self, start: NodeId) -> Result<DataChain> {
        self.ensure_node_exists(start)?;
        let mut path = self.collect_path(start, |node| {
            crate::call_graph::incoming_nodes(self.graph, node)
        });
        if path.is_empty() {
            bail!("Failed to build reverse chain: empty path");
        }
        path.reverse();

        let links = self.create_links_from_nodes(&path, ChainDirection::BackendToFrontend)?;
        let contracts = self.build_contracts(&links);
        let chain_type = self.determine_chain_type(&links);

        Ok(DataChain {
            id: format!("chain-reverse-{}", start.index()),
            name: format!("{} (reverse)", self.generate_chain_name(start)?),
            links,
            contracts,
            direction: ChainDirection::BackendToFrontend,
            chain_type,
        })
    }

    /// Builds data chain from Zod schema to Pydantic model.
    ///
    /// Steps:
    /// 1. Find Zod schema node in graph
    /// 2. Extract ZodUsage from metadata
    /// 3. Find API Route node from ZodUsage.api_call_location
    /// 4. Optionally find OpenAPI endpoint from route via OpenAPILinker
    /// 5. Optionally find Pydantic model from OpenAPI schema
    /// 6. Build chain and compare fields (if Pydantic model is found)
    pub fn build_zod_to_pydantic_chain(
        &self,
        zod_schema_node_id: NodeId,
        openapi_linker: Option<&OpenAPILinker>,
    ) -> Result<Option<DataChain>> {
        // 1. Get Zod schema node
        let zod_node = self
            .graph
            .node_weight(*zod_schema_node_id)
            .ok_or_else(|| anyhow!("Zod schema node not found: {:?}", zod_schema_node_id))?;

        let zod_schema = match zod_node {
            CallNode::Schema { schema } if schema.schema_type == SchemaType::Zod => schema.clone(),
            _ => bail!(
                "Node {:?} is not a Zod schema node",
                zod_schema_node_id.index()
            ),
        };

        // 2. Extract ZodUsage from metadata
        let zod_usages: Vec<ZodUsage> = zod_schema
            .metadata
            .get("usages")
            .and_then(|s| serde_json::from_str(s).ok())
            .unwrap_or_default();

        if zod_usages.is_empty() {
            // No usages for this Zod schema – cannot build chain
            return Ok(None);
        }

        // For now, use the first usage (can be extended to handle multiple)
        let zod_usage = &zod_usages[0];

        // 3. Find API Route node from api_call_location
        let api_call_node_id = if let Some(api_location) = &zod_usage.api_call_location {
            match self.find_route_node_by_location(api_location)? {
                Some(node_id) => node_id,
                None => {
                    // No matching Route node – cannot continue the chain
                    return Ok(None);
                }
            }
        } else {
            // Zod usage is not linked to any API call
            return Ok(None);
        };

        // 4. Optionally find OpenAPI endpoint corresponding to this route
        let openapi_endpoint = if let Some(linker) = openapi_linker {
            let route_node = self
                .graph
                .node_weight(*api_call_node_id)
                .ok_or_else(|| anyhow!("Route node not found for API call"))?;

            if let CallNode::Route { path, method, .. } = route_node {
                linker.match_route_to_endpoint(path, *method)
            } else {
                None
            }
        } else {
            None
        };

        // 5. Optionally find Pydantic model from OpenAPI schema via its name
        let pydantic_node_id = if let Some(endpoint) = openapi_endpoint {
            if let Some(request_schema_name) = &endpoint.request_schema {
                // Try to find Pydantic model node in the same graph by OpenAPI schema name
                self.find_pydantic_model_by_openapi_name(request_schema_name)?
            } else {
                None
            }
        } else {
            None
        };

        // 6. Build chain links
        // This is a FrontendToBackend chain (Zod → API Route → Pydantic)
        let direction = ChainDirection::FrontendToBackend;
        let mut links = Vec::new();

        // Zod schema is the source
        links.push(self.create_link_from_node(zod_schema_node_id, LinkType::Source, direction)?);

        // API Route (client-side call) is a transformer
        links.push(self.create_link_from_node(
            api_call_node_id,
            LinkType::Transformer,
            direction,
        )?);

        // If we have a Pydantic model node, add it as sink and compare fields
        if let Some(pydantic_id) = pydantic_node_id {
            links.push(self.create_link_from_node(pydantic_id, LinkType::Sink, direction)?);

            // Compare fields between Zod and Pydantic and attach mismatches to contracts
            let zod_fields = Self::extract_zod_fields(&zod_schema);

            let pydantic_node = self
                .graph
                .node_weight(*pydantic_id)
                .ok_or_else(|| anyhow!("Pydantic schema node not found"))?;

            if let CallNode::Schema {
                schema: pydantic_schema_ref,
            } = pydantic_node
            {
                let pydantic_fields = Self::extract_pydantic_fields(pydantic_schema_ref);
                let mismatches = Self::compare_zod_pydantic_fields(&zod_fields, &pydantic_fields);

                let contracts = self.create_contracts_with_mismatches(&links, mismatches.clone());

                return Ok(Some(DataChain {
                    id: format!(
                        "zod-{}-to-pydantic-{}",
                        zod_schema.name, pydantic_schema_ref.name
                    ),
                    name: format!(
                        "Zod {} → Pydantic {}",
                        zod_schema.name, pydantic_schema_ref.name
                    ),
                    links,
                    contracts,
                    direction: ChainDirection::FrontendToBackend,
                    chain_type: ChainType::Full,
                }));
            }
        }

        // If we reach here, we have only Zod → API chain (no Pydantic model found)
        let contracts = self.build_contracts(&links);

        Ok(Some(DataChain {
            id: format!("zod-{}-to-api", zod_schema.name),
            name: format!("Zod {} → API", zod_schema.name),
            links,
            contracts,
            direction: ChainDirection::FrontendToBackend,
            chain_type: ChainType::Full,
        }))
    }

    fn ensure_node_exists(&self, node_id: NodeId) -> Result<()> {
        if self.graph.node_weight(*node_id).is_some() {
            Ok(())
        } else {
            bail!("Node {:?} not found in graph", node_id);
        }
    }

    fn collect_path<F>(&self, start: NodeId, get_neighbors: F) -> Vec<NodeId>
    where
        F: Fn(NodeId) -> Vec<NodeId>,
    {
        let mut order = Vec::new();
        let mut current = start;
        let mut visited = HashSet::new();

        loop {
            if visited.contains(&current) {
                break;
            }
            visited.insert(current);
            order.push(current);

            let next = get_neighbors(current)
                .into_iter()
                .find(|candidate| !visited.contains(candidate));

            match next {
                Some(next_node) => current = next_node,
                None => break,
            }
        }

        order
    }

    fn create_links_from_nodes(
        &self,
        nodes: &[NodeId],
        direction: ChainDirection,
    ) -> Result<Vec<Link>> {
        let total = nodes.len();

        // 1. Построить базовые ссылки без учёта трансформаций
        let mut links: Vec<Link> = nodes
            .iter()
            .enumerate()
            .map(|(idx, node_id)| {
                let mut link_type = self.determine_link_type(*node_id);
                // Упрощённая логика без дублирования по направлению:
                // первый элемент цепочки — Source, последний — Sink
                if total == 1 || idx == 0 {
                    link_type = LinkType::Source;
                } else if idx == total - 1 {
                    link_type = LinkType::Sink;
                }
                // В остальных случаях использовать тип из determine_link_type
                self.create_link_from_node(*node_id, link_type, direction)
            })
            .collect::<Result<Vec<_>>>()?;

        // 2. Поднять информацию о трансформациях из рёбер DataFlow
        // Для каждой пары соседних ссылок пытаемся найти CallEdge::DataFlow
        // между соответствующими узлами в графе.
        for i in 0..links.len().saturating_sub(1) {
            let (from_link, to_link) = {
                let (head, tail) = links.split_at_mut(i + 1);
                (&mut head[i], &mut tail[0])
            };

            let from_node = *from_link.node_id;
            let to_node = *to_link.node_id;

            // В зависимости от направления цепочки мы ожидаем DataFlow как
            // from_node -> to_node (прямой поток) или to_node -> from_node (reverse-chain).
            let (primary_from, primary_to, secondary_from, secondary_to) = match direction {
                ChainDirection::FrontendToBackend => (from_node, to_node, to_node, from_node),
                ChainDirection::BackendToFrontend => (to_node, from_node, from_node, to_node),
            };

            // Сначала пробуем поискать ребро в «логичном» направлении потока данных
            let mut found_transformation: Option<TransformationType> = None;

            // Перебираем все рёбра между узлами и ищем первое DataFlow с трансформацией
            for (from_node, to_node) in
                &[(primary_from, primary_to), (secondary_from, secondary_to)]
            {
                if found_transformation.is_some() {
                    break;
                }

                for edge in self.graph.edges_connecting(*from_node, *to_node) {
                    if let crate::call_graph::CallEdge::DataFlow {
                        transformation: Some(t),
                        ..
                    } = edge.weight()
                    {
                        found_transformation = Some(t.clone());
                        break;
                    }
                }
            }

            // Если нашли трансформацию — привязываем её к звену-приёмнику
            if let Some(t) = found_transformation {
                to_link.transformation = Some(t);
            }
        }

        Ok(links)
    }

    fn build_contracts(&self, links: &[Link]) -> Vec<Contract> {
        let mut contracts = Vec::new();
        for window in links.windows(2) {
            if let [from, to] = window {
                contracts.push(self.create_contract(from, to));
            }
        }
        contracts
    }

    fn create_contract(&self, from: &Link, to: &Link) -> Contract {
        let mut contract = Contract {
            from_link_id: from.id.clone(),
            to_link_id: to.id.clone(),
            from_schema: from.schema_ref.clone(),
            to_schema: to.schema_ref.clone(),
            mismatches: Vec::new(),
            severity: Severity::Info,
        };

        // Check for missing schemas and set severity
        let has_missing_schema = contract.from_schema.metadata.contains_key("missing_schema")
            || contract.to_schema.metadata.contains_key("missing_schema");

        if has_missing_schema {
            contract.severity = Severity::Warning;
        }

        // Если на звене-приёмнике есть трансформация, провалидировать её
        if let Some(transformation) = &to.transformation {
            if let Some(transformation_issue) = self.validate_transformation(
                &contract.from_schema,
                &contract.to_schema,
                transformation,
            ) {
                // Любая проблема с трансформацией как минимум Warning
                if contract.severity < Severity::Warning {
                    contract.severity = Severity::Warning;
                }
                contract.mismatches.push(transformation_issue);
            }
        }

        contract
    }

    /// Базовая валидация соответствия типов схем для конкретной трансформации.
    /// Возвращает одну агрегированную проблему или None, если всё ок.
    fn validate_transformation(
        &self,
        from_schema: &SchemaReference,
        to_schema: &SchemaReference,
        transformation: &TransformationType,
    ) -> Option<crate::models::Mismatch> {
        use SchemaType::*;
        use TransformationType::*;

        let (expected_from, expected_to): (Option<SchemaType>, Option<SchemaType>) =
            match transformation {
                FromDict | ValidateData | FromJson | ValidateJson => (None, Some(Pydantic)),
                FromOrm | FromAttributes | OrmToPydantic => (Some(OrmModel), Some(Pydantic)),
                ToDict | ToJson | Serialize => (Some(Pydantic), None),
                PydanticToOrm => (Some(Pydantic), Some(OrmModel)),
            };

        let mut reasons = Vec::new();

        if let Some(expected) = expected_from {
            if from_schema.schema_type != expected {
                reasons.push(format!(
                    "from_schema type mismatch: expected {:?}, got {:?}",
                    expected, from_schema.schema_type
                ));
            }
        }

        if let Some(expected) = expected_to {
            if to_schema.schema_type != expected {
                reasons.push(format!(
                    "to_schema type mismatch: expected {:?}, got {:?}",
                    expected, to_schema.schema_type
                ));
            }
        }

        if reasons.is_empty() {
            return None;
        }

        Some(crate::models::Mismatch {
            mismatch_type: crate::models::MismatchType::TypeMismatch,
            path: "<transformation>".to_string(),
            expected: TypeInfo {
                base_type: BaseType::Unknown,
                schema_ref: Some(SchemaReference {
                    name: format!("{:?}", from_schema.schema_type),
                    schema_type: from_schema.schema_type,
                    location: from_schema.location.clone(),
                    metadata: from_schema.metadata.clone(),
                }),
                constraints: Vec::new(),
                optional: false,
            },
            actual: TypeInfo {
                base_type: BaseType::Unknown,
                schema_ref: Some(SchemaReference {
                    name: format!("{:?}", to_schema.schema_type),
                    schema_type: to_schema.schema_type,
                    location: to_schema.location.clone(),
                    metadata: to_schema.metadata.clone(),
                }),
                constraints: Vec::new(),
                optional: false,
            },
            location: to_schema.location.clone(),
            message: format!(
                "Invalid {:?} transformation between {:?} and {:?}: {}",
                transformation,
                from_schema.schema_type,
                to_schema.schema_type,
                reasons.join("; ")
            ),
            severity_level: crate::models::SeverityLevel::Medium,
        })
    }

    /// Creates contracts between consecutive links and injects field mismatches
    /// into the contract that connects Zod → Pydantic (if present).
    fn create_contracts_with_mismatches(
        &self,
        links: &[Link],
        mismatches: Vec<FieldMismatch>,
    ) -> Vec<Contract> {
        let mut contracts = self.build_contracts(links);

        if links.len() >= 2 && !mismatches.is_empty() {
            let zod_link = &links[0];
            let pydantic_link = &links[links.len() - 1];

            if let Some(contract) = contracts
                .iter_mut()
                .find(|c| c.from_link_id == zod_link.id && c.to_link_id == pydantic_link.id)
            {
                // Raise severity for this contract
                contract.severity = Severity::Warning;

                // Also store field mismatches as JSON in target schema metadata for reporting
                if let Ok(json) = serde_json::to_string(&mismatches) {
                    contract
                        .to_schema
                        .metadata
                        .insert("field_mismatches".to_string(), json);
                }
            }
        }

        contracts
    }

    fn create_link_from_node(
        &self,
        node_id: NodeId,
        link_type: LinkType,
        direction: ChainDirection,
    ) -> Result<Link> {
        let node = self
            .graph
            .node_weight(*node_id)
            .ok_or_else(|| anyhow!("Node not found: {:?}", node_id))?
            .clone();

        let (id, location, schema_ref) = match node {
            CallNode::Route { path, location, .. } => {
                let schema = self.extract_route_schema(node_id, direction)?;
                (
                    format!("route-{}-{}", path.replace('/', "-"), node_id.index()),
                    location,
                    schema,
                )
            }
            CallNode::Function {
                name,
                file,
                line,
                parameters,
                ..
            } => {
                let location = self.location_from_path(&file, line);
                let schema = self.extract_function_schema(&parameters, &name, &location);
                (
                    format!("func-{}-{}", name, node_id.index()),
                    location,
                    schema,
                )
            }
            CallNode::Method {
                name,
                class,
                parameters,
                ..
            } => {
                let (file_path, line) = self.method_location(class)?;
                let location = self.location_from_path(&file_path, line);
                let schema = self.extract_function_schema(&parameters, &name, &location);
                (
                    format!("method-{}-{}", name, node_id.index()),
                    location,
                    schema,
                )
            }
            CallNode::Class { name, file, .. } => {
                let location = self.location_from_path(&file, 0);
                let schema = self.extract_class_schema(&name, &location);
                (
                    format!("class-{}-{}", name, node_id.index()),
                    location,
                    schema,
                )
            }
            CallNode::Module { path } => {
                bail!("Cannot create chain link from module: {:?}", path.display());
            }
            CallNode::Schema { schema } => (
                format!("schema-{}-{}", schema.name, node_id.index()),
                schema.location.clone(),
                schema,
            ),
        };

        Ok(Link {
            id,
            link_type,
            location,
            node_id,
            schema_ref,
            transformation: None,
        })
    }

    /// Extracts Zod fields from SchemaReference metadata
    fn extract_zod_fields(schema_ref: &SchemaReference) -> Vec<ZodField> {
        schema_ref
            .metadata
            .get("fields")
            .and_then(|fields_json| serde_json::from_str::<Vec<ZodField>>(fields_json).ok())
            .unwrap_or_default()
    }

    /// Extracts Pydantic fields from SchemaReference metadata
    fn extract_pydantic_fields(schema_ref: &SchemaReference) -> Vec<PydanticFieldInfo> {
        schema_ref
            .metadata
            .get("fields")
            .and_then(|fields_json| {
                serde_json::from_str::<Vec<PydanticFieldInfo>>(fields_json).ok()
            })
            .unwrap_or_default()
    }

    fn extract_function_schema(
        &self,
        parameters: &[Parameter],
        fallback_name: &str,
        location: &Location,
    ) -> SchemaReference {
        for param in parameters {
            if let Some(schema) = self.schema_from_type_info(&param.type_info) {
                return schema;
            }
        }

        self.unknown_schema(fallback_name, location.clone())
    }

    fn extract_route_schema(
        &self,
        route_node_id: NodeId,
        direction: ChainDirection,
    ) -> Result<SchemaReference> {
        let route_node = self
            .graph
            .node_weight(*route_node_id)
            .ok_or_else(|| anyhow!("Route node not found: {:?}", route_node_id))?;

        if let CallNode::Route {
            request_schema,
            response_schema,
            handler,
            ..
        } = route_node
        {
            // For forward chain (Frontend → Backend), prefer request_schema
            // For reverse chain (Backend → Frontend), prefer response_schema
            match direction {
                ChainDirection::FrontendToBackend => {
                    // Prefer request_schema for forward chain
                    if let Some(ref schema) = request_schema {
                        return Ok(schema.clone());
                    }
                    if let Some(ref schema) = response_schema {
                        return Ok(schema.clone());
                    }
                }
                ChainDirection::BackendToFrontend => {
                    // Prefer response_schema for reverse chain
                    if let Some(ref schema) = response_schema {
                        return Ok(schema.clone());
                    }
                    if let Some(ref schema) = request_schema {
                        return Ok(schema.clone());
                    }
                }
            }

            // Fallback: extract from handler
            if let Some(CallNode::Function {
                name,
                parameters,
                file,
                line,
                ..
            }) = self.graph.node_weight(handler.0).cloned()
            {
                let location = self.location_from_path(&file, line);
                return Ok(self.extract_function_schema(&parameters, &name, &location));
            }
        }

        Ok(self.unknown_schema(
            "RouteRequest",
            Location {
                file: String::new(),
                line: 0,
                column: None,
            },
        ))
    }

    fn extract_class_schema(&self, name: &str, location: &Location) -> SchemaReference {
        // NOTE:
        // Class nodes in the call graph can represent many different things:
        // - ORM / database models (e.g. SQLAlchemy)
        // - Pydantic models
        // - Plain helper classes
        //
        // At this stage we don't have enough context in the ChainBuilder to
        // reliably distinguish Pydantic models from other classes. Pydantic
        // models are discovered and classified earlier in the pipeline and
        // attached to function/method parameters via TypeInfo::schema_ref.
        //
        // To avoid misclassifying non‑Pydantic classes (e.g. SQLAlchemy models)
        // as Pydantic, we treat standalone class nodes here as generic
        // JSON Schema objects. Concrete Pydantic models will still appear
        // in chains through their associated SchemaReference instances.
        SchemaReference {
            name: name.to_string(),
            schema_type: SchemaType::JsonSchema,
            location: location.clone(),
            metadata: HashMap::new(),
        }
    }

    /// Compares Zod fields with Pydantic fields and returns list of mismatches.
    fn compare_zod_pydantic_fields(
        zod_fields: &[ZodField],
        pydantic_fields: &[PydanticFieldInfo],
    ) -> Vec<FieldMismatch> {
        let mut mismatches = Vec::new();

        // Create maps for quick lookup
        let zod_map: HashMap<&str, &ZodField> =
            zod_fields.iter().map(|f| (f.name.as_str(), f)).collect();
        let pydantic_map: HashMap<&str, &PydanticFieldInfo> = pydantic_fields
            .iter()
            .map(|f| (f.name.as_str(), f))
            .collect();

        // Check all Zod fields
        for zod_field in zod_fields {
            if let Some(pydantic_field) = pydantic_map.get(zod_field.name.as_str()) {
                // Check type compatibility
                if !Self::types_compatible_zod_pydantic(
                    &zod_field.type_name,
                    &pydantic_field.type_name,
                ) {
                    mismatches.push(FieldMismatch {
                        field_name: zod_field.name.clone(),
                        zod_type: zod_field.type_name.clone(),
                        pydantic_type: pydantic_field.type_name.clone(),
                        reason: "Type mismatch".to_string(),
                    });
                }

                // Check optionality
                if zod_field.optional != pydantic_field.optional {
                    mismatches.push(FieldMismatch {
                        field_name: zod_field.name.clone(),
                        zod_type: format!(
                            "{} (optional: {})",
                            zod_field.type_name, zod_field.optional
                        ),
                        pydantic_type: format!(
                            "{} (optional: {})",
                            pydantic_field.type_name, pydantic_field.optional
                        ),
                        reason: "Optionality mismatch".to_string(),
                    });
                }
            } else {
                // Field exists in Zod but not in Pydantic
                mismatches.push(FieldMismatch {
                    field_name: zod_field.name.clone(),
                    zod_type: zod_field.type_name.clone(),
                    pydantic_type: "missing".to_string(),
                    reason: "Field missing in Pydantic".to_string(),
                });
            }
        }

        // Fields that exist in Pydantic but not in Zod
        for pydantic_field in pydantic_fields {
            if !zod_map.contains_key(pydantic_field.name.as_str()) {
                mismatches.push(FieldMismatch {
                    field_name: pydantic_field.name.clone(),
                    zod_type: "missing".to_string(),
                    pydantic_type: pydantic_field.type_name.clone(),
                    reason: "Field missing in Zod".to_string(),
                });
            }
        }

        mismatches
    }

    /// Checks if Zod type is compatible with Pydantic type.
    fn types_compatible_zod_pydantic(zod_type: &str, pydantic_type: &str) -> bool {
        let zod_normalized = zod_type.to_lowercase();
        let pydantic_normalized = pydantic_type.to_lowercase();

        match (zod_normalized.as_str(), pydantic_normalized.as_str()) {
            // String types
            ("string", "str") | ("str", "string") => true,

            // Number types
            ("number", "int") | ("int", "number") => true,
            ("number", "float") | ("float", "number") => true,

            // Boolean types
            ("boolean", "bool") | ("bool", "boolean") => true,

            // Array types
            (a, b) if a == "array" && b.starts_with("list[") => true,
            (a, b) if a.starts_with("list[") && b == "array" => true,

            // Object types
            (a, b) if a == "object" && b.starts_with("dict[") => true,
            (a, b) if a.starts_with("dict[") && b == "object" => true,

            // Exact match
            (a, b) if a == b => true,

            _ => false,
        }
    }

    /// Finds Route node by source-code location (file + line).
    fn find_route_node_by_location(&self, location: &Location) -> Result<Option<NodeId>> {
        let routes =
            crate::call_graph::find_nodes(self.graph, |n| matches!(n, CallNode::Route { .. }));

        for route_id in routes {
            let route_node = self
                .graph
                .node_weight(*route_id)
                .ok_or_else(|| anyhow!("Route node not found: {:?}", route_id.index()))?;

            if let CallNode::Route {
                location: route_location,
                ..
            } = route_node
            {
                if route_location.file == location.file && route_location.line == location.line {
                    return Ok(Some(route_id));
                }
            }
        }

        Ok(None)
    }

    /// Finds Pydantic model node by OpenAPI schema name.
    fn find_pydantic_model_by_openapi_name(&self, openapi_name: &str) -> Result<Option<NodeId>> {
        let pydantic_nodes = crate::call_graph::find_nodes(self.graph, |n| {
            matches!(
                n,
                CallNode::Schema { schema } if schema.schema_type == SchemaType::Pydantic
            )
        });

        for node_id in pydantic_nodes {
            let node = self
                .graph
                .node_weight(*node_id)
                .ok_or_else(|| anyhow!("Pydantic node not found: {:?}", node_id.index()))?;

            if let CallNode::Schema { schema } = node {
                // Exact or case-insensitive match
                if schema.name == openapi_name || schema.name.eq_ignore_ascii_case(openapi_name) {
                    return Ok(Some(node_id));
                }

                // Common variants with suffixes
                let variants = [
                    format!("{}Schema", openapi_name),
                    format!("{}Model", openapi_name),
                    format!("{}Request", openapi_name),
                    format!("{}Response", openapi_name),
                    format!("{}DTO", openapi_name),
                ];

                for variant in &variants {
                    if schema.name == *variant || schema.name.eq_ignore_ascii_case(variant) {
                        return Ok(Some(node_id));
                    }
                }
            }
        }

        Ok(None)
    }

    fn method_location(&self, class_node: NodeId) -> Result<(std::path::PathBuf, usize)> {
        let node = self
            .graph
            .node_weight(*class_node)
            .ok_or_else(|| anyhow!("Class for method not found: {:?}", class_node))?;

        if let CallNode::Class { file, .. } = node {
            Ok((file.clone(), 0))
        } else {
            bail!("Node {:?} is not a class", class_node);
        }
    }

    fn schema_from_type_info(&self, type_info: &TypeInfo) -> Option<SchemaReference> {
        if let Some(schema) = &type_info.schema_ref {
            return Some(schema.clone());
        }

        match type_info.base_type {
            BaseType::Object | BaseType::Array => {
                let mut metadata = HashMap::new();
                metadata.insert(
                    "base_type".to_string(),
                    format!("{:?}", type_info.base_type),
                );
                Some(SchemaReference {
                    name: format!("{:?}", type_info.base_type),
                    schema_type: SchemaType::JsonSchema,
                    location: Location {
                        file: String::new(),
                        line: 0,
                        column: None,
                    },
                    metadata,
                })
            }
            _ => None,
        }
    }

    fn unknown_schema(&self, name: &str, location: Location) -> SchemaReference {
        SchemaReference {
            name: name.to_string(),
            schema_type: SchemaType::JsonSchema,
            location,
            metadata: HashMap::new(),
        }
    }

    fn determine_link_type(&self, node_id: NodeId) -> LinkType {
        self.graph
            .node_weight(*node_id)
            .map(|node| match node {
                CallNode::Route { .. } => LinkType::Source,
                CallNode::Class { .. } => LinkType::Sink,
                _ => LinkType::Transformer,
            })
            .unwrap_or(LinkType::Transformer)
    }

    /// Determines the type of chain based on its links
    ///
    /// - Full: Contains Route nodes (API endpoints) - spans multiple layers
    /// - FrontendInternal: All nodes are from TypeScript files (.ts/.tsx)
    /// - BackendInternal: All nodes are from Python files (.py)
    fn determine_chain_type(&self, links: &[Link]) -> ChainType {
        if links.is_empty() {
            return ChainType::Full;
        }

        // Check if chain contains Route nodes (API endpoints)
        let has_route = links.iter().any(|link| {
            self.graph
                .node_weight(*link.node_id)
                .map(|node| matches!(node, CallNode::Route { .. }))
                .unwrap_or(false)
        });

        if has_route {
            return ChainType::Full;
        }

        // Check file extensions to determine if all nodes are from same layer
        let mut has_frontend = false;
        let mut has_backend = false;

        for link in links {
            let file_ext = Path::new(&link.location.file)
                .extension()
                .and_then(|e| e.to_str());

            match file_ext {
                Some("ts") | Some("tsx") => has_frontend = true,
                Some("py") => has_backend = true,
                _ => {}
            }
        }

        // Determine chain type
        if has_frontend && !has_backend {
            ChainType::FrontendInternal
        } else if has_backend && !has_frontend {
            ChainType::BackendInternal
        } else {
            // Mixed or unknown - treat as full chain
            ChainType::Full
        }
    }

    fn location_from_path(&self, path: &Path, line: usize) -> Location {
        Location {
            file: path.to_string_lossy().to_string(),
            line,
            column: None,
        }
    }

    fn generate_chain_name(&self, start: NodeId) -> Result<String> {
        let node = self
            .graph
            .node_weight(*start)
            .ok_or_else(|| anyhow!("Node not found: {:?}", start))?;

        Ok(match node {
            CallNode::Route { path, method, .. } => {
                let method_str = format!("{:?}", method).to_uppercase();
                format!("{} {}", method_str, path)
            }
            CallNode::Function { name, .. } => format!("Function {}", name),
            CallNode::Class { name, .. } => format!("Class {}", name),
            CallNode::Method { name, .. } => format!("Method {}", name),
            CallNode::Module { path } => {
                format!(
                    "Module {}",
                    path.file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or_default()
                )
            }
            CallNode::Schema { schema } => format!("Schema {}", schema.name),
        })
    }
}
