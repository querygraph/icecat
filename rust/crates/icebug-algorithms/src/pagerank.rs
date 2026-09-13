use crate::float_result;
#[cfg(feature = "parallel")]
use rayon::ThreadPool as Pool;
#[cfg(not(feature = "parallel"))]
type Pool = ();
use arrow_array::RecordBatch;
use icebug_core::{Error, ExecutionContext, Graph, NodeId, Result, execution::bytes_for};
use std::collections::BTreeMap;

/// Teleport distribution over dense internal vertex IDs.
#[derive(Clone, Debug, Default)]
pub enum Personalization {
    #[default]
    Uniform,
    Dense(Vec<f64>),
    Sparse(Vec<(NodeId, f64)>),
}
/// Canonical probability-mode options; legacy rescaling modes are not silently emulated.
#[derive(Clone, Debug)]
pub struct PageRankOptions {
    pub damping: f64,
    pub tolerance: f64,
    pub max_iterations: usize,
    pub personalization: Personalization,
}
impl Default for PageRankOptions {
    fn default() -> Self {
        Self {
            damping: 0.85,
            tolerance: 1e-8,
            max_iterations: 1000,
            personalization: Personalization::Uniform,
        }
    }
}
/// Scores and explicit L1 stopping information.
#[derive(Debug)]
pub struct PageRankResult {
    pub scores: RecordBatch,
    pub iterations: usize,
    pub residual: f64,
    pub converged: bool,
}

enum Distribution {
    Uniform,
    Sparse(Vec<(usize, f64)>),
    Dense(Vec<f64>),
}
fn distribution(input: &Personalization, n: usize) -> Result<Distribution> {
    let invalid = || {
        Error::InvalidOption(
            "personalization must be finite, nonnegative, in range, and have positive total mass"
                .into(),
        )
    };
    match input {
        Personalization::Uniform => Ok(Distribution::Uniform),
        Personalization::Dense(values) => {
            if values.len() != n || values.iter().any(|w| !w.is_finite() || *w < 0.0) {
                return Err(invalid());
            }
            let scale = values.iter().copied().fold(0.0, f64::max);
            if scale == 0.0 {
                return Err(invalid());
            }
            let sum: f64 = values.iter().map(|w| w / scale).sum();
            Ok(Distribution::Dense(
                values.iter().map(|w| (w / scale) / sum).collect(),
            ))
        }
        Personalization::Sparse(values) => {
            if values
                .iter()
                .any(|(u, w)| u.0 >= n as u64 || !w.is_finite() || *w < 0.0)
            {
                return Err(invalid());
            }
            let scale = values.iter().map(|(_, w)| *w).fold(0.0, f64::max);
            if scale == 0.0 {
                return Err(invalid());
            }
            let mut combined = BTreeMap::new();
            for (u, w) in values {
                *combined.entry(u.0 as usize).or_insert(0.0) += w / scale;
            }
            let sum: f64 = combined.values().sum();
            if !sum.is_finite() {
                return Err(Error::Overflow);
            }
            Ok(Distribution::Sparse(
                combined
                    .into_iter()
                    .filter(|(_, w)| *w > 0.0)
                    .map(|(u, w)| (u, w / sum))
                    .collect(),
            ))
        }
    }
}
fn add_distribution(values: &mut [f64], dist: &Distribution, mass: f64) {
    let n = values.len();
    match dist {
        Distribution::Uniform => {
            for x in values {
                *x += mass / n as f64;
            }
        }
        Distribution::Dense(p) => {
            for (x, p) in values.iter_mut().zip(p) {
                *x += mass * p;
            }
        }
        Distribution::Sparse(p) => {
            for &(u, p) in p {
                values[u] += mass * p;
            }
        }
    }
}

/// Pull PageRank with L1 residual and dangling mass distributed according to personalization.
/// Call `prepare_incoming` explicitly for directed graphs before invoking this function.
pub fn pagerank(
    graph: &Graph,
    options: &PageRankOptions,
    ctx: &ExecutionContext,
) -> Result<PageRankResult> {
    run(graph, options, ctx, None)
}

/// Reusable dedicated CPU pool; sharing an executor avoids per-operation thread creation.
/// Per-vertex sums and final reductions preserve serial ordering for repeatable results.
#[cfg(feature = "parallel")]
pub struct PageRankExecutor(Pool);

#[cfg(feature = "parallel")]
impl PageRankExecutor {
    /// Construct a bounded pool. Zero threads is rejected rather than selecting a global default.
    pub fn new(threads: usize) -> Result<Self> {
        if threads == 0 {
            return Err(Error::InvalidOption("threads must be positive".into()));
        }
        rayon::ThreadPoolBuilder::new()
            .num_threads(threads)
            .build()
            .map(Self)
            .map_err(|e| Error::InvalidOption(format!("cannot create CPU pool: {e}")))
    }
    /// Run parallel pull PageRank with shared scratch reservations and cancellation.
    pub fn run(
        &self,
        graph: &Graph,
        options: &PageRankOptions,
        ctx: &ExecutionContext,
    ) -> Result<PageRankResult> {
        run(graph, options, ctx, Some(&self.0))
    }
}

fn run(
    graph: &Graph,
    options: &PageRankOptions,
    ctx: &ExecutionContext,
    pool: Option<&Pool>,
) -> Result<PageRankResult> {
    #[cfg(not(feature = "parallel"))]
    let _ = pool;
    if !options.damping.is_finite()
        || !(0.0..1.0).contains(&options.damping)
        || !options.tolerance.is_finite()
        || options.tolerance <= 0.0
        || options.max_iterations == 0
    {
        return Err(Error::InvalidOption(
            "damping must be in [0,1), tolerance positive and finite, and max_iterations positive"
                .into(),
        ));
    }
    let n = graph.node_count();
    let extra = match &options.personalization {
        Personalization::Uniform => 0,
        Personalization::Dense(p) => bytes_for(p.len(), 8)?,
        Personalization::Sparse(p) => bytes_for(p.len(), 128)?,
    };
    let _memory = ctx.reserve(
        bytes_for(n, 48)?
            .checked_add(extra)
            .ok_or(Error::Overflow)?,
    )?;
    let dist = distribution(&options.personalization, n)?;
    if n == 0 {
        return Ok(PageRankResult {
            scores: float_result(graph, "score", vec![])?,
            iterations: 0,
            residual: 0.0,
            converged: true,
        });
    }
    let incoming = graph.incoming()?;
    let mut degree = vec![0.0; n];
    for (u, degree) in degree.iter_mut().enumerate() {
        ctx.check()?;
        for (i, (_, w)) in graph.outgoing().edges(NodeId(u as u64))?.enumerate() {
            if i % 4096 == 0 {
                ctx.check()?;
            }
            if w < 0.0 {
                return Err(Error::InvalidOption(
                    "PageRank requires nonnegative weights".into(),
                ));
            }
            *degree += w;
        }
        if !degree.is_finite() {
            return Err(Error::Overflow);
        }
    }
    let mut current = vec![0.0; n];
    add_distribution(&mut current, &dist, 1.0);
    let mut next = vec![0.0; n];
    let mut contribution = vec![0.0; n];
    let mut residual = f64::INFINITY;
    let mut iterations = 0;
    for iteration in 1..=options.max_iterations {
        ctx.check()?;
        let dangling: f64 = current
            .iter()
            .zip(&degree)
            .filter(|(_, d)| **d == 0.0)
            .map(|(p, _)| p)
            .sum();
        for ((value, &score), &degree) in contribution.iter_mut().zip(&current).zip(&degree) {
            *value = if degree > 0.0 { score / degree } else { 0.0 };
        }
        let needs_stable_ratio =
            contribution
                .iter()
                .zip(&current)
                .zip(&degree)
                .any(|((&c, &score), &d)| {
                    !c.is_finite() || (c < f64::MIN_POSITIVE && score > 0.0 && d > 0.0)
                });
        let offsets = incoming.offsets().values();
        let targets = incoming.targets().values();
        let weights = incoming.weights().map(|w| w.values());
        let compute = |(u, next_u): (usize, &mut f64)| -> Result<()> {
            if u % 4096 == 0 {
                ctx.check()?;
            }
            let range = offsets[u] as usize..offsets[u + 1] as usize;
            let mut sum = 0.0;
            if let Some(weights) = weights {
                for (i, (&v, &w)) in targets[range.clone()]
                    .iter()
                    .zip(&weights[range])
                    .enumerate()
                {
                    if i > 0 && i % 4096 == 0 {
                        ctx.check()?;
                    }
                    let v = v as usize;
                    sum += if needs_stable_ratio {
                        if degree[v] > 0.0 {
                            current[v] * (w / degree[v])
                        } else {
                            0.0
                        }
                    } else {
                        contribution[v] * w
                    };
                }
            } else {
                for (i, &v) in targets[range].iter().enumerate() {
                    if i > 0 && i % 4096 == 0 {
                        ctx.check()?;
                    }
                    sum += contribution[v as usize];
                }
            }
            *next_u = sum * options.damping;
            Ok(())
        };
        #[cfg(feature = "parallel")]
        if let Some(pool) = pool {
            use rayon::prelude::*;
            pool.install(|| next.par_iter_mut().enumerate().try_for_each(compute))?;
        } else {
            next.iter_mut().enumerate().try_for_each(compute)?;
        }
        #[cfg(not(feature = "parallel"))]
        next.iter_mut().enumerate().try_for_each(compute)?;
        add_distribution(
            &mut next,
            &dist,
            1.0 - options.damping + options.damping * dangling,
        );
        residual = current.iter().zip(&next).map(|(a, b)| (a - b).abs()).sum();
        if !residual.is_finite() {
            return Err(Error::Overflow);
        }
        std::mem::swap(&mut current, &mut next);
        iterations = iteration;
        if residual <= options.tolerance {
            break;
        }
    }
    ctx.check()?;
    Ok(PageRankResult {
        scores: float_result(graph, "score", current)?,
        iterations,
        residual,
        converged: residual <= options.tolerance,
    })
}
