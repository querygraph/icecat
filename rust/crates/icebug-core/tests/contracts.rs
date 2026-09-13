use arrow_array::{Array, Float64Array, Int64Array, RecordBatch, StringArray, UInt64Array};
use icebug_core::{
    CsrAdjacency, Edge, Error, ExecutionContext, Graph, InducedView, NodeId, from_edges,
    from_tables,
};
use proptest::prelude::*;
use std::sync::Arc;

fn csr(n: usize, indices: Vec<u64>, offsets: Vec<u64>) -> icebug_core::Result<CsrAdjacency> {
    CsrAdjacency::try_new(n, indices.into(), offsets.into(), None)
}
#[test]
fn property_selection_preserves_zero_column_row_count_and_checks_bounds() {
    let batch = RecordBatch::try_new_with_options(
        Arc::new(arrow_schema::Schema::empty()),
        vec![],
        &arrow_array::RecordBatchOptions::new().with_row_count(Some(3)),
    )
    .unwrap();
    let selected = icebug_core::builder::take_batch(&batch, &[2, 0]).unwrap();
    assert_eq!(selected.num_rows(), 2);
    assert_eq!(selected.num_columns(), 0);
    assert!(icebug_core::builder::take_batch(&batch, &[3]).is_err());
}
#[test]
fn rejects_invalid_csr() {
    for (n, indices, offsets) in [
        (1, vec![], vec![0]),
        (2, vec![0], vec![0, 2, 1]),
        (1, vec![0], vec![1, 1]),
        (1, vec![1], vec![0, 1]),
        (2, vec![1, 0], vec![0, 2, 2]),
        (2, vec![1, 1], vec![0, 2, 2]),
        (1, vec![0], vec![0, 0]),
    ] {
        assert!(csr(n, indices, offsets).is_err());
    }
    assert!(
        CsrAdjacency::try_new(1, UInt64Array::from(vec![None]), vec![0, 1].into(), None).is_err()
    );
    assert!(
        CsrAdjacency::try_new(
            1,
            vec![0].into(),
            vec![0, 1].into(),
            Some(vec![f64::NAN].into())
        )
        .is_err()
    );
    assert!(
        CsrAdjacency::try_new(
            1,
            vec![0].into(),
            vec![0, 1].into(),
            Some(Float64Array::from(vec![None]))
        )
        .is_err()
    );
}
#[test]
fn arrays_share_sliced_buffers_and_outlive_originals() {
    let indices = UInt64Array::from(vec![99, 1, 0, 99]).slice(1, 2);
    let offsets = UInt64Array::from(vec![99, 0, 1, 2, 99]).slice(1, 3);
    let pointer = indices.values().as_ptr();
    let graph = Graph::try_new(
        false,
        CsrAdjacency::try_new(2, indices, offsets, None).unwrap(),
        None,
    )
    .unwrap();
    assert_eq!(graph.outgoing().targets().values().as_ptr(), pointer);
    assert_eq!(graph.outgoing().neighbors(NodeId(0)).unwrap(), &[1]);
    let clone = graph.clone();
    drop(graph);
    assert!(clone.has_edge(NodeId(1), NodeId(0)).unwrap());
}
#[test]
fn loops_empty_weighted_and_incoming_capabilities() {
    let graph = from_edges(
        2,
        false,
        true,
        &[
            Edge {
                source: 0,
                target: 0,
                weight: 2.0,
            },
            Edge {
                source: 0,
                target: 1,
                weight: 3.0,
            },
        ],
    )
    .unwrap();
    assert_eq!(
        (
            graph.edge_count(),
            graph.self_loop_count(),
            graph.outgoing().slot_count()
        ),
        (2, 1, 3)
    );
    let empty = from_edges(0, true, true, &[]).unwrap();
    assert!(empty.is_weighted());
    assert!(matches!(empty.incoming(), Err(Error::MissingIncoming)));
    let prepared = empty
        .prepare_incoming(&ExecutionContext::default())
        .unwrap();
    assert_eq!(prepared.incoming().unwrap().slot_count(), 0);
    assert!(Graph::try_new(false, csr(2, vec![1], vec![0, 1, 1]).unwrap(), None).is_err());
    assert!(
        Graph::try_new(
            true,
            csr(2, vec![1], vec![0, 1, 1]).unwrap(),
            Some(csr(2, vec![1], vec![0, 1, 1]).unwrap())
        )
        .is_err()
    );
}
#[test]
fn reservations_cancel_and_follow_prepared_graph_lifetime() {
    let graph = from_edges(
        2,
        true,
        false,
        &[Edge {
            source: 0,
            target: 1,
            weight: 1.0,
        }],
    )
    .unwrap();
    let small = ExecutionContext::new(1);
    assert!(matches!(
        graph.prepare_incoming(&small),
        Err(Error::MemoryLimit { .. })
    ));
    assert_eq!(small.reserved_bytes(), 0);
    let ctx = ExecutionContext::new(1000);
    let prepared = graph.prepare_incoming(&ctx).unwrap();
    assert_eq!(ctx.reserved_bytes(), 32);
    let clone = prepared.clone();
    drop(prepared);
    assert_eq!(ctx.reserved_bytes(), 32);
    drop(clone);
    assert_eq!(ctx.reserved_bytes(), 0);
    ctx.cancel();
    assert!(matches!(
        graph.prepare_incoming(&ctx),
        Err(Error::Cancelled)
    ));
}
#[test]
fn table_properties_follow_slots_and_views() {
    let nodes = RecordBatch::try_from_iter(vec![
        (
            "node_id",
            Arc::new(StringArray::from(vec!["b", "a", "isolated"])) as _,
        ),
        ("value", Arc::new(Int64Array::from(vec![20, 10, 0])) as _),
    ])
    .unwrap();
    let edges = RecordBatch::try_from_iter(vec![
        ("source", Arc::new(StringArray::from(vec!["a", "b"])) as _),
        ("target", Arc::new(StringArray::from(vec!["a", "a"])) as _),
        ("weight", Arc::new(Float64Array::from(vec![5.0, 2.0])) as _),
        (
            "tag",
            Arc::new(StringArray::from(vec!["loop", "link"])) as _,
        ),
    ])
    .unwrap();
    let graph = from_tables(
        nodes,
        &[edges],
        Some("weight"),
        false,
        &ExecutionContext::default(),
    )
    .unwrap();
    assert_eq!(graph.slot_edge_ids().unwrap().values().as_ref(), &[1, 1, 0]);
    let view = InducedView::new(graph.clone(), vec![2, 1, 1]).unwrap();
    drop(graph);
    let materialized = view.materialize().unwrap();
    assert_eq!(
        (materialized.node_count(), materialized.edge_count()),
        (2, 1)
    );
    let tag = materialized
        .edge_properties()
        .unwrap()
        .column_by_name("tag")
        .unwrap()
        .as_any()
        .downcast_ref::<StringArray>()
        .unwrap();
    assert_eq!(tag.value(0), "loop");
    assert_eq!(view.neighbors(NodeId(0)).unwrap(), vec![0]);
}

proptest! {
    #[test]
    fn random_graph_transpose_and_induced_oracle(n in 1usize..25, raw in prop::collection::vec((0u64..100, 0u64..100), 0..100), directed in any::<bool>()) {
        let mut pairs = std::collections::BTreeSet::new();
        for (a, b) in raw { let (a,b) = (a % n as u64, b % n as u64); pairs.insert(if directed { (a,b) } else { (a.min(b), a.max(b)) }); }
        let edges: Vec<_> = pairs.iter().map(|&(source,target)| Edge { source,target,weight: 1.0 }).collect();
        let graph = from_edges(n, directed, false, &edges).unwrap().prepare_incoming(&ExecutionContext::default()).unwrap();
        prop_assert_eq!(graph.edge_count(), pairs.len());
        for u in 0..n as u64 { for v in 0..n as u64 {
            let expected = pairs.contains(&(u,v)) || (!directed && pairs.contains(&(v,u)));
            prop_assert_eq!(graph.has_edge(NodeId(u), NodeId(v)).unwrap(), expected);
            prop_assert_eq!(graph.incoming().unwrap().neighbors(NodeId(v)).unwrap().contains(&u), expected);
        }}
        let members: Vec<_> = (0..n as u64).filter(|u| u % 2 == 0).collect();
        let view = InducedView::new(graph, members.clone()).unwrap();
        let compact = view.materialize().unwrap();
        for u in 0..members.len() { prop_assert_eq!(view.neighbors(NodeId(u as u64)).unwrap(), compact.outgoing().neighbors(NodeId(u as u64)).unwrap()); }
    }
}
