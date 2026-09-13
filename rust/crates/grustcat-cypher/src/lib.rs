//! A narrow Cypher physical backend for Grustcat's Arrow graph algorithms.
//!
//! Uses the real Grust parser, validates the complete AST, then lowers admitted
//! queries to typed Arrow kernels. This is not Grust's materializing reference
//! executor, nor an implementation of arbitrary Cypher or Neo4j GDS procedures.
mod plan;
use arrow_array::{ArrayRef, Float64Array, RecordBatch, UInt64Array};
use arrow_schema::{Field, Schema};
pub use grust_cypher::CypherParameters;
use grustcat::{
    GrustcatGraph,
    grust::{GrustError, Result, Value},
};
use icebug_core::ExecutionContext;
pub use plan::PreparedQuery;
use std::sync::Arc;

/// Query output and execution diagnostics. Full-path aggregation also retains
/// the O(nodes) distance batch for independent validation, never all paths.
pub struct QueryResult {
    pub rows: RecordBatch,
    pub distances: Option<RecordBatch>,
    pub iterations: usize,
    pub reachable: u64,
}

/// Parse, validate, plan and execute. All four stages belong inside query timing.
pub fn query(
    graph: &GrustcatGraph,
    text: &str,
    params: &CypherParameters,
    ctx: &ExecutionContext,
) -> Result<QueryResult> {
    PreparedQuery::parse(text)?.execute(graph, params, ctx)
}

fn error(message: impl Into<String>) -> GrustError {
    GrustError::CypherExecution(format!("grustcat-cypher: {}", message.into()))
}

fn source(expr: &grust_cypher::ast::Expr, params: &CypherParameters) -> Result<usize> {
    use grust_cypher::ast::Expr;
    let value = match expr {
        Expr::Integer(v) => *v,
        Expr::Parameter(name) => match params.get(name) {
            Some(Value::Int(v)) => *v,
            _ => return Err(error(format!("parameter ${name} must be an integer"))),
        },
        _ => return Err(error("source must be an integer literal or parameter")),
    };
    usize::try_from(value).map_err(|_| error("source must be nonnegative and fit usize"))
}

impl PreparedQuery {
    /// Execute a validated plan against an immutable Arrow projection.
    pub fn execute(
        &self,
        graph: &GrustcatGraph,
        params: &CypherParameters,
        ctx: &ExecutionContext,
    ) -> Result<QueryResult> {
        ctx.check().map_err(|e| error(e.to_string()))?;
        let source = self
            .source
            .as_ref()
            .map(|e| source(e, params))
            .transpose()?;
        let mut iterations = 0;
        if self.algorithm == "fullpaths" {
            let (mut reachable, mut entries, mut nodes) = (0u64, 0u64, 0u64);
            let mut costs = 0.;
            let mut overflow = false;
            let needed = [0, 1, 2].map(|index| self.columns.iter().any(|(i, _)| *i == index));
            let distances = graph.dijkstra_paths(source.unwrap(), ctx, &mut |ids, values| {
                reachable += 1;
                // Fused UNWIND + aggregates: every array element is consumed,
                // without materializing a table with billions of intermediate rows.
                if needed[0] {
                    entries = entries.checked_add(ids.len() as u64).unwrap_or_else(|| {
                        overflow = true;
                        0
                    });
                }
                if needed[1] {
                    for &id in ids {
                        nodes = nodes.checked_add(id).unwrap_or_else(|| {
                            overflow = true;
                            0
                        });
                    }
                }
                if needed[2] {
                    costs += values.iter().sum::<f64>();
                }
            })?;
            if overflow
                || entries > i64::MAX as u64
                || nodes > i64::MAX as u64
                || !costs.is_finite()
            {
                return Err(error("aggregate overflow"));
            }
            let values: Vec<ArrayRef> = vec![
                Arc::new(UInt64Array::from(vec![entries])),
                Arc::new(UInt64Array::from(vec![nodes])),
                Arc::new(Float64Array::from(vec![costs])),
            ];
            let columns: Vec<_> = self
                .columns
                .iter()
                .map(|(index, _)| values[*index].clone())
                .collect();
            let fields: Vec<_> = self
                .columns
                .iter()
                .zip(&columns)
                .map(|((_, name), array)| Field::new(name, array.data_type().clone(), false))
                .collect();
            let rows = RecordBatch::try_new(Arc::new(Schema::new(fields)), columns)
                .map_err(|e| error(e.to_string()))?;
            return Ok(QueryResult {
                rows,
                distances: Some(distances),
                iterations,
                reachable,
            });
        }
        let batch = match self.algorithm.as_str() {
            "bfs" => graph.bfs(source.unwrap(), ctx)?,
            "dijkstra" => graph.dijkstra(source.unwrap(), ctx)?,
            "wcc" => graph.weakly_connected_components(ctx)?,
            "scc" => graph.strongly_connected_components(ctx)?,
            "pagerank" => {
                let result = graph.pagerank(ctx)?;
                if !result.converged {
                    return Err(error("PageRank did not converge"));
                }
                iterations = result.iterations;
                result.scores
            }
            _ => unreachable!("validated plan"),
        };
        let fields: Vec<_> = self
            .columns
            .iter()
            .map(|(i, name)| {
                let f = batch.schema().field(*i).clone();
                Field::new(name, f.data_type().clone(), f.is_nullable())
            })
            .collect();
        let columns = self
            .columns
            .iter()
            .map(|(i, _)| batch.column(*i).clone())
            .collect();
        let rows = RecordBatch::try_new(Arc::new(Schema::new(fields)), columns)
            .map_err(|e| error(e.to_string()))?;
        Ok(QueryResult {
            rows,
            distances: None,
            iterations,
            reachable: 0,
        })
    }
}
