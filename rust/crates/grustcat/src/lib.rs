//! Grustcat analytics over Arrow adjacency projected from Grust GraphIndex.
use arrow_array::{ArrayRef, Float64Array, RecordBatch, UInt64Array};
use arrow_schema::{Field, Schema};
pub use grust;
pub use grust::arrow::ArrowGraph;
use grust::{Graph, GraphIndex, GrustError, Result, Value};
use icebug_core::ExecutionContext;
use std::{
    cmp::Ordering,
    collections::{BinaryHeap, VecDeque},
    sync::Arc,
};
mod adjacency;
use adjacency::Adjacency;
mod interchange;
pub use interchange::{arrow_from_icecat, icecat_from_arrow};
/// Compatibility aliases for the initial experimental API.
pub use interchange::{
    arrow_from_icecat as arrow_from_icebug, icecat_from_arrow as icebug_from_arrow,
};

/// Immutable Grust model with packed Arrow adjacency projected from GraphIndex.
/// Dense output IDs refer to original node table order.
pub struct GrustcatGraph {
    graph: Graph,
    outgoing: Adjacency,
    incoming: Adjacency,
    negative_weights: bool,
}
impl GrustcatGraph {
    pub fn from_arrow(tables: &ArrowGraph, weight_property: Option<&str>) -> Result<Self> {
        Self::new(tables.to_graph()?, weight_property)
    }
    pub fn new(graph: Graph, weight_property: Option<&str>) -> Result<Self> {
        let index = GraphIndex::new(&graph)?;
        let weights = graph
            .edges
            .iter()
            .map(|e| {
                let w = match weight_property {
                    None => 1.,
                    Some(key) => match e.props.get(key) {
                        Some(Value::Float(w)) => *w,
                        _ => return Err(err("weight must be a Float on every edge")),
                    },
                };
                if !w.is_finite() {
                    return Err(err("nonfinite weight"));
                }
                Ok(w)
            })
            .collect::<Result<Vec<_>>>()?;
        let endpoints = index.edge_endpoints_slice().to_vec();
        drop(index);
        let (outgoing, incoming) = Adjacency::pair(graph.nodes.len(), &endpoints, &weights);
        let negative_weights = weights.iter().any(|w| *w < 0.);
        Ok(Self {
            graph,
            outgoing,
            incoming,
            negative_weights,
        })
    }
    pub fn to_arrow(&self) -> Result<ArrowGraph> {
        ArrowGraph::from_graph(&self.graph)
    }
    pub fn node_count(&self) -> usize {
        self.graph.nodes.len()
    }
    fn check_source(&self, source: usize) -> Result<()> {
        if source >= self.node_count() {
            Err(err("source out of bounds"))
        } else {
            Ok(())
        }
    }
    pub fn bfs(&self, source: usize, ctx: &ExecutionContext) -> Result<RecordBatch> {
        check(ctx)?;
        self.check_source(source)?;
        let mut d = vec![f64::INFINITY; self.node_count()];
        d[source] = 0.;
        // Dense graphs commonly need a broad frontier. Reserve its proven n-node
        // upper bound once; keep narrow graphs on a small circular queue.
        let capacity = if self.outgoing.targets.len() / self.node_count() >= 4 {
            self.node_count()
        } else {
            4.min(self.node_count())
        };
        let mut queue = VecDeque::with_capacity(capacity);
        queue.push_back(source);
        while let Some(u) = queue.pop_front() {
            check(ctx)?;
            let next_distance = d[u] + 1.;
            for (i, &v) in self.outgoing.neighbors(u).iter().enumerate() {
                if i % 4096 == 0 {
                    check(ctx)?;
                }
                let v = v as usize;
                if d[v].is_infinite() {
                    d[v] = next_distance;
                    queue.push_back(v);
                }
            }
        }
        floats("distance", d)
    }
    pub fn dijkstra(&self, source: usize, ctx: &ExecutionContext) -> Result<RecordBatch> {
        self.dijkstra_impl(source, ctx, None)
    }
    /// Visit one source-first shortest path and cumulative costs per reachable node.
    /// Includes the source; ties are arbitrary. Callback buffers are reused.
    pub fn dijkstra_paths(
        &self,
        source: usize,
        ctx: &ExecutionContext,
        visitor: &mut dyn FnMut(&[u64], &[f64]),
    ) -> Result<RecordBatch> {
        self.dijkstra_impl(source, ctx, Some(visitor))
    }
    #[allow(clippy::type_complexity)]
    fn dijkstra_impl(
        &self,
        source: usize,
        ctx: &ExecutionContext,
        mut visitor: Option<&mut dyn FnMut(&[u64], &[f64])>,
    ) -> Result<RecordBatch> {
        check(ctx)?;
        self.check_source(source)?;
        if self.negative_weights {
            return Err(err("negative weight"));
        }
        let mut d = vec![f64::INFINITY; self.node_count()];
        d[source] = 0.;
        let mut parents = visitor.as_ref().map(|_| vec![0usize; self.node_count()]);
        let mut heap = BinaryHeap::from([Entry(0., source)]);
        while let Some(Entry(cost, u)) = heap.pop() {
            check(ctx)?;
            if cost != d[u] {
                continue;
            }
            let range = self.outgoing.range(u);
            for (i, (&v, &w)) in self.outgoing.targets.values()[range.clone()]
                .iter()
                .zip(&self.outgoing.weights.values()[range])
                .enumerate()
            {
                if i % 4096 == 0 {
                    check(ctx)?;
                }
                let v = v as usize;
                let next = cost + w;
                if !next.is_finite() {
                    return Err(err("distance overflow"));
                }
                if next < d[v] {
                    d[v] = next;
                    if let Some(p) = &mut parents {
                        p[v] = u;
                    }
                    heap.push(Entry(next, v));
                }
            }
        }
        if let Some(visit) = &mut visitor {
            let parents = parents.as_ref().unwrap();
            let mut nodes = Vec::new();
            let mut costs = Vec::new();
            for target in 0..self.node_count() {
                check(ctx)?;
                if !d[target].is_finite() {
                    continue;
                }
                nodes.clear();
                costs.clear();
                let mut u = target;
                loop {
                    if nodes.len() % 4096 == 0 {
                        check(ctx)?;
                    }
                    nodes.push(u as u64);
                    costs.push(d[u]);
                    if u == source {
                        break;
                    }
                    u = parents[u];
                }
                nodes.reverse();
                costs.reverse();
                visit(&nodes, &costs);
            }
        }
        floats("distance", d)
    }
    pub fn weakly_connected_components(&self, ctx: &ExecutionContext) -> Result<RecordBatch> {
        check(ctx)?;
        let n = self.node_count();
        let mut parent: Vec<u64> = (0..n as u64).collect();
        for u in 0..n {
            check(ctx)?;
            let mut a = root(&mut parent, u);
            for (i, &v) in self.outgoing.neighbors(u).iter().enumerate() {
                if i % 4096 == 0 {
                    check(ctx)?;
                }
                let b = root(&mut parent, v as usize);
                parent[a.max(b)] = a.min(b) as u64;
                a = a.min(b);
            }
        }
        for u in 0..n {
            parent[u] = root(&mut parent, u) as u64;
        }
        batch("component_id", Arc::new(UInt64Array::from(parent)))
    }
    pub fn strongly_connected_components(&self, ctx: &ExecutionContext) -> Result<RecordBatch> {
        check(ctx)?;
        let n = self.node_count();
        let mut seen = vec![false; n];
        let mut order = Vec::with_capacity(n);
        let mut stack = Vec::with_capacity(n);
        let targets = self.outgoing.targets.values();
        for root in 0..n {
            check(ctx)?;
            if seen[root] {
                continue;
            }
            seen[root] = true;
            let range = self.outgoing.range(root);
            stack.push((root, range.start, range.end));
            while let Some((u, next, end)) = stack.last_mut() {
                check(ctx)?;
                if *next == *end {
                    order.push(*u);
                    stack.pop();
                    continue;
                }
                let v = targets[*next] as usize;
                *next += 1;
                if !seen[v] {
                    seen[v] = true;
                    let range = self.outgoing.range(v);
                    stack.push((v, range.start, range.end));
                }
            }
        }
        let mut labels = vec![u64::MAX; n];
        let mut members = Vec::new();
        let mut work = Vec::new();
        for root in order.into_iter().rev() {
            if labels[root] != u64::MAX {
                continue;
            }
            members.clear();
            work.push(root);
            labels[root] = root as u64;
            let mut minimum = root;
            while let Some(u) = work.pop() {
                check(ctx)?;
                members.push(u);
                minimum = minimum.min(u);
                for (i, &v) in self.incoming.neighbors(u).iter().enumerate() {
                    if i % 4096 == 0 {
                        check(ctx)?;
                    }
                    let v = v as usize;
                    if labels[v] == u64::MAX {
                        labels[v] = root as u64;
                        work.push(v);
                    }
                }
            }
            let label = minimum as u64;
            for &v in &members {
                labels[v] = label;
            }
        }
        batch("component_id", Arc::new(UInt64Array::from(labels)))
    }
    /// Uniform weighted probability PageRank, damping .85, L1 tolerance 1e-8,
    /// at most 1000 iterations, matching Icecat's default parameters.
    pub fn pagerank(&self, ctx: &ExecutionContext) -> Result<PageRankResult> {
        check(ctx)?;
        let n = self.node_count();
        if self.negative_weights {
            return Err(err("negative weight"));
        }
        if n == 0 {
            return Ok(PageRankResult {
                scores: floats("score", vec![])?,
                iterations: 0,
                converged: true,
                residual: 0.,
            });
        }
        let mut degree = vec![0.; n];
        for (u, d) in degree.iter_mut().enumerate() {
            check(ctx)?;
            for (i, &w) in self.outgoing.weights.values()[self.outgoing.range(u)]
                .iter()
                .enumerate()
            {
                if i % 4096 == 0 {
                    check(ctx)?;
                }
                *d += w;
            }
        }
        if degree.iter().any(|w| !w.is_finite()) {
            return Err(err("weighted degree overflow"));
        }
        let mut rank = vec![1. / n as f64; n];
        let mut next = vec![0.; n];
        let mut residual = f64::INFINITY;
        let mut contribution = vec![0.; n];
        for iterations in 1..=1000 {
            check(ctx)?;
            let dangling: f64 = rank
                .iter()
                .zip(&degree)
                .filter(|(_, d)| **d == 0.)
                .map(|(r, _)| *r)
                .sum();
            for ((c, &r), &d) in contribution.iter_mut().zip(&rank).zip(&degree) {
                *c = if d > 0. { r / d } else { 0. };
            }
            let stable_ratio = contribution
                .iter()
                .zip(&rank)
                .zip(&degree)
                .any(|((&c, &r), &d)| {
                    !c.is_finite() || (c < f64::MIN_POSITIVE && r > 0. && d > 0.)
                });
            for (v, out) in next.iter_mut().enumerate() {
                if v % 4096 == 0 {
                    check(ctx)?;
                }
                let mut sum = 0.;
                let range = self.incoming.range(v);
                for (i, (&u, &w)) in self.incoming.targets.values()[range.clone()]
                    .iter()
                    .zip(&self.incoming.weights.values()[range])
                    .enumerate()
                {
                    if i > 0 && i % 4096 == 0 {
                        check(ctx)?;
                    }
                    let u = u as usize;
                    if stable_ratio {
                        if degree[u] > 0. {
                            sum += rank[u] * (w / degree[u]);
                        }
                    } else {
                        sum += contribution[u] * w;
                    }
                }
                *out = (1. - 0.85) / n as f64 + 0.85 * (sum + dangling / n as f64);
            }
            residual = rank.iter().zip(&next).map(|(a, b)| (a - b).abs()).sum();
            std::mem::swap(&mut rank, &mut next);
            if residual <= 1e-8 {
                return Ok(PageRankResult {
                    scores: floats("score", rank)?,
                    iterations,
                    residual,
                    converged: true,
                });
            }
        }
        Ok(PageRankResult {
            scores: floats("score", rank)?,
            iterations: 1000,
            residual,
            converged: false,
        })
    }
}
pub struct PageRankResult {
    pub scores: RecordBatch,
    pub iterations: usize,
    pub residual: f64,
    pub converged: bool,
}
fn err(s: impl ToString) -> GrustError {
    GrustError::Backend(s.to_string())
}
#[inline]
fn check(ctx: &ExecutionContext) -> Result<()> {
    ctx.check().map_err(err)
}
fn floats(name: &str, values: Vec<f64>) -> Result<RecordBatch> {
    let nulls = if values.iter().all(|v| v.is_finite()) {
        None
    } else {
        Some(arrow_buffer::NullBuffer::new(
            arrow_buffer::BooleanBuffer::collect_bool(values.len(), |i| values[i].is_finite()),
        ))
    };
    batch(name, Arc::new(Float64Array::new(values.into(), nulls)))
}
#[inline]
fn root(parent: &mut [u64], mut u: usize) -> usize {
    while parent[u] as usize != u {
        parent[u] = parent[parent[u] as usize];
        u = parent[u] as usize;
    }
    u
}

fn batch(name: &str, a: ArrayRef) -> Result<RecordBatch> {
    let ids: ArrayRef = Arc::new(UInt64Array::from_iter_values(0..a.len() as u64));
    RecordBatch::try_new(
        Arc::new(Schema::new(vec![
            Field::new("node_id", ids.data_type().clone(), false),
            Field::new(name, a.data_type().clone(), true),
        ])),
        vec![ids, a],
    )
    .map_err(err)
}
#[derive(PartialEq)]
struct Entry(f64, usize);
impl Eq for Entry {}
impl Ord for Entry {
    fn cmp(&self, other: &Self) -> Ordering {
        other
            .0
            .total_cmp(&self.0)
            .then_with(|| other.1.cmp(&self.1))
    }
}
impl PartialOrd for Entry {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// Compatibility alias for the initial experimental API.
pub type GrustGraph = GrustcatGraph;
