use arrow_array::{Array, Float64Array, UInt64Array};
use icebug_algorithms::*;
use icebug_core::{Edge, Error, ExecutionContext, NodeId, from_edges};
use proptest::prelude::*;

fn ctx() -> ExecutionContext {
    ExecutionContext::default()
}
fn graph(n: usize, directed: bool, edges: &[(u64, u64, f64)]) -> icebug_core::Graph {
    from_edges(
        n,
        directed,
        true,
        &edges
            .iter()
            .map(|&(source, target, weight)| Edge {
                source,
                target,
                weight,
            })
            .collect::<Vec<_>>(),
    )
    .unwrap()
}
#[test]
fn distances_components_and_isolates() {
    let g = graph(
        5,
        true,
        &[(0, 1, 2.0), (1, 2, 3.0), (2, 0, 1.0), (2, 3, 0.0)],
    )
    .prepare_incoming(&ctx())
    .unwrap();
    let b = bfs(&g, NodeId(0), &ctx()).unwrap();
    let a = b.column(1).as_any().downcast_ref::<Float64Array>().unwrap();
    assert_eq!(
        a.iter().collect::<Vec<_>>(),
        vec![Some(0.0), Some(1.0), Some(2.0), Some(3.0), None]
    );
    let d = dijkstra(&g, NodeId(0), &ctx()).unwrap();
    assert_eq!(
        d.column(1)
            .as_any()
            .downcast_ref::<Float64Array>()
            .unwrap()
            .iter()
            .collect::<Vec<_>>(),
        vec![Some(0.0), Some(2.0), Some(5.0), Some(5.0), None]
    );
    let s = strongly_connected_components(&g, &ctx()).unwrap();
    assert_eq!(
        s.column(1)
            .as_any()
            .downcast_ref::<UInt64Array>()
            .unwrap()
            .values()
            .as_ref(),
        &[0, 0, 0, 3, 4]
    );
    let w = weakly_connected_components(&g, &ctx()).unwrap();
    assert_eq!(
        w.column(1)
            .as_any()
            .downcast_ref::<UInt64Array>()
            .unwrap()
            .values()
            .as_ref(),
        &[0, 0, 0, 0, 4]
    );
    assert!(connected_components(&g, &ctx()).is_err());
}
#[test]
fn pagerank_dangling_and_personalization() {
    let g = graph(3, true, &[(0, 1, 1.0), (1, 2, 0.0)])
        .prepare_incoming(&ctx())
        .unwrap();
    let options = PageRankOptions {
        tolerance: 1e-12,
        personalization: Personalization::Sparse(vec![
            (NodeId(0), 2.0),
            (NodeId(0), 1.0),
            (NodeId(1), 3.0),
        ]),
        ..Default::default()
    };
    let r = pagerank(&g, &options, &ctx()).unwrap();
    assert!(r.converged);
    let x = r
        .scores
        .column(1)
        .as_any()
        .downcast_ref::<Float64Array>()
        .unwrap();
    assert!((x.value(0) - 1.0 / 2.85).abs() < 1e-10);
    assert!((x.value(1) - 1.85 / 2.85).abs() < 1e-10);
    assert_eq!(x.value(2), 0.0);
    let dense = PageRankOptions {
        personalization: Personalization::Dense(vec![3.0, 3.0, 0.0]),
        ..options
    };
    let other = pagerank(&g, &dense, &ctx()).unwrap();
    assert_eq!(r.scores, other.scores);
}
#[test]
fn empty_invalid_options_missing_transpose_and_cancel() {
    let empty = graph(0, true, &[]);
    let r = pagerank(&empty, &Default::default(), &ctx()).unwrap();
    assert_eq!((r.scores.num_rows(), r.iterations), (0, 0));
    let g = graph(2, true, &[(0, 1, 1.0)]);
    assert!(matches!(
        pagerank(&g, &Default::default(), &ctx()),
        Err(Error::MissingIncoming)
    ));
    let prepared = g.prepare_incoming(&ctx()).unwrap();
    for damping in [f64::NAN, 1.0, -1.0, f64::INFINITY] {
        assert!(
            pagerank(
                &prepared,
                &PageRankOptions {
                    damping,
                    ..Default::default()
                },
                &ctx()
            )
            .is_err()
        );
    }
    let options = PageRankOptions {
        max_iterations: 1,
        tolerance: 1e-15,
        ..Default::default()
    };
    assert!(!pagerank(&prepared, &options, &ctx()).unwrap().converged);
    let c = ctx();
    c.cancel();
    assert!(matches!(bfs(&g, NodeId(0), &c), Err(Error::Cancelled)));
    assert!(matches!(
        pagerank(&prepared, &Default::default(), &ExecutionContext::new(1)),
        Err(Error::MemoryLimit { .. })
    ));
    assert!(dijkstra(&graph(2, true, &[(0, 1, -1.0)]), NodeId(0), &ctx()).is_err());
}

proptest! {
    #[test]
    fn random_reachability_matches_floyd_warshall(n in 1usize..12, raw in prop::collection::vec((0u64..100,0u64..100),0..60)) {
        let edges: std::collections::BTreeSet<_>=raw.into_iter().map(|(a,b)|(a%n as u64,b%n as u64)).collect();
        let g=graph(n,true,&edges.iter().map(|&(a,b)|(a,b,1.0)).collect::<Vec<_>>()).prepare_incoming(&ctx()).unwrap();
        let mut distance=vec![vec![f64::INFINITY;n];n];
        for (u,row) in distance.iter_mut().enumerate() { row[u]=0.0; }
        for &(u,v) in &edges { distance[u as usize][v as usize]=distance[u as usize][v as usize].min(1.0); }
        for k in 0..n { for u in 0..n { for v in 0..n { distance[u][v]=distance[u][v].min(distance[u][k]+distance[k][v]); }}}
        let s=strongly_connected_components(&g,&ctx()).unwrap();
        let labels=s.column(1).as_any().downcast_ref::<UInt64Array>().unwrap();
        for (u, distance_u) in distance.iter().enumerate() {
            let output=bfs(&g,NodeId(u as u64),&ctx()).unwrap();
            let got=output.column(1).as_any().downcast_ref::<Float64Array>().unwrap();
            for (v, distance_v) in distance.iter().enumerate() {
                prop_assert_eq!(got.is_null(v),distance_u[v].is_infinite());
                if distance_u[v].is_finite() { prop_assert_eq!(got.value(v),distance_u[v]); }
                prop_assert_eq!(labels.value(u)==labels.value(v),distance_u[v].is_finite()&&distance_v[u].is_finite());
            }
        }
    }
}

#[cfg(feature = "parallel")]
#[test]
fn parallel_matches_serial_and_reuses_pool() {
    let graph = graph(
        5,
        true,
        &[(0, 1, 2.0), (1, 2, 1.0), (2, 1, 1.0), (2, 3, 3.0)],
    )
    .prepare_incoming(&ctx())
    .unwrap();
    let options = PageRankOptions {
        personalization: Personalization::Sparse(vec![(NodeId(0), 3.0), (NodeId(4), 2.0)]),
        ..Default::default()
    };
    let reference = pagerank(&graph, &options, &ctx()).unwrap();
    for threads in [1, 2, 4] {
        let executor = PageRankExecutor::new(threads).unwrap();
        for _ in 0..2 {
            let output = executor.run(&graph, &options, &ctx()).unwrap();
            assert_eq!(output.scores, reference.scores);
            assert_eq!(output.iterations, reference.iterations);
            assert_eq!(output.residual, reference.residual);
        }
    }
    assert!(PageRankExecutor::new(0).is_err());
}

#[test]
fn extreme_weights_keep_probability_mass_and_negative_isolates_fail() {
    for weight in [f64::from_bits(1), 1e-300, 1e300, 1e308] {
        let g = graph(3, true, &[(0, 1, weight), (1, 0, weight)])
            .prepare_incoming(&ctx())
            .unwrap();
        let result = pagerank(&g, &PageRankOptions::default(), &ctx()).unwrap();
        let scores = result
            .scores
            .column(1)
            .as_any()
            .downcast_ref::<Float64Array>()
            .unwrap();
        assert!(result.converged);
        assert!((scores.values().iter().sum::<f64>() - 1.0).abs() < 1e-12);
        assert!((scores.value(0) - scores.value(1)).abs() < 1e-12);
    }
    let g = graph(3, true, &[(1, 2, -1.0)]);
    assert!(g.outgoing().has_negative_weights());
    assert!(matches!(
        dijkstra(&g, NodeId(0), &ctx()),
        Err(Error::InvalidOption(_))
    ));
}
