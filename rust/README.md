# Icecat Rust rewrite

Experimental immutable graph analytics, alongside the existing C++/Cython package.
The workspace uses Arrow 59.3 for buffers, properties, results and IPC; the optional
DataFusion 55.1 integration prepares graph inputs and joins graph results.
See the [implementation status](../docs/rust-rewrite-status.md) and
[execution roadmap](../docs/rust-rewrite-roadmap.md) before adopting it in production.

Terminology: **Icebug** is the original Arrow update of NetworKit (C++);
**Icecat** is this first Rust rewrite; **Grustcat** is the variant built against
[Grust](https://github.com/querygraph/grust), a backend-neutral property-graph
API for Rust that runs the same typed graph model over several storage backends.
Grustcat exposes these kernels through that API, so an application already using
Grust can run them over a graph it has already loaded. Existing internal
`icebug-*` crate names and the `icebug_rust` Python import remain unchanged for
compatibility.

## Build and test

From the repository root, with the pinned Rust toolchain installed:

```sh
cargo test --locked --manifest-path rust/Cargo.toml
cargo test --locked --manifest-path rust/Cargo.toml -p icebug-algorithms --features parallel
cargo test --locked --manifest-path rust/Cargo.toml -p icebug-datafusion
cargo fmt --manifest-path rust/Cargo.toml --all --check
```

Python development installation (Python 3.10 or newer):

```sh
cd rust
uv venv
uv pip install maturin pytest numpy pyarrow
VIRTUAL_ENV="$PWD/.venv" .venv/bin/maturin develop --release --locked
.venv/bin/python -m pytest compat-tests -q
```

For an installable wheel, use `.venv/bin/maturin build --release --locked --out dist`.
An sdist can be built with `.venv/bin/maturin sdist --out dist`. Maturin trims the
sdist workspace to Python dependencies; rebuilding it may prune the lock file, so
use the ordinary build command there without `--locked` (or `--offline` with cached dependencies).

## Python example

```python
import pyarrow as pa
from icebug_rust import Graph

nodes = pa.table({"node_id": [100, 200, 300], "name": ["a", "b", "isolated"]})
edges = pa.table({"source": [100, 200], "target": [200, 100]})
graph = Graph.from_tables(nodes, edges).prepare_incoming()
result = graph.pagerank()
assert result.converged
print(result.table)  # node_id contains dense internal IDs 0, 1, 2
print(graph.node_properties())  # node_id retains external IDs 100, 200, 300
print(graph.bfs(0))  # unreachable distances are null
```

`Graph.from_csr(n, directed, targets, offsets, ...)` imports UInt64 Arrow CSR.
Each row must be sorted, with unique in-range neighbors. Offsets start at zero,
have length `n + 1`, and end at the number of adjacency slots. Nulls are rejected;
Float64 weights must be finite. An undirected graph requires reciprocal slots
with equal weights. Self-loops count as one logical edge and one adjacency slot.

`copy="never"` shares compatible Arrow arrays and rejects required conversions;
`copy="if_needed"` reports conversions through `graph.import_report`;
`copy="always"` copies topology buffers. Shared foreign buffers must remain immutable.
Validation scans the input even when importing without copies. Preparing incoming
adjacency is a separate, explicit allocation. Imported incoming CSR must be an exact
transpose, including weights.

## Crates and supported operations

| Crate | Responsibilities |
| --- | --- |
| `icebug-core` | Validated CSR snapshots, edge/table construction, Arrow properties, induced views, cancellation and tracked reservations |
| `icebug-algorithms` | Degrees, BFS, Dijkstra, CC/WCC/SCC, PageRank, dense and sparse personalization; optional reusable Rayon PageRank executor |
| `icebug-io` | Version 1 checksummed Arrow IPC directory snapshots |
| `icebug-datafusion` | DataFrame construction and property/result registration; see `tests/workflow.rs` for a complete SQL workflow |
| `icebug-python` | PyO3 and Arrow C-interface bindings, packaged as `icebug_rust` |

Native algorithms return Arrow RecordBatches keyed by internal UInt64 node IDs.
Results carry process-local snapshot identity in schema metadata. External node IDs
remain in node properties; join using row position or DataFusion's explicitly added
`__icebug_internal_id` column. Node IDs accepted by table construction are Int64,
UInt64, or Utf8, with exactly matching endpoint types. Duplicate edges and unknown
endpoints are errors. Construction preserves isolated node rows and edge properties.

PageRank uses probability scores, L1 convergence, and personalized dangling-mass
redistribution. Negative weights are rejected by PageRank and Dijkstra; zero total
outgoing weight makes a PageRank vertex dangling. Maximum iterations and convergence
are reported explicitly. Directed PageRank and SCC require `prepare_incoming()`.
Python currently runs serial kernels; the optional Rayon executor is a Rust API.

`graph.induced(ids)` retains the base snapshot and uses compact IDs in ascending
base-ID order. `members()` exposes that mapping. Call `materialize()` to run algorithms
on a compact graph with selected node and logical-edge properties.

## Current limits

This is not a drop-in `networkit` replacement. Mutable graphs, community algorithms,
legacy stateful algorithm classes, and legacy PageRank rescaling modes are absent.
The icebug-format duck adapter handles homogeneous topology only and has not been
qualified against the external package. The DataFusion API is currently Rust-only.

Builders and DataFusion collection materialize their inputs in memory; there is no
spill-aware graph builder. `ExecutionContext` accounts for explicitly reserved
scratch/index allocations, not all allocations or process RSS. Foreign buffers,
builder temporaries, and returned result lifetimes are not fully budgeted. Snapshot
loading limits encoded bytes, not decoded allocation or RSS. Snapshot files must
remain immutable during loading. There is no mmap reader or power-loss qualification.

Unsafe code is forbidden in workspace crates; Arrow/PyO3 dependencies implement the
FFI boundary. Local validation covers macOS ARM64/Python 3.14. The CI matrix proposes
Linux/macOS/Windows and Python 3.10/3.14, but those remote jobs have not yet run.

## Grustcat and shared Arrow IPC

The optional local [Grustcat variant](crates/grustcat/README.md) implements five
algorithms over packed Arrow adjacency projected from Grust GraphIndex and reads/writes the same Arrow IPC tables as
Icecat. It requires the sibling `~/src/grust` checkout and is isolated from this
workspace and Python package. See its README for the supported Arrow types,
interchange contract, commands and benchmark boundaries.

## Grustcat Cypher

[grustcat-cypher](crates/grustcat-cypher/README.md) is a separate workspace using Grust parsing and semantic analysis with a focused Arrow algorithm-query backend. It supports the five benchmark algorithms and streaming full-path aggregation; it is not the general Grust reference executor. [General procedure-dispatcher design](../docs/grust-procedure-dispatcher-plan.md).
