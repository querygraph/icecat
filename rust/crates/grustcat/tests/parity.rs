use arrow_array::{Array, Float64Array, RecordBatch};
use grustcat::*;
use icebug_core::{Edge, ExecutionContext, NodeId, from_edges};
fn compare(a: RecordBatch, b: RecordBatch) {
    if let Some(a) = a.column(1).as_any().downcast_ref::<Float64Array>() {
        let b = b.column(1).as_any().downcast_ref::<Float64Array>().unwrap();
        assert_eq!(a.len(), b.len());
        for i in 0..a.len() {
            assert_eq!(a.is_null(i), b.is_null(i));
            if !a.is_null(i) {
                assert!((a.value(i) - b.value(i)).abs() < 1e-9);
            }
        }
    } else {
        assert_eq!(a.column(1), b.column(1));
    }
}
#[test]
fn cross_backend_ipc_and_all_algorithms() {
    let ctx = ExecutionContext::default();
    for seed in 0..12u64 {
        let mut edges = Vec::new();
        for u in 0..24 {
            for v in 0..24 {
                if (u * 17 + v * 13 + seed * 7) % 19 == 0 {
                    edges.push(Edge {
                        source: u,
                        target: v,
                        weight: 1. + ((u + v) % 7) as f64,
                    });
                }
            }
        }
        let ice = from_edges(25, true, true, &edges)
            .unwrap()
            .prepare_incoming(&ctx)
            .unwrap();
        let tables = arrow_from_icebug(&ice).unwrap();
        let (mut n, mut e) = (Vec::new(), Vec::new());
        tables.write_ipc(&mut n, &mut e).unwrap();
        let loaded =
            ArrowGraph::read_ipc(std::io::Cursor::new(n), std::io::Cursor::new(e)).unwrap();
        let restored = icebug_from_arrow(&loaded, Some("weight"), &ctx).unwrap();
        assert_eq!(
            arrow_from_icebug(&restored).unwrap().to_graph().unwrap(),
            tables.to_graph().unwrap()
        );
        let g = GrustGraph::from_arrow(&loaded, Some("weight")).unwrap();
        for source in [0, 7, 24] {
            compare(
                g.bfs(source, &ctx).unwrap(),
                icebug_algorithms::bfs(&ice, NodeId(source as u64), &ctx).unwrap(),
            );
            compare(
                g.dijkstra(source, &ctx).unwrap(),
                icebug_algorithms::dijkstra(&ice, NodeId(source as u64), &ctx).unwrap(),
            );
        }
        compare(
            g.weakly_connected_components(&ctx).unwrap(),
            icebug_algorithms::weakly_connected_components(&ice, &ctx).unwrap(),
        );
        compare(
            g.strongly_connected_components(&ctx).unwrap(),
            icebug_algorithms::strongly_connected_components(&ice, &ctx).unwrap(),
        );
        let a = g.pagerank(&ctx).unwrap();
        let b = icebug_algorithms::pagerank(&ice, &Default::default(), &ctx).unwrap();
        assert!(a.converged);
        assert_eq!(a.iterations, b.iterations);
        compare(a.scores, b.scores);
    }
}
#[test]
fn negative_and_empty_graphs() {
    let ctx = ExecutionContext::default();
    let g = GrustGraph::new(grust::Graph::default(), None).unwrap();
    assert_eq!(g.pagerank(&ctx).unwrap().scores.num_rows(), 0);
    assert!(g.bfs(0, &ctx).is_err());
    let ice = from_edges(
        3,
        true,
        true,
        &[Edge {
            source: 1,
            target: 2,
            weight: -1.,
        }],
    )
    .unwrap();
    let g = GrustGraph::from_arrow(&arrow_from_icebug(&ice).unwrap(), Some("weight")).unwrap();
    assert!(g.dijkstra(0, &ctx).is_err());
    assert!(g.pagerank(&ctx).is_err());
}

#[test]
fn zero_and_extreme_weights_remain_finite() {
    let ctx = ExecutionContext::default();
    for w in [0., f64::from_bits(1), 1e-300, 1e300] {
        let ice = from_edges(
            3,
            true,
            true,
            &[
                Edge {
                    source: 0,
                    target: 1,
                    weight: w,
                },
                Edge {
                    source: 1,
                    target: 0,
                    weight: w,
                },
            ],
        )
        .unwrap()
        .prepare_incoming(&ctx)
        .unwrap();
        let g = GrustGraph::from_arrow(&arrow_from_icebug(&ice).unwrap(), Some("weight")).unwrap();
        let a = g.pagerank(&ctx).unwrap();
        let b = icebug_algorithms::pagerank(&ice, &Default::default(), &ctx).unwrap();
        assert!(a.converged);
        compare(a.scores, b.scores);
    }
}

#[test]
fn packed_adjacency_preserves_parallel_edges_and_row_identity() {
    let graph = grust::Graph::new(
        ["z", "a", "isolated"]
            .into_iter()
            .map(|id| grust::Node::new("N", id, grust::Props::new()))
            .collect(),
        vec![
            grust::Edge::new(
                "E",
                "a",
                "z",
                grust::Props::from([("weight".into(), grust::Value::Float(4.))]),
            )
            .with_id("reverse"),
            grust::Edge::new(
                "E",
                "z",
                "a",
                grust::Props::from([("weight".into(), grust::Value::Float(7.))]),
            )
            .with_id("expensive"),
            grust::Edge::new(
                "E",
                "z",
                "a",
                grust::Props::from([("weight".into(), grust::Value::Float(2.))]),
            )
            .with_id("cheap"),
        ],
    );
    let g = GrustGraph::new(graph.clone(), Some("weight")).unwrap();
    assert_eq!(g.to_arrow().unwrap().to_graph().unwrap(), graph);
    let ctx = ExecutionContext::default();
    let d = g.dijkstra(0, &ctx).unwrap();
    let d = d.column(1).as_any().downcast_ref::<Float64Array>().unwrap();
    assert_eq!(d.value(0), 0.);
    assert_eq!(d.value(1), 2.);
    assert!(d.is_null(2));
    let bfs = g.bfs(0, &ctx).unwrap();
    let bfs = bfs
        .column(1)
        .as_any()
        .downcast_ref::<Float64Array>()
        .unwrap();
    assert_eq!(bfs.value(1), 1.);
    assert!(bfs.is_null(2));
    for components in [
        g.weakly_connected_components(&ctx),
        g.strongly_connected_components(&ctx),
    ] {
        let batch = components.unwrap();
        let labels = batch
            .column(1)
            .as_any()
            .downcast_ref::<arrow_array::UInt64Array>()
            .unwrap();
        assert_eq!(labels.values().as_ref(), &[0, 0, 2]);
    }
    let scores = g.pagerank(&ctx).unwrap();
    assert!(scores.converged);
    let scores = scores
        .scores
        .column(1)
        .as_any()
        .downcast_ref::<Float64Array>()
        .unwrap();
    assert!((scores.value(0) - scores.value(1)).abs() < 1e-12);
    assert!((scores.values().iter().sum::<f64>() - 1.).abs() < 1e-12);
}

#[test]
fn cancellation_and_overflow_still_fail() {
    let ctx = ExecutionContext::default();
    ctx.cancel();
    let empty = GrustGraph::new(grust::Graph::default(), None).unwrap();
    assert!(empty.pagerank(&ctx).is_err());
    assert!(empty.weakly_connected_components(&ctx).is_err());
    assert!(empty.strongly_connected_components(&ctx).is_err());
    let ice = from_edges(
        3,
        true,
        true,
        &[
            Edge {
                source: 0,
                target: 1,
                weight: 1e308,
            },
            Edge {
                source: 1,
                target: 2,
                weight: 1e308,
            },
        ],
    )
    .unwrap();
    let g = GrustGraph::from_arrow(&arrow_from_icebug(&ice).unwrap(), Some("weight")).unwrap();
    assert!(g.bfs(0, &ctx).is_err());
    assert!(g.dijkstra(0, &ctx).is_err());
    assert!(g.dijkstra(0, &ExecutionContext::default()).is_err());
    assert!(g.dijkstra_paths(0, &ctx, &mut |_, _| {}).is_err());
    assert!(
        g.dijkstra_paths(0, &ExecutionContext::default(), &mut |_, _| {})
            .is_err()
    );
}

#[test]
fn full_paths_have_valid_edges_and_cumulative_costs() {
    let ctx = ExecutionContext::default();
    // Equal-cost alternatives, a zero-cost cycle, and an isolate.
    let edges: Vec<Edge> = [
        (0, 1, 0.),
        (1, 0, 0.),
        (0, 2, 2.),
        (1, 2, 2.),
        (2, 3, 3.),
        (0, 3, 5.),
    ]
    .into_iter()
    .map(|(source, target, weight)| Edge {
        source,
        target,
        weight,
    })
    .collect();
    let ice = from_edges(5, true, true, &edges).unwrap();
    let grust =
        GrustcatGraph::from_arrow(&arrow_from_icecat(&ice).unwrap(), Some("weight")).unwrap();
    let mut all = Vec::new();
    let a = icebug_algorithms::dijkstra_paths(&ice, NodeId(0), &ctx, &mut |nodes, costs| {
        all.push((nodes.to_vec(), costs.to_vec()))
    })
    .unwrap();
    let b = grust
        .dijkstra_paths(0, &ctx, &mut |nodes, costs| {
            all.push((nodes.to_vec(), costs.to_vec()))
        })
        .unwrap();
    compare(a, b);
    assert_eq!(all.len(), 8);
    for (nodes, costs) in all {
        assert_eq!(nodes[0], 0);
        assert_eq!(costs[0], 0.);
        assert_eq!(nodes.len(), costs.len());
        assert_eq!(
            costs.last().copied().unwrap(),
            [0., 0., 2., 5.][*nodes.last().unwrap() as usize]
        );
        assert!(nodes.len() <= 4);
        for (pair, values) in nodes.windows(2).zip(costs.windows(2)) {
            assert!(edges.iter().any(|e| e.source == pair[0]
                && e.target == pair[1]
                && values[0] + e.weight == values[1]));
        }
    }
    assert!(grust.dijkstra_paths(5, &ctx, &mut |_, _| {}).is_err());
    let cancelled = ExecutionContext::default();
    assert!(
        grust
            .dijkstra_paths(0, &cancelled, &mut |_, _| cancelled.cancel())
            .is_err()
    );
    let cancelled = ExecutionContext::default();
    assert!(
        icebug_algorithms::dijkstra_paths(&ice, NodeId(0), &cancelled, &mut |_, _| cancelled
            .cancel())
        .is_err()
    );
}
