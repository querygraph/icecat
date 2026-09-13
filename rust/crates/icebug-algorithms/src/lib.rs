//! Native graph algorithms over validated Icebug snapshots.
mod components;
mod pagerank;
mod traversal;

pub use components::{
    connected_components, strongly_connected_components, weakly_connected_components,
};
#[cfg(feature = "parallel")]
pub use pagerank::PageRankExecutor;
pub use pagerank::{PageRankOptions, PageRankResult, Personalization, pagerank};
pub use traversal::{bfs, degrees, dijkstra, dijkstra_paths};

use arrow_array::{ArrayRef, Float64Array, RecordBatch, UInt64Array};
use arrow_schema::{DataType, Field, Schema};
use icebug_core::{Graph, Result};
use std::{collections::HashMap, sync::Arc};

fn result_batch(graph: &Graph, name: &str, values: ArrayRef) -> Result<RecordBatch> {
    let schema = Schema::new_with_metadata(
        vec![
            Field::new("node_id", DataType::UInt64, false),
            Field::new(name, values.data_type().clone(), true),
        ],
        HashMap::from([
            ("icebug.snapshot_id".into(), graph.snapshot_id().to_string()),
            ("icebug.node_id_kind".into(), "internal".into()),
        ]),
    );
    Ok(RecordBatch::try_new(
        Arc::new(schema),
        vec![
            Arc::new(UInt64Array::from_iter_values(0..graph.node_count() as u64)),
            values,
        ],
    )?)
}
fn float_result(graph: &Graph, name: &str, values: Vec<f64>) -> Result<RecordBatch> {
    let nulls = if values.iter().all(|x| x.is_finite()) {
        None
    } else {
        Some(arrow_buffer::NullBuffer::new(
            arrow_buffer::BooleanBuffer::collect_bool(values.len(), |i| values[i].is_finite()),
        ))
    };
    result_batch(
        graph,
        name,
        Arc::new(Float64Array::new(values.into(), nulls)),
    )
}
