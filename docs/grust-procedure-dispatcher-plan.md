# General graph-algorithm procedures in Grust

## Current state

Grust's read executor in `crates/grust-cypher/src/read.rs` has a closed `procedure_signature` match and a `procedure_rows` match. The latter returns `Vec<Vec<Value>>`. The CALL machinery already evaluates arguments against incoming bindings and handles YIELD projection/filtering. The semantic analyzer checks bindings, but procedure-specific types and output signatures need a shared registry. The existing row executor also materializes intermediate rows; UNWIND clones its input bindings per output row.

Adding another hard-coded procedure is enough for a small proof of concept. It does not establish a general extension API or make full-path analytics bounded in memory. Merely replacing the dispatcher with an iterator is insufficient if downstream operators immediately collect its results.

The separate `grustcat-cypher` crate implements a deliberately narrow physical backend with Grust parsing/semantic analysis. It is useful for measuring typed Arrow execution now, but is not a replacement for the general registry design below.

## Registry and provider contract

Introduce a registry containing immutable procedure definitions and implementation providers. Each definition should declare:

- namespaced name and compatibility/version policy;
- argument names, accepted types, nullability and defaults;
- output columns, types and nullability;
- read/write mode, determinism, correlation support and streaming capabilities;
- relevant resource settings and supported graph/projection requirements.

Compile CALL against this registry and use the same definition for argument checking, YIELD validation, discovery and execution. Duplicate registration, unknown procedures/fields and invalid options must fail before running an algorithm. Preserve catalog and TVF procedures as built-in providers. Do not create a parallel hard-coded signature list.

There are two other integration points beyond the dispatcher: `read_policy.rs` currently gates all CALL clauses through `allow_catalog_procedures`, and `pushdown.rs` recognizes a fixed set of catalog/TVF row sources. Add explicit procedure admission/capability checks without broadening the existing read-policy flag silently. Backend pushdown must advertise supported procedures; unsupported calls should take an explicit planned execution path or fail, never silently change backend or graph scope.

Provider implementations must live outside `grust-cypher`: Grustcat registers Arrow-backed algorithms, and other backends can register their own implementations or explicitly reject unsupported procedures. Avoid dependency cycles. In particular, the parser/executor must not acquire a dependency on Grustcat or Icecat. Arrow should be an optional result adapter unless the project deliberately chooses it as a mandatory executor dependency.

An execution context should carry the graph snapshot or named projection handle, cancellation token, budget accounting, concurrency and procedure options. Bind projections to snapshot identity and validate weight/direction/ID semantics. Reuse a prepared projection within a query/session rather than rebuilding CSR per correlated row. The query planner must define when a CALL is correlated and invoke it once per input row unless safe deduplication is proven.

## Streaming execution

Use a pull-based result cursor or bounded batch producer with a schema. A scalar Value batch can provide compatibility; Arrow RecordBatch can supply the high-throughput path. Define ownership and lifetime rules explicitly, including whether a consumer can retain a batch. Cancellation, LIMIT and errors must close cursors and release resources.

CALL, YIELD, projection and filtering should process batches incrementally. Global aggregates need incremental states; grouped aggregates, DISTINCT, sorting and joins require memory accounting and either spilling or an explicit unsupported/resource-limit outcome. A LIMIT after an aggregate cannot truncate its input. Ordering guarantees must be explicit. Large List arrays need sensible offset widths or bounded batches; a whole chain's full-path output cannot fit in one ordinary Arrow ListArray.

Full-path algorithms should emit one path or bounded groups of paths at a time. The query executor should aggregate node-ID and cumulative-cost arrays without cloning them per UNWIND element. Typed expression execution and fused reductions are optimizations of the general plan, with the reference executor retained as a semantic oracle on small cases.

## Delivery milestones

1. **Extensible CALL with the existing result model.** Registry, signatures, provider context, catalog/TVF migration, and one read-only score procedure. Validate argument evaluation, aliases, filters, correlations, unknown names and existing query compatibility. Explicitly bound materialized outputs.
2. **Arrow/batch streaming through the query pipeline.** Cursor lifecycle, incremental projection/filter/aggregation, cancellation and memory accounting. Verify that execution genuinely consumes lazily; use consumers that stop early and providers that fail mid-stream.
3. **Algorithm provider.** Register BFS, Dijkstra, WCC, SCC and PageRank, then full paths. Document exact graph and numerical semantics. Keep mutation/write-back procedures out of the first milestone.
4. **Evidence and integration.** Compare direct kernels, registry CALL and reference execution on small fixtures. Run large full-path output through bounded-memory aggregation, retain errors/timeouts/unsupported outcomes distinctly, and record both query and algorithm timing boundaries.

The registry is a contained extension. Incremental execution through the existing materializing pipeline is the larger architectural change. This is a source-review scope assessment, not a measured schedule estimate.

## Acceptance tests

Cover typed arguments/defaults, aliases and column order, correlated CALL invocation counts, named-graph isolation, concurrent queries, deterministic cleanup, cancellation during an algorithm and during consumption, invalid weights, isolates, loops/parallel edges, tied shortest paths, arithmetic overflow, batch boundaries and premature consumer termination. Run all existing Grust procedure/TVF tests after the built-ins move into the registry. Test aggregate results across several batch sizes and ensure budgets count both provider and executor allocations.

Substantial upstream changes must also update the Grust book and changelog and rebuild its local book artifacts. Publication remains governed by the repository's explicit publication authorization rules. This note does not change the upstream dispatcher or authorize a release.
