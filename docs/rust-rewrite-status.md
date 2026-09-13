# Rust rewrite implementation status

Terminology: Icebug = original Arrow-enabled NetworKit (C++); Icecat = first Rust
rewrite; Grustcat = Grust-compatible Rust rewrite.

Updated 2026-09-13. The experimental implementation is in [rust/](../rust/README.md).
It is a working backend, not completion of the full W00–W16 release program.
The existing `networkit` production implementation remains available independently.

## Confirmed rewrite priority

Grust compatibility is the primary goal. The local Arrow interchange and five
Grust-indexed kernels are an initial subset; complex properties, multi-batch IPC,
a versioned conformance matrix and full cross-backend semantic qualification
remain open. Python and legacy API compatibility follow this contract. See the
[revised roadmap](rust-rewrite-roadmap.md) for the dependency order. This priority
change does not imply that existing implementation or release gates are complete.

## Delivered behavior and remaining gates

| Package | Implemented | Still needed before closing the package |
| --- | --- | --- |
| W00 | Explicit simple-graph, ID, loop, weight, incoming-index and probability PageRank contracts | Full supported legacy API inventory and consumer compatibility sign-off |
| W01 | Independent/randomized Rust checks, Python numerical oracle, restored shadowed test names, runnable C++/Rust PageRank fixtures | Representative real-world corpus, full legacy Python baseline and memory profiles |
| W02 | Five-crate workspace, pinned toolchain and Cargo lock, optional dependencies, native wheel and sdist | Execute all supported platform jobs and establish release dependency policy |
| W03 | Checked Arrow CSR, exact transpose/reciprocal validation, explicit incoming preparation, safe owning snapshots | Extended adversarial/fuzz campaign and large-input qualification |
| W04 | Typed errors, cooperative cancellation, shared atomic reservations and Arrow result identity | Comprehensive builder/FFI/output-lifetime accounting and bounded admission throughout |
| W05 | Python Arrow import/export, explicit copy policy/report, ownership and GC coverage | Full producer/version matrix and asynchronous cancellation qualification |
| W06 | Degree, BFS distances, Dijkstra distances, deterministic CC/WCC/SCC | Legacy traversal facade, predecessor/path APIs and broader reference corpus |
| W07 | Canonical PageRank, dense/sparse personalization, dangling redistribution and explicit convergence | Legacy modes only if accepted separately; broad numerical/performance qualification |
| W08 | Immutable induced selection, compact IDs, explicit property-preserving materialization | Direct generic algorithm traversal over views and cost-model benchmarks |
| W09 | Edge-list and Arrow-table construction, external-ID mapping, properties and isolates | Streaming/spill-aware builder, dictionary normalization and configured duplicate policies |
| W10 | Versioned/checksummed IPC roundtrip and corruption checks; homogeneous topology adapter | Actual icebug-format integration/property fixtures, mmap, crash/durability and version migration tests |
| W11 | Tested DataFusion filter → construct → PageRank → property/result SQL join | Unified resource budgets and bounded streaming ingestion; Python query facade if required |
| W12 | Ordinary Arrow batch registration is sufficient for current workflow | Custom providers remain conditional on measured benefit |
| W13 | Dedicated reusable Rayon PageRank pool with serial-equivalent results | Broader kernel parallelism, degree-skew scheduling, throughput/RSS acceptance gates |
| W14 | Separate `icebug-rust` wheel, documented API, Python compatibility tests, MIT artifacts | Final legacy compatibility facade and executed platform/producer matrix |
| W15 | Legacy backend preserved; additive package and CI workflow | Release qualification, rollout/rollback exercise and publication |
| W16 | No additional algorithm families claimed | Independently accepted community and later algorithm waves |

## Compatibility decisions implemented

| Surface | Rust behavior |
| --- | --- |
| Graph ownership | Immutable shared snapshots; views and exported Arrow buffers retain ownership |
| Directed incoming adjacency | Required explicitly for pull algorithms; missing index returns an error |
| Malformed CSR | Rejected before graph construction, including duplicate/unsorted neighbors and invalid transpose |
| Undirected loops | One logical edge and one slot; other edges use reciprocal slots |
| Weights | Finite Float64; negative values valid in storage but rejected by Dijkstra/PageRank |
| Table IDs | Int64, UInt64 or Utf8 external IDs remapped to dense internal UInt64 IDs |
| Algorithm outputs | Arrow batches with internal IDs; unreachable distances are null |
| PageRank | Probability mode, L1 residual, finite iteration cap, explicit convergence status |
| Components | Deterministic labels equal to minimum internal ID in each component |
| Python API | New `icebug_rust.Graph` methods; no silent replacement of `networkit` |
| Persistence | New version 1 IPC directory format; no claim of legacy file compatibility |

Resource reservations are partial accounting, not a hard memory ceiling. In-memory
table collection and construction are intentionally visible limitations. No claim
of bounded-memory ingestion, zero-copy table construction, or full release parity
should be inferred from passing the current tests.

## Validation recorded locally

Environment: macOS ARM64, Rust 1.97.1, CPython 3.14.6, PyArrow 25.0.1;
Rust Arrow 59.3.0, DataFusion 55.1.0, PyO3 0.29.2. C++ reference uses
Apple Clang 21, Homebrew Arrow 25.0.1 and OpenMP, CMake Release without native flags.

- Core contracts/property tests, algorithms (including randomized reachability and
  component checks), serial/parallel PageRank equivalence, and IPC tests pass.
- The DataFusion integration test passes, including an isolate and external-ID join.
- Workspace Clippy with all targets and the parallel feature passes with warnings denied.
- An installed release wheel, tested outside the checkout, passes 10 Python tests
  and five subtests. Coverage includes Arrow slice ownership, copy policy, validation,
  properties/views, IPC, cancellation, and random PageRank versus an independent
  dense linear-system solution.
- The C++ build succeeds; 42 tests selected by `*PageRank*:*GraphR*` pass. This filter
  also matches some reader names; it is not the full C++ test suite.
- Three duplicate Python test method names were restored to unique names. An AST
  check now catches future shadowing. The restored legacy Python tests were not run.
- Wheel and source distribution are generated locally; an extracted sdist also
  rebuilds a release wheel offline. The sdist's trimmed workspace
  requires lock pruning on rebuild; `--locked` is supported for the original workspace.

CI is configured in [rust.yml](../.github/workflows/rust.yml). Remote matrix success,
full fuzzing/sanitizer campaigns, performance gates, and package publication are not
claimed by this implementation.

## Initial performance measurement

Five sequential runs per executable on the deterministic dangling-vertex fixture
in [benchmarks](../rust/benchmarks/README.md), release builds, one thread:

| Nodes | C++ median kernel ms | Rust median kernel ms | Iterations |
| --- | --- | --- | --- |
| 100,000 | 56.608 | 67.958 | 51 |
| 1,000,000 | 565.118 | 683.008 | 51 |

Score mass and weighted checksum agree at relative tolerance 1e-11. Rust is about
20–21% slower on this fixture. These measurements were collected during local
development, without machine isolation; they do not establish a release performance
gate. The checked CSR baseline is ready for profiling; optimization and testing on
realistic degree distributions remain open. The reusable comparison script alternates
executables to reduce run-order bias in subsequent measurements.

## Performance follow-up, 2026-09-13

A standalone, reproducible comparison now lives at
[adversarial-graph-algorithms](../../adversarial-graph-algorithms/README.md).
It compares five algorithms on six synthetic families at two sizes, validates every
output vector, and preserves baseline and optimized samples. This supersedes the
single-fixture result above as the primary local performance evidence.

The 60-case optimized matrix passes the declared local gate (Rust median no more
than the greater of 1.10 times C++ or C++ plus 0.1 ms). Rust wins 55 cases outright in the final run.
Geometric-mean Rust/C++ time ratios: BFS 0.703, Dijkstra 1.009, WCC 0.550,
SCC 0.495, PageRank 0.811. The tiny disconnected-search cases retain measurable
Arrow output overhead; passing the absolute allowance does not imply parity there.

Changes: PageRank reuses per-source contributions and Arrow slices with a stable
fallback for extreme weights; floating results adopt existing buffers and build
only the required validity bitmap; immutable CSR caches negative-weight presence
so Dijkstra no longer scans every edge before a localized query. No unsafe code or
C++ production changes were introduced. Numerical, parallel-equivalence, extreme
weight, and installed Python wheel tests pass. The complete release gates in the
work-package table remain open where indicated.

The extended 262,144-node holdout validates all 30 cases and Rust is faster in 28.
The two larger clustered BFS/Dijkstra cases fail the same timing gate because of
cold Arrow result construction (~0.31–0.36 ms versus ~0.05 ms for C++ positional
vectors). The local initial-matrix goal has passed; these holdout exceptions remain
explicit limitations. See the [full benchmark report](../../adversarial-graph-algorithms/RESULTS.md).

A subsequent [official Neo4j GDS comparison](../../adversarial-graph-algorithms/NEO4J-RESULTS.md)
ran the initial matrix against Neo4j 2026.08.0 / GDS 2026.08.1: 59 completed,
validated cases and one bounded Dijkstra path-query timeout. Its report distinguishes
GDS compute time from lazy full-path query work and documents PageRank normalization
and different stopping criteria; these figures are not a pure language comparison.

## Local Grust variant (2026-09-13)

`rust/crates/grustcat` is a separate experimental workspace using the sibling
Grust checkout. It adds GraphIndex-based BFS/Dijkstra/WCC/SCC/default PageRank
and direct two-table Arrow IPC interchange with Icebug. This is not full API
parity, a Python backend, or a released package. Native scalar properties only;
see its README for error behavior and IO/allocation limits.

## Grustcat performance qualification (2026-09-13)

Terminology is now explicit: Icebug is the original C++ Arrow update of
NetworKit; Icecat is the first Rust rewrite; Grustcat is the Grust-compatible
rewrite in `rust/crates/grustcat`. Internal Icecat crate/Python names remain
compatible; the benchmark executables are `icecat` and `grustcat`.

Grustcat projects validated Grust endpoints into packed Arrow adjacency after
releasing the temporary Grust index. Optimizations include aligned weights,
vector ownership transfer into results, cached negative-weight validation,
PageRank contributions with an extreme-weight fallback, union-find root reuse,
BFS queue capacity planning and SCC frames with precomputed edge ranges.

All 60 main cases (16,384/65,536 nodes) and 30 holdout cases (262,144 nodes)
passed full result checks and the declared kernel-time gate:
`Grustcat <= max(1.10 * Icecat, Icecat + 0.1 ms)`. On the Apple M1 Max, main
geometric-mean Grustcat/Icecat ratios were BFS .845, Dijkstra .892, WCC .829,
SCC .898 and PageRank .840. Relative to the preserved original Grustcat binary,
speedups range from 1.19x to 2.58x. Five native conformance tests (including
randomized oracle comparisons), Clippy and both-direction Arrow IPC checks pass.

This qualifies the measured single-threaded kernels, not ingestion time, whole-
process memory, full Grust compatibility or the entire release program. Graph
construction/projection and IO are excluded from timers. See
`~/src/adversarial-graph-algorithms/GRUSTCAT-RESULTS.md` and its raw measurements.


## Docker and equivalent full paths (2026-09-13)

Icecat and Grustcat now expose `dijkstra_paths` visitors returning source-first node IDs and cumulative costs for every reachable target. Their Arrow distance results remain available. The Docker benchmark adds Icebug's official stored-path Dijkstra and unmodified Neo4j GDS to the same full-path output contract, with all reconstruction and consumption timed. Full-path mode removes both the native process timeout and Neo4j transaction deadline.

All 30 small Docker cases passed. The 65,536-node chain completed and validated for all four engines, including 2,147,516,416 entries in each path array: Icebug 22.264 s, Icecat 16.816 s, Grustcat 16.761 s, GDS 200.919 s. These are one-sample Linux VM completion measurements; GDS includes Cypher overhead. See [results and limitations](../../adversarial-graph-algorithms/DOCKER-FULL-PATH-RESULTS.md) and [Docker commands](../../adversarial-graph-algorithms/docker/README.md).


## Grustcat Cypher backend (2026-09-13)

The separate `rust/crates/grustcat-cypher` crate parses and semantically analyzes queries with Grust, validates a focused CALL/YIELD/RETURN subset, and executes Arrow algorithms and fused full-path aggregation. It is separate from the generic Grust procedure dispatcher. All five Cypher API tests, 45 runtime kernel/IPC checks, and 30 five-way Docker smoke cases passed. The 65,536-node full-path run completed for all five variants, including Grustcat Cypher at 16.727 s and official GDS at 194.113 s; results are one-sample completion measurements with documented timing boundaries.

See [the backend API](../rust/crates/grustcat-cypher/README.md), [benchmark evidence](../../adversarial-graph-algorithms/GRUSTCAT-CYPHER-RESULTS.md), and [the general dispatcher design](grust-procedure-dispatcher-plan.md).
