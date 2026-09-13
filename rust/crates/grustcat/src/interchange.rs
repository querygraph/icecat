use super::*;
/// Import the shared directed graph tables into Icecat CSR, retaining all columns.
/// Icecat rejects parallel edges; Grust itself permits them. Weights must be
/// present, non-null Float64 on every edge when selected.
pub fn icecat_from_arrow(
    tables: &ArrowGraph,
    weight_property: Option<&str>,
    ctx: &ExecutionContext,
) -> icebug_core::Result<icebug_core::Graph> {
    let weight = weight_property.map(|s| format!("property.{s}"));
    icebug_core::from_tables(
        tables.nodes().clone(),
        std::slice::from_ref(tables.edges()),
        weight.as_deref(),
        true,
        ctx,
    )
}
/// Export a directed Icecat graph as shared Arrow tables. Imported tables retain
/// IDs/labels/properties. Topology-only graphs receive decimal-string node IDs,
/// default labels and a Float64 `weight` property when weighted.
/// Undirected graphs are rejected: the shared format represents directed edges.
pub fn arrow_from_icecat(graph: &icebug_core::Graph) -> Result<ArrowGraph> {
    if !graph.is_directed() {
        return Err(err("shared Arrow export requires a directed graph"));
    }
    if let (Some(nodes), Some(edges)) = (graph.node_properties(), graph.edge_properties()) {
        return ArrowGraph::try_new(nodes.clone(), edges.clone());
    }
    if graph.node_properties().is_some() || graph.edge_properties().is_some() {
        return Err(err("cannot discard partially attached properties"));
    }
    let nodes = (0..graph.node_count())
        .map(|i| grust::Node {
            id: i.to_string().into(),
            label: "Node".into(),
            props: grust::Props::new(),
        })
        .collect();
    let mut edges = Vec::with_capacity(graph.edge_count());
    for u in 0..graph.node_count() {
        for (v, w) in graph
            .outgoing()
            .edges(icebug_core::NodeId(u as u64))
            .map_err(err)?
        {
            let mut props = grust::Props::new();
            if graph.is_weighted() {
                props.insert("weight".into(), Value::Float(w));
            }
            edges.push(grust::Edge::new(
                "EDGE",
                u.to_string(),
                v.to_string(),
                props,
            ));
        }
    }
    ArrowGraph::from_graph(&Graph::new(nodes, edges))
}
