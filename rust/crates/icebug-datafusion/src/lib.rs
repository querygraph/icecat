//! Optional relational integration.
//! Queries materialize Arrow batches before CSR construction. This is an in-memory
//! adapter, not an out-of-core graph engine. Include endpoint joins explicitly when
//! filtering the node universe. Graph kernels run outside the async executor.
use datafusion::{
    arrow::{
        array::{RecordBatch, UInt64Array},
        compute::concat_batches,
        datatypes::{DataType, Field, Schema},
    },
    dataframe::DataFrame,
    error::{DataFusionError, Result},
    prelude::SessionContext,
};
use icebug_core::{ExecutionContext, Graph, from_tables};
use std::sync::Arc;

/// Materialize two query plans and construct a graph preserving the node query's row order.
/// Queries must expose `node_id`, `source`, `target` and optional Float64 weights.
pub async fn graph_from_frames(
    nodes: DataFrame,
    edges: DataFrame,
    weight_column: Option<String>,
    directed: bool,
    resources: ExecutionContext,
) -> Result<Graph> {
    resources.check().map_err(external)?;
    let node_schema = Arc::new(nodes.schema().as_arrow().clone());
    let edge_schema = Arc::new(edges.schema().as_arrow().clone());
    let (nodes, mut edges) = tokio::try_join!(nodes.collect(), edges.collect())?;
    resources.check().map_err(external)?;
    let nodes = concat_batches(&node_schema, &nodes)?;
    if edges.is_empty() {
        edges.push(RecordBatch::new_empty(edge_schema));
    }
    tokio::task::spawn_blocking(move || {
        from_tables(
            nodes,
            &edges,
            weight_column.as_deref(),
            directed,
            &resources,
        )
        .map_err(external)
    })
    .await
    .map_err(|e| DataFusionError::Execution(format!("graph construction worker failed: {e}")))?
}
fn external(e: icebug_core::Error) -> DataFusionError {
    DataFusionError::External(Box::new(e))
}

/// Register algorithm result batches for ordinary SQL analysis.
pub fn register_result(ctx: &SessionContext, name: &str, result: RecordBatch) -> Result<()> {
    ctx.register_batch(name, result)?;
    Ok(())
}

/// Register retained node properties with an explicit internal-ID join column.
/// Results use internal `node_id`; join it to `__icebug_internal_id`, not external labels.
pub fn register_node_properties(ctx: &SessionContext, name: &str, graph: &Graph) -> Result<()> {
    let props = graph
        .node_properties()
        .ok_or_else(|| DataFusionError::Execution("graph has no node property table".into()))?;
    if props.column_by_name("__icebug_internal_id").is_some() {
        return Err(DataFusionError::Execution(
            "reserved __icebug_internal_id column already exists".into(),
        ));
    }
    let mut fields = props.schema().fields().to_vec();
    fields.push(Arc::new(Field::new(
        "__icebug_internal_id",
        DataType::UInt64,
        false,
    )));
    let mut columns = props.columns().to_vec();
    columns.push(Arc::new(UInt64Array::from_iter_values(
        0..graph.node_count() as u64,
    )));
    ctx.register_batch(
        name,
        RecordBatch::try_new(Arc::new(Schema::new(fields)), columns)?,
    )?;
    Ok(())
}
