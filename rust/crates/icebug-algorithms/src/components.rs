use crate::result_batch;
use arrow_array::{RecordBatch, UInt64Array};
use icebug_core::{Error, ExecutionContext, Graph, NodeId, Result, execution::bytes_for};
use std::sync::Arc;

fn root(parent: &mut [u64], mut u: usize) -> usize {
    while parent[u] as usize != u {
        parent[u] = parent[parent[u] as usize];
        u = parent[u] as usize;
    }
    u
}
/// Undirected components, labeled by their smallest internal vertex.
pub fn connected_components(graph: &Graph, ctx: &ExecutionContext) -> Result<RecordBatch> {
    if graph.is_directed() {
        return Err(Error::InvalidOption(
            "use weakly/strongly_connected_components for directed graphs".into(),
        ));
    }
    weakly_connected_components(graph, ctx)
}
/// Weak components from outgoing edges; no transpose is needed.
pub fn weakly_connected_components(graph: &Graph, ctx: &ExecutionContext) -> Result<RecordBatch> {
    let _memory = ctx.reserve(bytes_for(graph.node_count(), 24)?)?;
    let mut parent: Vec<u64> = (0..graph.node_count() as u64).collect();
    for u in 0..graph.node_count() {
        ctx.check()?;
        for (i, &v) in graph
            .outgoing()
            .neighbors(NodeId(u as u64))?
            .iter()
            .enumerate()
        {
            if i % 4096 == 0 {
                ctx.check()?;
            }
            let a = root(&mut parent, u);
            let b = root(&mut parent, v as usize);
            parent[a.max(b)] = a.min(b) as u64;
        }
    }
    for u in 0..parent.len() {
        parent[u] = root(&mut parent, u) as u64;
    }
    result_batch(graph, "component_id", Arc::new(UInt64Array::from(parent)))
}
/// Iterative Kosaraju SCC, labeled by the smallest vertex. Requires incoming adjacency.
pub fn strongly_connected_components(graph: &Graph, ctx: &ExecutionContext) -> Result<RecordBatch> {
    let incoming = graph.incoming()?;
    let n = graph.node_count();
    let _memory = ctx.reserve(bytes_for(n, 80)?)?;
    let mut seen = vec![false; n];
    let mut order = Vec::with_capacity(n);
    let mut stack = Vec::new();
    for start in 0..n {
        ctx.check()?;
        if seen[start] {
            continue;
        }
        seen[start] = true;
        stack.push((start, 0));
        while let Some((u, next)) = stack.last_mut() {
            ctx.check()?;
            let row = graph.outgoing().neighbors(NodeId(*u as u64))?;
            if *next == row.len() {
                order.push(*u);
                stack.pop();
            } else {
                let v = row[*next] as usize;
                *next += 1;
                if !seen[v] {
                    seen[v] = true;
                    stack.push((v, 0));
                }
            }
        }
    }
    let mut labels = vec![u64::MAX; n];
    let mut work = Vec::new();
    let mut members = Vec::new();
    for &start in order.iter().rev() {
        if labels[start] != u64::MAX {
            continue;
        }
        work.push(start);
        labels[start] = start as u64;
        members.clear();
        let mut minimum = start;
        while let Some(u) = work.pop() {
            ctx.check()?;
            members.push(u);
            minimum = minimum.min(u);
            for (i, &v) in incoming.neighbors(NodeId(u as u64))?.iter().enumerate() {
                if i % 4096 == 0 {
                    ctx.check()?;
                }
                let v = v as usize;
                if labels[v] == u64::MAX {
                    labels[v] = start as u64;
                    work.push(v);
                }
            }
        }
        for &u in &members {
            labels[u] = minimum as u64;
        }
    }
    result_batch(graph, "component_id", Arc::new(UInt64Array::from(labels)))
}
