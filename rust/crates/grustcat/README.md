# Grustcat: Grust-compatible graph analytics

This experimental local variant uses `~/src/grust` through path dependencies.
It is a separate Cargo workspace so ordinary Icecat/Python builds do not need
Grust. It has no Python bindings yet and does not replace the original backend.

```sh
cargo test --manifest-path rust/crates/grustcat/Cargo.toml --locked
cargo clippy --manifest-path rust/crates/grustcat/Cargo.toml --all-targets -- -D warnings
```

`GrustcatGraph` retains the Grust model and projects its validated `GraphIndex`
into packed Arrow adjacency once at construction. Dense endpoints are copied
out and the temporary Grust index is dropped before building both orientations
together. Outgoing/incoming targets and weights are contiguous. This is an
O(n + m) allocation and copy, not zero-copy indexing. The retained adjacency uses
approximately 16(n + 1) + 32m bytes (two offsets/targets/weights sets), in addition
to the Grust model and allocator metadata. Kernels execute in Grustcat and return
Arrow RecordBatches with dense UInt64
`node_id` in original node row order, null unreachable distances, minimum-row-ID
component labels and probability PageRank scores. PageRank uses damping .85,
L1 tolerance 1e-8 and 1000 maximum iterations. Personalization, degree kernels,
parallel execution and the broader Icecat API are not implemented here.
ExecutionContext cancellation is polled, but its memory budget is not enforced
by this variant and polling occurs at traversal boundaries and within long adjacency rows.

## Direct Arrow read/write with both backends

```rust,no_run
use grustcat::{ArrowGraph, GrustcatGraph, icecat_from_arrow, arrow_from_icecat};
use icebug_core::ExecutionContext;
use std::fs::File;
# fn main() -> Result<(), Box<dyn std::error::Error>> {
let tables = ArrowGraph::read_ipc(
    File::open("nodes.arrow")?, File::open("edges.arrow")?,
)?;
let ctx = ExecutionContext::default();
let grust = GrustcatGraph::from_arrow(&tables, Some("weight"))?;
let icecat = icecat_from_arrow(&tables, Some("weight"), &ctx)?;
let distances = grust.bfs(0, &ctx)?;
arrow_from_icecat(&icecat)?.write_ipc(
    File::create("copied-nodes.arrow")?, File::create("copied-edges.arrow")?,
)?;
# Ok(()) }
```

`ArrowGraph::try_new(nodes, edges)` accepts in-memory Arrow RecordBatches;
`nodes()` and `edges()` expose them directly for Arrow/DataFusion use. Imported
Icecat tables retain original IDs, labels and property columns. Topology-only
Icecat export creates decimal-string IDs, default labels, and a Float64 weight
property when weighted. Export rejects undirected graphs and nonconforming
attached property schemas rather than losing data.

The shared contract is documented in `~/src/grust/crates/grust-arrow/README.md`.
It represents directed graphs, with native Boolean, Int64, Float64, Utf8 and
null scalar properties; presence columns distinguish missing from explicit
null. Complex properties and multiple IPC record batches are currently
unsupported. Arrow input/output is direct, but Grust model/index construction
materializes values; this is not a zero-copy analytics backend. Icecat rejects
parallel edges, which Grust supports.

## Benchmark executables

In `~/src/adversarial-graph-algorithms`, `./build.sh` builds both Rust variants
and C++. Both Rust executables accept a text edge list or a directory containing
`nodes.arrow` and `edges.arrow`. An optional final argument exports the loaded
graph into a **new** directory before the algorithm timer starts:

```sh
./grustcat data/hub-16384.txt bfs /tmp/grust.bin 0 /tmp/new-grust-arrow
./icecat /tmp/new-grust-arrow bfs /tmp/icebug.bin 0
./icecat data/hub-16384.txt bfs /tmp/icebug.bin 0 /tmp/new-icebug-arrow
./grustcat /tmp/new-icebug-arrow bfs /tmp/grust.bin 0
python3 compare_grustcat.py
```

Benchmarks exclude graph construction, model/index allocation, IPC IO and output
serialization. Rust timings include Arrow result construction. Results are in
`results/grustcat-final.json` and `GRUSTCAT-RESULTS.md` in the benchmark directory.

The historical `GrustGraph`, `icebug_from_arrow` and `arrow_from_icebug` names
remain compatibility aliases. The crate and primary executable are now `grustcat`.

BFS reserves an n-node circular queue when mean out-degree is at least four;
otherwise it starts small. This reduces reallocations on broad frontiers while
keeping narrow traversals compact. It can reserve more memory than a reachable
subgraph needs. WCC reuses the current union-find root across each row.

SCC stack frames retain global edge ranges and reserve up to n frames (three
`usize` values each), removing repeated row lookup inside the DFS edge loop.

## Measured kernel qualification

On the Apple M1 Max, all 60 main cases and 30 larger holdout cases passed full
result checks and `Grustcat <= max(1.10 * Icecat, Icecat + 0.1 ms)`. Main
geometric-mean times are 0.829–0.898 of Icecat depending on algorithm; speedup
over original Grustcat is 1.19–2.58x. See the complete benchmark report for
all cases, raw samples and the construction/IO/memory boundaries.

`dijkstra_paths(source, ctx, visitor)` visits a source-first node-ID slice and cumulative-cost slice for each reachable target, including the source. Ties choose one shortest path. Buffers are reused and valid only during the callback; total work scales with the sum of path lengths. The distance result remains an Arrow record batch.
