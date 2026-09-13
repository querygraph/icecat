# Rust rewrite: detailed execution roadmap

Terminology: Icebug = original Arrow-enabled NetworKit (C++); Icecat = first Rust
rewrite; Grustcat = Grust-compatible Rust rewrite.

Status: implementation underway, 2026-09-12. Repository baseline: `a7d7cd7c9`,
Icebug 13.4. See [implementation status](rust-rewrite-status.md) for completed
capabilities, validation evidence, and remaining acceptance criteria. Work packages
below describe the full target, not a claim that every release gate has passed.

This roadmap turns the [code review and architecture plan](rust-rewrite-plan.md)
into concrete work packages. That document contains source-linked findings,
Arrow/DataFusion references, graph invariants, and the rationale for the design.
This document defines execution order, deliverables, acceptance criteria, and
release decisions. All new paths and API names below are proposed.

## 1. Deliverable and scope

Deliver a Grust-compatible Rust graph analytics library first. This user-confirmed
priority (2026-09-13) supersedes the earlier Python/legacy-first framing. The
Icecat CSR and Grustcat implementations must share an explicit Arrow
interchange and graph-semantics contract. Python packaging and phased
`networkit` compatibility remain downstream deliverables. DataFusion is optional.

Before expanding the earlier work packages, execute these compatibility steps:

1. Extend W00 with a matrix against the local Grust version: application IDs,
   labels, optional edge IDs, isolates, loops, parallel edges, property types,
   missing/null values, direction and algorithm/result semantics. Mark each
   entry supported, explicitly rejected or deferred in each backend.
2. Extend W05/W09/W10 with a versioned shared Arrow schema, bidirectional IPC
   round trips and multi-batch ingestion. Specify Grust complex-property type
   mappings and explicit behavior where Icebug's simple-graph model cannot
   represent the input; require errors or a user-selected projection.
3. Extend W01 with Grust → Arrow → each backend → Arrow → Grust conformance
   tests, including external-ID result joins, malformed input and boundary cases.
   Preserve the existing independent algorithm oracles.
4. Qualify the supported subset before widening algorithms or Python/legacy
   APIs. Benchmark both storage variants using the same semantics and disclose
   construction, conversion, IO, kernel and result-materialization costs.

The current local scalar-property/single-batch implementation is the starting
point, not completion of these gates. Full Grust backend/database API parity is
not implied by graph-model and Arrow compatibility.

The first supported release must pass the Grust compatibility gates above and provide:

- Directed and undirected simple graphs, isolated vertices, self-loops, optional
  finite weights, dense internal IDs, and explicit external-ID mappings.
- Checked Arrow CSR import, batch-oriented edge-table construction, retained
  properties, explicit incoming-index preparation, and Arrow results.
- Immutable node-induced views and explicit materialization of edge-filtered
  graphs, with correct ID/property mappings.
- Degree, weighted degree, BFS/reachability, Dijkstra, connected components,
  weakly/strongly connected components, PageRank and personalized PageRank.
- Versioned IPC persistence, a working icebug-format adapter, and an end-to-end
  DataFusion preparation → graph computation → property/result join workflow.
- Documented memory use, cancellation, supported Python compatibility methods,
  clean native/Python packages, and measured correctness/performance evidence.

Full mutable-graph parity, all inherited algorithm families, arbitrary multigraph
analytics, heterogeneous graphs, distributed execution, and out-of-core kernels
are separate milestones. Community detection is the next substantial algorithm
wave. Keep these boundaries visible in the compatibility matrix so unsupported
features do not become silent fallbacks or implied release promises.

## 2. Organization and working rules

Use two engineering roles for planning: **core lead** owns topology, algorithms,
and performance; **integration lead** owns Arrow/Python, DataFusion, persistence,
and packaging. These are responsibilities, not assigned people. Both review
ownership, identity, and resource contracts. A graph-domain reviewer validates
numerical and community semantics when available.

Keep the existing C++/Cython package operational. Initially place the Rust work
in a nested workspace:

```text
rust/
  Cargo.toml
  Cargo.lock
  rust-toolchain.toml
  crates/
    icebug-core/src/
    icebug-algorithms/src/
    icebug-io/src/
    icebug-datafusion/src/
    icebug-python/src/
  python/icebug_rust/
  compat-tests/
  benchmarks/
  fuzz/
docs/rust-rewrite/
  decisions/
  compatibility.csv
  validation-report.md
  migration.md
```

Create components when their work package starts. A core build must not pull in
DataFusion, Tokio, Python, or C++. Put shared execution-resource types in a small
core module so builders, index preparation, and algorithms can use them without
a dependency cycle. Query integration adapts those types into its own runtime.

Each package below should produce one or more bounded PRs. Keep correctness,
parallelization, and unsafe optimization in separate reviews. A package is done
only when its acceptance evidence is reproducible from committed commands and
fixtures. Proposed performance limits are ratified after baseline measurement;
they are not current results.

## 3. Dependency map and milestones

| Work package | Prerequisites | Lead | Milestone |
| --- | --- | --- | --- |
| W00: feature and semantic contract | None | Both | Baseline |
| W01: legacy/reference harness | W00 initial decisions | Core | Baseline |
| W02: Rust workspace and dependency proof | W00 initial decisions | Integration | Storage |
| W03: checked CSR and incoming index | W02; W00 graph contract | Core | Storage |
| W04: execution resources and results | W02; W00 result contract | Core | Storage |
| W05: Arrow/Python ownership path | W03, W04 | Integration | Storage |
| W06: serial traversal/components | W01, W03, W04 | Core | Analytics |
| W07: serial PageRank and personalization | W01, W03, W04 for native work; W05 for Python acceptance | Core | Analytics |
| W08: immutable views | W03, W04 to start; W06/W07/W09 for full acceptance | Core | Analytics |
| W09: edge batches, IDs, properties | W03, W04; W00 identity contract | Integration | Integration |
| W10: persistence and format adapter | W05, W09 | Integration | Integration |
| W11: DataFusion preparation/results | W07, W09 | Integration | Integration |
| W12: selective graph table providers | W11 and demonstrated need | Integration | Optional optimization |
| W13: parallel kernels/performance | W05–W11 relevant correctness gates | Core | Release |
| W14: Python facade and package matrix | W05–W11; API stabilized | Integration | Release |
| W15: release qualification and rollout | W13, W14; all mandatory gates | Both | Release |
| W16: community and wider analytics | First release; domain contracts | Core | Expansion |

W03 and W04 can progress together after their interfaces are agreed. W09 can run
beside W06/W07. W10 and W11 share integration staffing, so concurrent scheduling
requires help or narrower scope. W12 is not required if existing batch/file
providers already meet the first-release workflow. Run packaging smoke builds
from W02 onward; W14 is their final hardening step.

The critical path is semantic agreement → validated ownership/storage → correct
algorithms and property construction → integrated performance → release evidence.

## 4. Detailed work packages

### W00 — Freeze the supported behavior

**Work.** Inventory public imports, constructors, algorithms, methods, flags,
exception behavior, return types, and mutation requirements. Record each API as
first release, expansion, legacy-only, or intentionally changed. Include
`networkit`/`icebug` aliases, `GraphFromCoo`, native C++ use, serialization, and
Leiden scoring extensions. Prioritize from call sites and representative use
cases; inferred priorities remain labeled assumptions until confirmed.

Write short decisions for graph identity, simple-edge/loop policy, weight domains,
incoming-index preparation, property alignment, immutable views, PageRank modes,
Python compatibility, Arrow ownership, and the snapshot format. Explicitly
document that new snapshot cloning shares storage whereas current Python graph
copies can materialize writable graphs. Set finite algorithm iteration defaults;
record legacy differences in the facade contract.

**Artifacts.** `docs/rust-rewrite/compatibility.csv` and numbered decision notes.
Each API row includes capability requirements, migration target, comparison
method, and its known legacy defect or intentional semantic difference.

**Acceptance.** Every first-release API has a declared graph domain, input/output
contract, error policy, and acceptance test description. Deferred families have
an explicit disposition. Resolve graph/identity decisions before W03/W09; later
scope changes must state schedule and compatibility impact.

### W01 — Establish trustworthy references and measurements

**Work.** Reproduce the legacy build using executable dependency configuration.
Record compiler, Arrow/Python versions, optimization flags, CPU, RAM, OS, and
thread settings. Audit Python test collection and rename shadowed test methods.
Reproduce the source-review findings as minimal tests, running potential native
crash cases in isolated subprocesses. Keep baseline defects recorded rather than
treating their outputs as correct answers. Legacy fixes can be separate PRs with
their own regression evidence.

Implement a tiny independent edge-map reference for graph semantics and small
reference routines for traversal, components, and PageRank. Use class-based
Python tests. Produce fixtures with explicit isolated nodes, loops, duplicate
policies, weighted/directed variants, sparse external labels, and known numerical
answers. Store seeds, dataset provenance/checksums, and expected comparison rules.

**Artifacts.** `rust/compat-tests/`, `rust/benchmarks/datasets.json`, baseline
commands, machine-readable results, and a known-failures ledger. Preserve original
and corrected legacy baseline revisions separately when bug fixes change results.

**Acceptance.** Another developer can build the baseline and reproduce collected
test counts, expected failures, reference answers, and timing/memory reports.
Measure C++ GraphW and GraphR only on their supported, correct input paths.
The comparison harness distinguishes crashes, unsupported cases, wrong answers,
and numerical differences.

### W02 — Prove the Rust dependency and packaging foundation

**Work.** Add the nested workspace, a tested toolchain, shared dependency versions,
minimal feature sets, and license metadata. Resolve one compatible Arrow family
with the selected DataFusion release; independently prove PyO3 interoperability.
Add the smallest Python extension and clean native consumer example. Build
core-only and query-enabled configurations so optional dependencies remain real.

**Artifacts.** Workspace manifests, lockfile, toolchain file, and an initial
`.github/workflows/rust.yml`. Record the resolved compatibility matrix in a
decision note; use the architecture document's versions as candidates to test.

**Acceptance.** Formatting, linting, unit/doc-test scaffolds, clean Python wheel
installation, and native consumer smoke tests pass. Inspect the dependency tree
for incompatible duplicated Arrow types. The core build works without Python,
Tokio, or DataFusion installed or enabled. Existing C++ CI still runs unchanged.

### W03 — Implement checked topology and explicit incoming adjacency

**Work.** Add concrete `NodeId`, `EdgeId`, `ArcSlot`, `CsrAdjacency`, and
`GraphSnapshot` types. Build checked constructors over owned Arrow typed buffers.
Validate types, nulls, offset length/origin/monotonicity/terminal value, endpoint
bounds, sorting, duplicates, weights, and all arithmetic conversions. Preserve
empty-but-present weights. Test logical Arrow slices without rebasing by accident.

Implement correct directed/undirected edge and loop metadata. Adopt a clear
contract for reciprocal undirected slots. Implement supplied-transpose validation
and `prepare_incoming` using degree counts, prefix sums, and scatter. Preparation
returns a handle sharing the original topology; allocation failures leave the
original snapshot valid. Store the weight/edge mapping consistently in both
directions. Incoming-dependent operations require prepared capability.

**Artifacts.** Core `ids.rs`, `csr.rs`, `graph.rs`, `validate.rs`, `transpose.rs`,
and property/fuzz tests. Start with safe checked traversal and concrete storage;
add traits only where views or prepared graphs require a second implementation.

**Acceptance.** Valid snapshots match the reference edge map; every invalid
fixture returns a structured error before traversal. Directed transpose preserves
every logical edge and weight. Loops and empty graphs have correct counts.
Buffer adoption preserves ownership and pointer identity. Validation scratch
and transpose allocation are reported separately from imported buffer sharing.

### W04 — Define resource control, errors, and result ownership

**Work.** Add an execution context with a CPU budget, cancellation token, memory
admission/reservations, and stage metrics. Specify ownership of graph, scratch,
prepared indexes, input batches, and returned results. Reserve known large
allocations before creating them; release reservations when their owners drop,
including error and cancellation paths. Report external/shared buffers without
double-counting the same allocation across snapshots.

Define errors for invalid graph/schema/options, missing capabilities, unsupported
operations, overflow, memory limits, cancellation, and I/O. Algorithms return
typed results with Arrow conversion, snapshot/mapping identity, parameters,
iterations, residual, termination reason, and stage timings where applicable.
Results must retain any owners needed by exported arrays.

**Artifacts.** Core `execution.rs`, `memory.rs`, `error.rs`; algorithm `result.rs`;
tests for reservation lifetime and failure paths.

**Acceptance.** Tests inject failure between allocation stages and leave no live
scratch reservation. Concurrent operations share the declared budget without
mutating snapshots. Cancellation is distinguishable from success. Graph/result
lifetimes and metrics remain valid after input handles are dropped. Document
untracked allocator overhead; do not describe the budget as a perfect RSS cap.

### W05 — Complete the first Arrow/Python vertical slice

**Work.** Import compatible Arrow arrays through supported capsule helpers,
construct a validated snapshot, compute degrees, and export Arrow results.
Implement `copy="never"`, `"if_needed"`, and `"always"` plus an import report.
Add casts/rechunking only as explicit, reported normalization. Keep the Python
wrapper thin and own snapshots through `Arc`; detached computation must not
borrow Python object storage without a valid imported owner.

Test capsule ownership transfer and release, invalid input error mapping, sliced
arrays, empty chunks, exporters dropped before computation, and final release
from worker threads. Use instrumented valid exporters for release tests. Treat
the foreign producer's validity/immutability contract as an FFI precondition.

**Artifacts.** Python extension `arrow.rs`, `graph.rs`, `errors.rs`; package
`rust/python/icebug_rust/`; ownership tests and an executable degree example.

**Acceptance.** Arrow → graph → degree → Arrow works after deleting Python input
objects and forcing GC. Compatible no-copy imports share the same buffers;
normalization reports its copies. Release callbacks run exactly once, including
exception paths. Computation releases the interpreter and the graph remains
usable afterward. This is the first storage/FFI investment gate.

### W06 — Implement serial traversal and components

**Work.** Port degree/weighted-degree inspection, BFS/reachability, Dijkstra,
undirected CC, WCC, and SCC. Choose straightforward serial algorithms first;
for example, outgoing-edge union for WCC and a two-pass SCC algorithm over a
prepared transpose. Use iterative traversals so graph depth does not consume
the process call stack. Declare each routine's capability and weight domain.

Specify unreachable distances, optional predecessor storage, path reconstruction,
tie behavior, and component-label policy. Dijkstra rejects negative weights and
reports arithmetic overflow; optional outputs allocate only when requested.
Return dense internal results with explicit external-ID mapping. Implement
cancellation checkpoints inside long scans as well as between stages.

**Artifacts.** Algorithm `degree.rs`, `bfs.rs`, `dijkstra.rs`, `components.rs`;
corresponding independent and cross-backend tests.

**Acceptance.** Paths, distances, and partitions match independent answers on
all graph fixtures, including long chains, disconnected graphs, loops, isolates,
and zero-weight edges. Compare partitions independent of label numbering.
Measure serial work/memory scaling and verify that no scratch allocation depends
on the largest external label.

### W07 — Implement PageRank from an explicit mathematical contract

**Work.** Implement one validated configuration path for ordinary, dense-personalized,
and sparse-personalized input. Require finite valid damping/tolerance, a finite
iteration cap, nonnegative finite graph weights, and nonnegative personalization
with positive total mass. Combine duplicate personalization IDs before
normalization without allocating a vector indexed by external IDs.
Check computed weight sums and normalization factors for overflow; use numerically
stable accumulation or return a defined error rather than producing non-finite
scores from otherwise finite inputs.

For canonical probability-mode PageRank, use:

```text
next = damping * transition_transpose * current
     + ((1 - damping) + damping * dangling_mass) * personalization
```

Use incoming pull traversal with one output slot per vertex. Treat zero outgoing
weight sum as dangling. Apply sparse teleport/dangling additions in an `O(k)`
pass after the traversal rather than scanning all sources at every vertex.
Track convergence separately from the iteration cap. Empty graphs return an
empty result with zero iterations. Add explicitly named legacy normalization and
sink modes only after their intended behavior is recorded by W00/W01.

**Artifacts.** Algorithm `pagerank.rs`, shared personalization validation, Python
entry points, analytic fixtures, and residual/mass tests.

**Acceptance.** Numerical results match the independent formulation; canonical
scores are finite, nonnegative, and sum to one within the chosen tolerance.
Test sinks, all-dangling graphs, empty graphs, zero-weight rows, duplicate sources,
unnormalized weights, extreme finite inputs, and early cancellation. Sparse
configuration needs `O(k)` storage and each iteration targets `O(n+a+k)` work.
Correctness tests include modes that previously masked normalization defects.

### W08 — Implement immutable views and explicit materialization

**Work.** Add an immutable node selection owning its base snapshot, compact
base-to-view/view-to-base mappings, and filtered adjacency traversal. Preserve
ascending base-ID order for default compact IDs. Compute or explicitly cache
degree metadata instead of claiming constant-time degree from a fresh scan.
Flatten or correctly compose nested selections and retain property identity.

Make membership changes produce a new view. Add `materialize` to build a compact
snapshot for repeated analytics. Node-induced selection can share topology;
arbitrary edge filtering initially builds new CSR. Keep explicitly selected
isolated vertices unless an endpoint-only selection is requested.

**Artifacts.** Core `view.rs`, `selection.rs`, mapping tests, and a view versus
materialized-graph benchmark.

**Acceptance.** Views and materialized graphs agree on topology, degrees,
components, PageRank, and property joins. Drop all external base handles and
verify the view remains usable. Check nested/empty selections and ID mappings.
Measure view construction, base-sized membership storage, selected-node scratch,
and repeated traversal; publish the materialization tradeoff.

### W09 — Build CSR directly from edge batches and retain properties

**Work.** Accept explicit node tables and edge batches/streams. Preserve node-table
row order; validate non-null supported external key types, uniqueness, and endpoint
membership. For edge-only ingestion, implement a documented deterministic ID
assignment policy. Preserve source row/edge identity and property sidecars before
parallel preparation or sorting changes order.

Separate topology projection from property retention. Normalize directed edges or
canonical undirected pairs, then count, prefix-sum, fill, and sort CSR. Distinguish
an undirected logical-edge list from a supplied reciprocal arc representation.
Reject duplicate logical edges by default; explicit merging requires declared
weight and property reducers. Reject ambiguous property aggregation. Generate
slot-to-logical-edge and input-to-output mappings when needed.

Handle batch boundaries, schema consistency, dictionaries, nullable properties,
empty streams, and presorted inputs. Add bounded queues and memory estimates.
Provide a rewindable/spooled preparation path when construction needs more than
one pass. Streaming arrival does not remove final in-memory graph requirements.

**Artifacts.** Core `builder.rs`, `mapping.rs`, `properties.rs`; I/O `batches.rs`;
directed/undirected builder and property-round-trip tests.

**Acceptance.** Input permutation and batch boundaries do not change logical
results under deterministic policy. Each output edge's properties match its
original/aggregated identity after sorting and mirroring. Explicit node tables
retain isolates. A large-graph construction report explains peak memory from
retained input, mappings, sorting, CSR, transpose, and properties.

### W10 — Add durable snapshots and icebug-format interoperability

**Work.** Implement a versioned manifest and separate IPC sections for node tables,
edge properties, offsets, adjacency, and optional reverse/mapping data. Record
schema, counts, direction, ID and weight types, loop/duplicate conventions, and
checksums. Write into a temporary location and publish only after all sections
are complete. Define a platform-tested publication rule for directories/files.

Add a checked ordinary reader first; reject inconsistent versions, schemas,
relationships, counts, or checksums before publishing a graph handle. Inspect
the external icebug-format implementation and obtain golden fixtures before
declaring interoperability. Map its semantics through an adapter rather than
reusing its package name for a different layout. Add mmap only after the ordinary
reader passes, with explicit file immutability and mapping-owner tests.

**Artifacts.** I/O `manifest.rs`, `ipc.rs`, `icebug_format.rs` or a Python adapter
if the external protocol is Python-specific; format specification and fixtures.

**Acceptance.** Rust/Python round trips preserve graph, IDs, weights, nullable
properties, and index presence. Partial writes are not readable as valid snapshots.
Wrong-schema, corrupted, truncated, and unsupported-version files fail cleanly.
Interoperability is tested against the actual external package. Optional mmap
does not become a first-release blocker.

### W11 — Integrate DataFusion preparation and result analysis

**Work.** Start with existing file and Arrow-batch providers. Support projection,
property filtering, external-ID joins, explicit edge normalization, and ordering
where required before the builder. Carry stable identity columns through each
plan. Node filtering must remove incident edges with missing selected endpoints;
edge filtering retains the selected node universe unless an explicit endpoint-only
node universe is requested. ID compaction alone does not remove vertices.

Consume query batches with backpressure and cancellation, then run native graph
kernels. Register Arrow score/component results as tables keyed by the correct
node identity and snapshot. Join them to property tables for ranking and grouped
analysis. Explicitly separate query runtime, graph CPU pool, and resource budgets.

**Artifacts.** DataFusion `prepare.rs`, `results.rs`, `resources.rs`; an example
that reads edge/property data, filters a subgraph, builds CSR, runs PageRank, and
joins scores to labels. Add a typed configuration API rather than requiring
string construction for every operation.

**Acceptance.** End-to-end results equal a plain-Arrow/reference pipeline, including
null predicates, isolated nodes, noncontiguous labels, and batch boundaries.
Inspect `EXPLAIN` for unnecessary full-width scans or sorts. Measure query,
mapping/build, transpose, kernel, and export independently. Test memory exhaustion,
supported spill behavior, cancellation, and cleanup while building a graph.

### W12 — Add custom graph providers only where they help

**Work.** Use W11 profiles to decide whether custom `nodes`, `edges`, or `arcs`
table providers improve a required workflow. If needed, implement projection,
partitioned batch scans, and source-ID range pruning over CSR. `edges` emits one
row per logical edge; `arcs` emits one per adjacency slot. Only advertise exact
pushdown, ordering, or statistics when the provider guarantees them.

Keep planning lightweight; produce batches during execution. Generate source-ID
columns and gather properties with their allocation costs accounted. Defer SQL
graph-algorithm operators until there is an actual composability requirement;
if added, define once-per-snapshot execution and correct multi-partition behavior.

**Artifacts.** DataFusion `tables.rs`, `scan.rs`, semantic/optimizer tests, and
before/after workload measurements, or a decision recording that existing
providers suffice and W12 is deferred.

**Acceptance.** Query results match ordinary registered batches across projections,
filters, limits, and partitions. Undirected edge counts are never doubled by
accident. `LIMIT` cannot bypass required graph computation. Any custom provider
has demonstrated benefit that justifies its maintenance burden.

### W13 — Parallelize and qualify performance

**Work.** Profile correct serial kernels first. Parallelize PageRank with disjoint
output ranges and work partitioning that handles degree skew. Add reusable
scratch and reductions with documented determinism. Extend parallelism to other
algorithms only where measurements justify complexity. Test multiple concurrent
jobs to control CPU oversubscription and memory contention.

Benchmark contiguous reverse weights versus canonical-weight gathers. Evaluate
view materialization, `u32` adjacency specialization, mmap, or unchecked accesses
as separate optional experiments. Keep the simple implementation when an
optimization lacks a clear end-to-end benefit. Any unsafe change requires an
invariant proof, focused boundary tests, and a measured gain.

**Artifacts.** Benchmark runner, comparable machine-readable reports, per-stage
profiles, and focused optimization PRs.

**Acceptance.** All correctness suites still pass at one and multiple threads.
Required workloads meet the architecture plan's agreed latency, memory, copy,
and cancellation gates. Repeat noisy measurements and report dispersion.
Separate kernel improvements from ingestion/output costs; explain regressions
caused by correcting invalid legacy behavior. Publish full retained-state and
peak-RSS accounting for the largest supported benchmark.

### W14 — Finish the Python facade and release artifacts

**Work.** Finish first-release public methods, typed options, Python documentation,
and explicit Arrow/NumPy/list output conversion. Implement only declared legacy
aliases and `run().scores()` conveniences, with state/ownership retained safely.
Unsupported operations raise clear errors; they must not silently materialize a
mutable graph. Document changes in copying, views, IDs, PageRank, and exceptions.

Choose Python query delivery from the packaging spike: separate query extension
or deliberately bundled wheel. An installation extra alone cannot change native
wheel features. Keep a distinct import/distribution during opt-in migration.
Build the advertised OS/architecture/Python/PyArrow matrix, with both minimal
and query-enabled configurations. Test source distributions and native consumers.

**Artifacts.** Package manifests, final public wrappers and stubs, installed-wheel
functional tests, native consumer examples, and migration documentation.

**Acceptance.** A clean environment outside the repository installs artifacts,
imports Arrow, creates weighted directed CSR, runs PageRank/components, queries
results, and retains views/results after input GC. It requires no system Arrow
C++ library. An unpacked sdist rebuilds and a packaged Rust consumer works.
Compare each supported compatibility row against its documented semantics.

### W15 — Qualify, release, and preserve rollback

**Work.** Produce a release report with semantic coverage, intentional differences,
baseline revisions, performance results, dependency/platform matrix, known limits,
and format compatibility. Run representative workflows through both backends
from the same immutable inputs. Invite opt-in use before changing the default
backend for any supported surface. Preserve the previous released backend and
data artifacts throughout rollout.

**Artifacts.** `docs/rust-rewrite/validation-report.md`, migration and rollback
instructions, release notes, and reproducible artifact-build commands.

**Acceptance.** All mandatory work packages are accepted; unresolved correctness
defects block release. Confirm installation, dataset reload, and backend/version
rollback through a rehearsal. Follow the repository's maintainer-review,
deprecation, and major-version rules. Choose default-backend changes separately
from merely publishing an opt-in package. Every advertised feature has a test
and a documented compatibility status.

Rollback uses retained original node/edge data or a tested legacy export. Do not
assume the C++ backend can read the new IPC snapshot format; document that boundary
and rehearse the data conversion as well as the package/version switch.

### W16 — Expand algorithms in independently accepted waves

**Work.** Prioritize community detection, k-core, triangles/clustering, and selected
betweenness from real adoption. Establish partition and coarsening contracts
before Louvain/Leiden. Implement a correctness-first serial baseline, then
parallelize with objective/refinement tests. Preserve external scoring extensions
on the legacy backend until their versioned replacement is specified.

For every later family, record accepted graph domains, required capabilities,
references, numerical/stochastic comparison method, memory model, benchmark
workload, and compatibility scope. Do not bundle unrelated generators, layouts,
dynamic algorithms, or file formats into a community milestone.

**Acceptance.** Coarsening conserves declared weights and mappings; partitions
cover the graph; Leiden tests enforce the intended connectivity/refinement
properties rather than matching known defective legacy labels. Each algorithm
wave has its own release gate and estimate. Broader parity remains demand-led.

## 5. First two weeks and initial PR sequence

| Window | Core lead | Integration lead | Observable output |
| --- | --- | --- | --- |
| Days 1–2 | Inventory graph/algorithm semantics and legacy failures | Inventory public API, package/support surfaces, Arrow boundary | Initial compatibility matrix and unresolved-decision list |
| Days 3–4 | Reproduce focused defects; audit collected tests; write tiny references | Reproduce build/dependency matrix; test Rust/Arrow/PyO3 resolution | Reproducible commands and independent graph fixtures |
| Day 5 | Capture small/medium GraphW/GraphR baselines | Build and install minimal native/Python artifacts | Baseline report; first packaging proof |
| Days 6–8 | Start checked CSR and resource interfaces | Start owned Arrow import/export against agreed interfaces | Reviewed constructor, identity, and lifetime contracts |
| Days 9–10 | Add degrees, loops, transpose tests | Complete degree round trip and GC/release tests | Demonstration and revised schedule based on actual complexity |

This is a proposed working schedule, not a promise that all storage work finishes
in ten days. A failed baseline build or dependency spike is visible work with a
named resolution, and shifts dependent dates. The initial PR sequence should be:

1. Compatibility inventory and graph/result decisions (W00).
2. Test collection fixes, independent fixtures, reproducers, baseline runner (W01).
3. Nested Rust workspace and minimal native/Python CI (W02).
4. Checked CSR representation and errors (first part of W03/W04).
5. Resource reservations, directed preparation, and result ownership (W03/W04).
6. Arrow import → degrees → Arrow export, including GC/lifetime tests (W05).
7. Serial traversal/components and PageRank as separate algorithm PRs (W06/W07).
8. Views and batch/property construction on separate tracks (W08/W09).

Later PRs follow W10–W15 dependencies. A package can span multiple PRs; keep a PR
reviewable by separating a new contract from optional optimization of it.

## 6. Scheduling assumptions and contingency

The architecture plan's phases 0–4 sum to **10–17 weeks** for two experienced
engineers, before additional contingency. That is a provisional phase-level
estimate, not a sum of independently estimated tickets. Re-estimate W00–W15 after
the first ownership/benchmark slice; list omitted features explicitly if narrowing
the release is required. Respect reviewer availability and the repository's
seven-day post-substantial-change PR review window when projecting merge dates.
Independent PR reviews can overlap; serial dependencies can extend the schedule.

| Risk signal | Response and decision point |
| --- | --- |
| Legacy tests cannot run reproducibly or expected answers disagree | Resolve the build and independent semantics in W01 before treating parity as a gate; retain defect reproducers separately. |
| Arrow ownership/release is unsafe or requires unexpected copies | Block Python/foreign-buffer execution at W05; constrain accepted exporters or use a documented copy path until sharing is proven. Independent native kernel work may continue on owned, validated buffers. |
| External-ID mapping or retained property buffers dominate peak memory | Measure W09 stages; choose bounded/spooled preparation, narrower columns, or explicit memory limits. Do not hide mapping costs in kernel numbers. |
| Incoming CSR exceeds the workload budget | Benchmark push variants or incoming-weight permutations; retain explicit missing-index errors until a supported alternative exists. |
| DataFusion build size or Python version coupling is excessive | Keep the core package independent; choose a separate query artifact or narrow integration features after the packaging spike. |
| DataFusion/custom scans do not improve the actual workflow | Retain ordinary batch/file providers and defer W12; graph kernels remain usable independently. |
| Parallel Rust kernels miss the baseline | Profile edge work, memory bandwidth, and scheduling; keep the serial oracle and change one optimization at a time. |
| Required compatibility expands to mutation or broad NetworKit parity | Create a separate scope/estimate; keep the legacy backend and revise the first-release commitment explicitly. |
| Community semantics cannot meet connectivity/refinement guarantees | Keep W16 independent from the first release; prioritize correctness over exact legacy output or parallel speed. |
| Target graphs cannot fit final topology and scratch in memory | Reject with a usable estimate; commission a separate out-of-core/distributed design rather than implying relational spill solves graph execution. |

## 7. Definition of done and status tracking

Track each work package as `not started`, `in progress`, `in review`, or `accepted`.
Acceptance must link to a revision, commands, test results, benchmark evidence
where applicable, and the compatibility/decision records it satisfies. Keep a
short unresolved-issues list with an owner and next decision; elapsed time is not
acceptance. At the time this document was written all work packages are proposed.

A reviewable implementation change includes its behavior, domain restrictions,
failure behavior, ownership/resource effect, relevant tests, and an executable
example when it changes the public API. Performance-sensitive changes also include
a reproducible comparison. Format, lint, feature-matrix, and required tests pass
before merging; public APIs have Rust/Python documentation and migration notes.

The first release is complete only when the required graph/algorithm/query
workflow works from installed artifacts, source and independent semantic tests
pass, resource/performance targets have measured evidence, and rollback has been
rehearsed. A successful compilation or broad similarity to legacy APIs is not
sufficient acceptance evidence.
