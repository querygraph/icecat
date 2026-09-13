use datafusion::{
    arrow::array::{Float64Array, Int64Array, RecordBatch, StringArray},
    prelude::*,
};
use icebug_algorithms::{PageRankOptions, pagerank};
use icebug_core::ExecutionContext;
use icebug_datafusion::{graph_from_frames, register_node_properties, register_result};
use std::sync::Arc;

#[tokio::test]
async fn filter_build_compute_join() -> datafusion::error::Result<()> {
    let ctx = SessionContext::new();
    let nodes = RecordBatch::try_from_iter(vec![
        (
            "node_id",
            Arc::new(Int64Array::from(vec![100, 200, 300])) as _,
        ),
        (
            "label",
            Arc::new(StringArray::from(vec!["a", "b", "isolate"])) as _,
        ),
    ])?;
    let edges = RecordBatch::try_from_iter(vec![
        ("source", Arc::new(Int64Array::from(vec![100, 200])) as _),
        ("target", Arc::new(Int64Array::from(vec![200, 100])) as _),
        ("weight", Arc::new(Float64Array::from(vec![1.0, 2.0])) as _),
    ])?;
    ctx.register_batch("input_nodes", nodes)?;
    ctx.register_batch("input_edges", edges)?;
    let resources = ExecutionContext::default();
    let graph = graph_from_frames(
        ctx.sql("SELECT * FROM input_nodes ORDER BY node_id")
            .await?,
        ctx.sql("SELECT * FROM input_edges WHERE weight > 0")
            .await?,
        Some("weight".into()),
        true,
        resources.clone(),
    )
    .await?
    .prepare_incoming(&resources)
    .unwrap();
    let scores = pagerank(&graph, &PageRankOptions::default(), &resources).unwrap();
    register_result(&ctx, "scores", scores.scores)?;
    register_node_properties(&ctx, "properties", &graph)?;
    let batches = ctx.sql("SELECT p.label, s.score FROM scores s JOIN properties p ON s.node_id=p.__icebug_internal_id ORDER BY p.node_id").await?.collect().await?;
    assert_eq!(batches.iter().map(|b| b.num_rows()).sum::<usize>(), 3);
    let all = datafusion::arrow::compute::concat_batches(&batches[0].schema(), &batches)?;
    let values = all
        .column(1)
        .as_any()
        .downcast_ref::<Float64Array>()
        .unwrap();
    assert!((values.values().iter().sum::<f64>() - 1.0).abs() < 1e-10);
    assert!(values.value(2) < values.value(0));
    Ok(())
}
