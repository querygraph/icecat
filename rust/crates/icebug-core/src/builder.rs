//! Deterministic simple-graph construction and Arrow property association.
use crate::{CsrAdjacency, Error, ExecutionContext, Graph, Result};
use arrow_array::{
    Array, ArrayRef, Float64Array, Int64Array, RecordBatch, StringArray, UInt64Array,
};
use arrow_select::{concat::concat_batches, take::take};
use std::{collections::HashMap, sync::Arc};

/// An edge with dense internal endpoints and an optional explicit weight.
#[derive(Clone, Copy, Debug)]
pub struct Edge {
    pub source: u64,
    pub target: u64,
    pub weight: f64,
}

/// Build sorted simple CSR, rejecting duplicate logical edges. Unweighted edges require unit weights.
pub fn from_edges(n: usize, directed: bool, weighted: bool, edges: &[Edge]) -> Result<Graph> {
    build(n, directed, weighted, edges, false).map(|x| x.0)
}

fn build(
    n: usize,
    directed: bool,
    weighted: bool,
    edges: &[Edge],
    need_ids: bool,
) -> Result<(Graph, Option<UInt64Array>)> {
    let mut slots = Vec::new();
    for (id, edge) in edges.iter().enumerate() {
        if edge.source >= n as u64
            || edge.target >= n as u64
            || !edge.weight.is_finite()
            || (!weighted && edge.weight != 1.0)
        {
            return Err(Error::InvalidGraph(
                "edge endpoint or weight is invalid".into(),
            ));
        }
        slots.push((edge.source, edge.target, edge.weight, id as u64));
        if !directed && edge.source != edge.target {
            slots.push((edge.target, edge.source, edge.weight, id as u64));
        }
    }
    slots.sort_by_key(|e| (e.0, e.1));
    if slots
        .windows(2)
        .any(|w| (w[0].0, w[0].1) == (w[1].0, w[1].1))
    {
        return Err(Error::InvalidGraph(
            "duplicate logical edge; normalize explicitly before building".into(),
        ));
    }
    let mut offsets = vec![0u64; n.checked_add(1).ok_or(Error::Overflow)?];
    for slot in &slots {
        offsets[slot.0 as usize + 1] += 1;
    }
    for i in 0..n {
        offsets[i + 1] += offsets[i];
    }
    let targets = UInt64Array::from_iter_values(slots.iter().map(|e| e.1));
    let weights = weighted.then(|| Float64Array::from_iter_values(slots.iter().map(|e| e.2)));
    let ids = need_ids.then(|| UInt64Array::from_iter_values(slots.iter().map(|e| e.3)));
    let csr = CsrAdjacency::try_new(n, targets, offsets.into(), weights)?;
    Ok((Graph::try_new(directed, csr, None)?, ids))
}

#[derive(Hash, Eq, PartialEq)]
enum Key {
    Signed(i64),
    Unsigned(u64),
    Text(String),
}
fn key(array: &dyn Array, i: usize) -> Result<Key> {
    if array.is_null(i) {
        return Err(Error::InvalidGraph("external IDs cannot be null".into()));
    }
    if let Some(a) = array.as_any().downcast_ref::<UInt64Array>() {
        return Ok(Key::Unsigned(a.value(i)));
    }
    if let Some(a) = array.as_any().downcast_ref::<Int64Array>() {
        return Ok(Key::Signed(a.value(i)));
    }
    if let Some(a) = array.as_any().downcast_ref::<StringArray>() {
        return Ok(Key::Text(a.value(i).into()));
    }
    Err(Error::InvalidGraph(
        "external IDs must be Int64, UInt64 or Utf8; decode dictionaries first".into(),
    ))
}
fn column<'a>(batch: &'a RecordBatch, name: &str) -> Result<&'a ArrayRef> {
    batch
        .column_by_name(name)
        .ok_or_else(|| Error::InvalidGraph(format!("missing column {name}")))
}

/// Construct from an explicit node universe and edge batches, preserving every property column.
/// IDs are taken from `node_id`, `source`, and `target`; optional weights must be Float64.
/// All batches are currently materialized in memory; this API does not promise spill or bounded ingestion.
pub fn from_tables(
    nodes: RecordBatch,
    batches: &[RecordBatch],
    weight_column: Option<&str>,
    directed: bool,
    ctx: &ExecutionContext,
) -> Result<Graph> {
    ctx.check()?;
    let node_keys = column(&nodes, "node_id")?;
    let mut mapping = HashMap::new();
    for i in 0..nodes.num_rows() {
        if i % 4096 == 0 {
            ctx.check()?;
        }
        if mapping
            .insert(key(node_keys.as_ref(), i)?, i as u64)
            .is_some()
        {
            return Err(Error::InvalidGraph("duplicate external node ID".into()));
        }
    }
    let schema = batches
        .first()
        .ok_or_else(|| {
            Error::InvalidGraph("provide at least one edge batch (it may have zero rows)".into())
        })?
        .schema();
    if batches.iter().any(|b| b.schema() != schema) {
        return Err(Error::InvalidGraph("edge batch schemas differ".into()));
    }
    let table = concat_batches(&schema, batches)?;
    let source = column(&table, "source")?;
    let target = column(&table, "target")?;
    if source.data_type() != node_keys.data_type() || target.data_type() != node_keys.data_type() {
        return Err(Error::InvalidGraph(
            "endpoint types must match node_id".into(),
        ));
    }
    let weights = weight_column
        .map(|name| -> Result<&Float64Array> {
            column(&table, name)?
                .as_any()
                .downcast_ref::<Float64Array>()
                .ok_or_else(|| Error::InvalidGraph("weight column must be Float64".into()))
        })
        .transpose()?;
    if weights.is_some_and(|w| w.null_count() != 0) {
        return Err(Error::InvalidGraph("weights cannot be null".into()));
    }
    let mut edges = Vec::with_capacity(table.num_rows());
    for i in 0..table.num_rows() {
        if i % 4096 == 0 {
            ctx.check()?;
        }
        let u = mapping
            .get(&key(source.as_ref(), i)?)
            .ok_or_else(|| Error::InvalidGraph("source not in node table".into()))?;
        let v = mapping
            .get(&key(target.as_ref(), i)?)
            .ok_or_else(|| Error::InvalidGraph("target not in node table".into()))?;
        edges.push(Edge {
            source: *u,
            target: *v,
            weight: weights.map_or(1.0, |w| w.value(i)),
        });
    }
    let (graph, ids) = build(nodes.num_rows(), directed, weights.is_some(), &edges, true)?;
    ctx.check()?;
    graph.with_properties(Some(nodes), Some(table), ids)
}

/// Select property rows while preserving schema and metadata.
pub fn take_batch(batch: &RecordBatch, rows: &[u64]) -> Result<RecordBatch> {
    if rows.iter().any(|&row| row >= batch.num_rows() as u64) {
        return Err(Error::InvalidGraph("property row out of bounds".into()));
    }
    let indices = UInt64Array::from(rows.to_vec());
    let columns = batch
        .columns()
        .iter()
        .map(|col| take(col.as_ref(), &indices, None))
        .collect::<std::result::Result<Vec<_>, _>>()?;
    Ok(RecordBatch::try_new_with_options(
        Arc::clone(&batch.schema()),
        columns,
        &arrow_array::RecordBatchOptions::new().with_row_count(Some(rows.len())),
    )?)
}
