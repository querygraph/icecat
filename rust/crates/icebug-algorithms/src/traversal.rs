use crate::float_result;
use arrow_array::RecordBatch;
use icebug_core::{Error, ExecutionContext, Graph, NodeId, Result, execution::bytes_for};
use std::{
    cmp::Ordering,
    collections::{BinaryHeap, VecDeque},
};

/// Degree or weighted degree for each vertex. Incoming degree requires a prepared graph.
pub fn degrees(
    graph: &Graph,
    incoming: bool,
    weighted: bool,
    ctx: &ExecutionContext,
) -> Result<RecordBatch> {
    let _memory = ctx.reserve(bytes_for(graph.node_count(), 24)?)?;
    let csr = if incoming {
        graph.incoming()?
    } else {
        graph.outgoing()
    };
    let mut values = Vec::with_capacity(graph.node_count());
    for u in 0..graph.node_count() {
        ctx.check()?;
        let value = if weighted {
            let mut sum = 0.0;
            for (i, (_, weight)) in csr.edges(NodeId(u as u64))?.enumerate() {
                if i % 4096 == 0 {
                    ctx.check()?;
                }
                sum += weight;
            }
            if !sum.is_finite() {
                return Err(Error::Overflow);
            }
            sum
        } else {
            csr.neighbors(NodeId(u as u64))?.len() as f64
        };
        values.push(value);
    }
    float_result(graph, "degree", values)
}

/// Unweighted shortest-path distances. Unreachable vertices have null distance.
pub fn bfs(graph: &Graph, source: NodeId, ctx: &ExecutionContext) -> Result<RecordBatch> {
    graph.outgoing().range(source)?;
    let _memory = ctx.reserve(bytes_for(graph.node_count(), 32)?)?;
    let mut distances = vec![f64::INFINITY; graph.node_count()];
    let mut queue = VecDeque::new();
    distances[source.0 as usize] = 0.0;
    queue.push_back(source.0);
    while let Some(u) = queue.pop_front() {
        ctx.check()?;
        for (i, &v) in graph.outgoing().neighbors(NodeId(u))?.iter().enumerate() {
            if i % 4096 == 0 {
                ctx.check()?;
            }
            if distances[v as usize].is_infinite() {
                distances[v as usize] = distances[u as usize] + 1.0;
                queue.push_back(v);
            }
        }
    }
    float_result(graph, "distance", distances)
}

#[derive(PartialEq)]
struct Entry {
    distance: f64,
    node: u64,
}
impl Eq for Entry {}
impl PartialOrd for Entry {
    fn partial_cmp(&self, rhs: &Self) -> Option<Ordering> {
        Some(self.cmp(rhs))
    }
}
impl Ord for Entry {
    fn cmp(&self, rhs: &Self) -> Ordering {
        rhs.distance
            .total_cmp(&self.distance)
            .then_with(|| rhs.node.cmp(&self.node))
    }
}

/// Weighted shortest distances, rejecting negative weights; unreachable vertices are null.
pub fn dijkstra(graph: &Graph, source: NodeId, ctx: &ExecutionContext) -> Result<RecordBatch> {
    dijkstra_impl(graph, source, ctx, None)
}

/// Visit one shortest path per reachable vertex, including the source.
/// Paths contain source-first node IDs and cumulative costs; ties are arbitrary.
/// Buffers live only for the callback and are reused between paths.
pub fn dijkstra_paths(
    graph: &Graph,
    source: NodeId,
    ctx: &ExecutionContext,
    visitor: &mut dyn FnMut(&[u64], &[f64]),
) -> Result<RecordBatch> {
    dijkstra_impl(graph, source, ctx, Some(visitor))
}

type PathVisitor<'a> = Option<&'a mut dyn FnMut(&[u64], &[f64])>;
fn dijkstra_impl(
    graph: &Graph,
    source: NodeId,
    ctx: &ExecutionContext,
    mut visitor: PathVisitor<'_>,
) -> Result<RecordBatch> {
    graph.outgoing().range(source)?;
    // Lazy decrease-key can retain one heap entry per arc. Charge conservative doubled capacity.
    let heap_bytes = bytes_for(
        graph
            .outgoing()
            .slot_count()
            .checked_add(1)
            .ok_or(Error::Overflow)?,
        32,
    )?;
    let _memory = ctx.reserve(
        bytes_for(graph.node_count(), if visitor.is_some() { 64 } else { 24 })?
            .checked_add(heap_bytes)
            .ok_or(Error::Overflow)?,
    )?;
    if graph.outgoing().has_negative_weights() {
        return Err(Error::InvalidOption(
            "Dijkstra requires nonnegative weights".into(),
        ));
    }
    let mut distances = vec![f64::INFINITY; graph.node_count()];
    let mut parents = visitor.as_ref().map(|_| vec![0u64; graph.node_count()]);
    let mut heap = BinaryHeap::new();
    distances[source.0 as usize] = 0.0;
    heap.push(Entry {
        distance: 0.0,
        node: source.0,
    });
    while let Some(Entry { distance, node }) = heap.pop() {
        ctx.check()?;
        if distance != distances[node as usize] {
            continue;
        }
        for (i, (v, w)) in graph.outgoing().edges(NodeId(node))?.enumerate() {
            if i % 4096 == 0 {
                ctx.check()?;
            }
            let next = distance + w;
            if !next.is_finite() {
                return Err(Error::Overflow);
            }
            if next < distances[v as usize] {
                distances[v as usize] = next;
                if let Some(p) = &mut parents {
                    p[v as usize] = node;
                }
                heap.push(Entry {
                    distance: next,
                    node: v,
                });
            }
        }
    }
    if let Some(visit) = &mut visitor {
        let parents = parents.as_ref().unwrap();
        let mut nodes = Vec::new();
        let mut costs = Vec::new();
        for target in 0..graph.node_count() {
            ctx.check()?;
            if !distances[target].is_finite() {
                continue;
            }
            nodes.clear();
            costs.clear();
            let mut u = target as u64;
            loop {
                if nodes.len() % 4096 == 0 {
                    ctx.check()?;
                }
                nodes.push(u);
                costs.push(distances[u as usize]);
                if u == source.0 {
                    break;
                }
                u = parents[u as usize];
            }
            nodes.reverse();
            costs.reverse();
            visit(&nodes, &costs);
        }
    }
    float_result(graph, "distance", distances)
}
