# Grustcat Cypher

A focused Cypher execution backend for the Grustcat Arrow graph. It uses Grust's real lexer/parser and semantic analyzer, validates the entire AST against a documented subset, and compiles it to Grustcat kernels plus typed Arrow projections or fused full-path aggregates. It does not modify Grust's procedure dispatcher, run the materializing reference executor, or implement arbitrary Cypher/GDS compatibility.

```rust
use grustcat_cypher::{query, CypherParameters};
use grustcat::grust::Value;
// Given an immutable graph and an ExecutionContext:
let parameters = CypherParameters::from([("source".into(), Value::Int(0))]);
let result = query(graph,
    "CALL grustcat.dijkstra($source) YIELD node_id, distance RETURN node_id, distance",
    &parameters, ctx)?;
assert_eq!(result.rows.num_columns(), 2);
```

`PreparedQuery::parse` separates compilation from execution for callers that want reusable plans. The benchmark deliberately uses `query`, putting compilation inside its timer. Graph projection/loading is outside the timer. Input and output IPC remain available through the benchmark CLI's existing Arrow support.

## Admitted procedures

| Procedure | Arguments | YIELD fields |
|---|---|---|
| `grustcat.bfs` | integer source literal or parameter | `node_id`, `distance` |
| `grustcat.dijkstra` | integer source literal or parameter | `node_id`, `distance` |
| `grustcat.wcc` | none | `node_id`, `component_id` |
| `grustcat.scc` | none | `node_id`, `component_id` |
| `grustcat.pagerank` | none | `node_id`, `score` |
| `grustcat.fullPaths` | integer source literal or parameter | `nodeIds`, `costs` |

The first five support explicit `YIELD` and variable-only `RETURN`, including aliases, subsets and reordering. Node IDs are dense positions in the loaded Arrow node table, not external string identities. Algorithm direction, weight property and other graph properties come from the immutable Grustcat projection. PageRank uses Grustcat's existing defaults. Distances are nullable Arrow Float64; unreachable nodes remain null.

The full-path procedure currently supports this aggregate shape, with aliases and reordered/subset aggregates:

```cypher
CALL grustcat.fullPaths($source) YIELD nodeIds, costs
UNWIND range(0, size(nodeIds)-1) AS i
RETURN count(*) AS path_entries,
       sum(nodeIds[i]) AS node_sum,
       sum(costs[i]) AS cost_sum
```

Each reachable destination, including the source, produces source-first node and cumulative-cost arrays. The planner fuses `UNWIND` and the aggregates: it consumes the arrays without creating a row object for every entry. Peak path buffering is O(nodes), while total full-path work can be quadratic. Aggregates are returned as a one-row Arrow batch (UInt64 counts/node sums, Float64 cost sum); signed 64-bit aggregate overflow and nonfinite cost sums fail. `QueryResult.distances` retains the O(nodes) distance batch and `reachable` reports a diagnostic count, allowing independent benchmark validation. Equal-cost paths can differ between engines.

The backend rejects `MATCH`, mutation, `UNION`, subqueries, filtering, DISTINCT, sorting, limits, unknown procedures, unknown bindings, arbitrary argument expressions and other RETURN expressions. Rejection is explicit; it never silently dispatches a different algorithm or ignores extra query clauses. This separate API uses Grustcat’s ExecutionContext for cancellation; it does not inherit the reference executor’s ReadQueryPolicy or claim a hard bound on all parsing/kernel allocations. Supporting the general Grust procedure mechanism is a separate architectural task; see [the design note](../../../docs/grust-procedure-dispatcher-plan.md).

## Verification and benchmarking

Tests compare algorithm outputs to direct Grustcat calls and full-path aggregate results to Grust's actual reference Cypher executor on injected path rows. They cover isolates, ties, zero-cost cycles, parameter rebinding, aliases, cancellation, invalid queries, and aggregate overflow.

```sh
cargo test --locked --manifest-path rust/crates/grustcat-cypher/Cargo.toml
cargo clippy --locked --manifest-path rust/crates/grustcat-cypher/Cargo.toml --all-targets -- -D warnings
```

The sibling `adversarial-graph-algorithms` project builds `grustcat-cypher` and includes it in Docker comparisons. Neo4j still runs its official GDS procedure and Cypher aggregation. Both full-path variants consume equivalent path arrays, but query text and execution machinery differ. The new column measures the Grust parser plus an optimized Arrow backend, not the performance of the general Grust reference executor.
