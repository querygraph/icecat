//! Reproducible construction/transpose/PageRank timings; not a legacy performance comparison.
use icebug_algorithms::{PageRankOptions, pagerank};
use icebug_core::{Edge, ExecutionContext, from_edges};
use std::time::Instant;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let n = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "100000".into())
        .parse::<usize>()?;
    if n < 3 {
        return Err("benchmark needs at least three vertices".into());
    }
    let start = Instant::now();
    let edges: Vec<_> = (0..n as u64)
        .filter(|u| u % 11 != 0)
        .flat_map(|u| {
            [
                Edge {
                    source: u,
                    target: (u + 1) % n as u64,
                    weight: 1.0,
                },
                Edge {
                    source: u,
                    target: (u + 2) % n as u64,
                    weight: 1.0,
                },
            ]
        })
        .collect();
    let graph = from_edges(n, true, false, &edges)?;
    let construction = start.elapsed();
    drop(edges);
    let resources = ExecutionContext::default();
    let start = Instant::now();
    let graph = graph.prepare_incoming(&resources)?;
    let transpose = start.elapsed();
    let start = Instant::now();
    let result = pagerank(&graph, &PageRankOptions::default(), &resources)?;
    let elapsed = start.elapsed();
    let scores = result
        .scores
        .column(1)
        .as_any()
        .downcast_ref::<arrow_array::Float64Array>()
        .ok_or("invalid result type")?;
    let sum: f64 = scores.values().iter().sum();
    let checksum: f64 = scores
        .values()
        .iter()
        .enumerate()
        .map(|(u, x)| x * (u + 1) as f64)
        .sum();
    println!(
        "nodes,edges,construction_ms,transpose_ms,pagerank_ms,iterations,residual,tracked_peak_bytes,sum,checksum"
    );
    println!(
        "{},{},{:.3},{:.3},{:.3},{},{:.12},{},{:.15},{:.15}",
        n,
        graph.edge_count(),
        construction.as_secs_f64() * 1000.0,
        transpose.as_secs_f64() * 1000.0,
        elapsed.as_secs_f64() * 1000.0,
        result.iterations,
        result.residual,
        resources.peak_bytes(),
        sum,
        checksum
    );
    Ok(())
}
