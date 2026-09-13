"""Experimental Arrow-native graph snapshots; the legacy networkit package is separate."""
from dataclasses import dataclass
from operator import index

import pyarrow as pa

from . import _native
from ._native import ExecutionContext

__all__ = ["Graph", "InducedView", "PageRankResult", "ExecutionContext"]


@dataclass(frozen=True)
class PageRankResult:
    """Arrow scores keyed by internal IDs, with L1 convergence metadata."""

    table: pa.RecordBatch
    iterations: int
    residual: float
    converged: bool

    def scores(self):
        """Explicitly allocate and return a Python score list."""
        return self.table.column("score").to_pylist()


class Graph:
    """Immutable graph with dense internal IDs; external IDs are retained as properties.

    Use ``from_csr`` to share compatible Arrow topology, or ``from_tables`` to
    construct from node/edge tables. Directed PageRank and SCC require an explicit
    ``prepare_incoming()`` call. Clone/copy shares buffers; mutation is unsupported.
    """

    def __init__(self, native, import_report=None):
        if not isinstance(native, _native.Graph):
            raise TypeError("use Graph.from_csr or Graph.from_tables")
        self._graph = native
        self.import_report = import_report or {}

    @classmethod
    def from_csr(cls, n, directed, targets, offsets, *, weights=None,
                 in_targets=None, in_offsets=None, in_weights=None, copy="if_needed"):
        """Import sorted simple CSR. Nulls, duplicates, and malformed offsets fail.

        ``copy='never'`` requires compatible Arrow arrays/single chunks. Validation
        still scans shared buffers. Foreign producers must retain immutable data;
        use ``copy='always'`` for mutable buffers. Reverse-index allocation is a
        separate explicit operation, not included in the import copy policy.
        """
        if copy not in ("never", "if_needed", "always"):
            raise ValueError("copy must be never, if_needed, or always")
        report = {}

        def normalize(name, value, dtype):
            if value is None:
                return None
            operations = []
            if isinstance(value, pa.ChunkedArray):
                if value.num_chunks == 1:
                    value = value.chunk(0)
                else:
                    if copy == "never":
                        raise ValueError(f"{name} requires rechunking")
                    value = value.combine_chunks()
                    operations.append("rechunked")
            if not isinstance(value, pa.Array):
                if copy == "never":
                    raise TypeError(f"{name} must be an Arrow array for copy=never")
                if dtype == pa.uint64():
                    value = [None if item is None else index(item) for item in value]
                value = pa.array(value, type=dtype)
                operations.append("constructed")
            if value.type != dtype:
                if copy == "never":
                    raise TypeError(f"{name} requires {dtype}")
                if dtype == pa.uint64() and not pa.types.is_integer(value.type):
                    raise TypeError(f"{name} must contain integer IDs or offsets")
                value = value.cast(dtype, safe=True)
                operations.append("cast")
            if copy == "always":
                # concat allocates independent Arrow buffers even for a single input.
                value = pa.concat_arrays([value])
                operations.append("copied")
            report[name] = {"operations": operations or ["shared"], "logical_bytes": value.nbytes}
            return value

        arrays = [normalize(name, value, dtype) for name, value, dtype in [
            ("targets", targets, pa.uint64()), ("offsets", offsets, pa.uint64()),
            ("weights", weights, pa.float64()), ("in_targets", in_targets, pa.uint64()),
            ("in_offsets", in_offsets, pa.uint64()), ("in_weights", in_weights, pa.float64())]]
        return cls(_native.Graph.from_csr(n, directed, *arrays), report)

    @classmethod
    def from_tables(cls, nodes, edges, *, directed=True, weight=None, context=None):
        """Build CSR from Arrow tables with node_id/source/target columns.

        Node IDs must be unique, non-null Int64, UInt64, or Utf8 and endpoint
        types must match. Duplicate logical edges fail. All property columns and
        explicitly supplied isolated nodes are retained. Construction materializes
        the input and is not covered by the tracked scratch-memory budget yet.
        """
        return cls(_native.Graph.from_tables(nodes, edges, directed, weight, context))

    @classmethod
    def from_icebug_mem_graph(cls, graph, *, directed=True, copy="if_needed"):
        """Import homogeneous icebug-format topology; property transfer is not yet supported."""
        for name in ("src", "dest", "indices", "indptr"):
            if not isinstance(getattr(graph, name, None), pa.Table):
                raise TypeError(f"{name} must be an Arrow table")
        if not graph.src.equals(graph.dest):
            raise ValueError("only homogeneous graphs are supported")
        if not graph.indices.num_columns or not graph.indptr.num_columns:
            raise ValueError("indices and indptr must have columns")
        target = "target" if "target" in graph.indices.column_names else 0
        return cls.from_csr(graph.src.num_rows, directed, graph.indices.column(target),
                            graph.indptr.column(0), copy=copy)

    @property
    def node_count(self):
        return self._graph.node_count()

    @property
    def edge_count(self):
        return self._graph.edge_count()

    @property
    def self_loop_count(self):
        return self._graph.self_loop_count()

    @property
    def directed(self):
        return self._graph.is_directed()

    @property
    def weighted(self):
        return self._graph.is_weighted()

    @property
    def snapshot_id(self):
        return self._graph.snapshot_id()

    def __copy__(self):
        return Graph(self._graph.clone_snapshot(), self.import_report.copy())

    def prepare_incoming(self, context=None):
        """Return a handle with incoming CSR, sharing outgoing storage."""
        return Graph(self._graph.prepare_incoming(context))

    def neighbors(self, node):
        """Return an Arrow slice sharing outgoing adjacency."""
        return self._graph.neighbors(node)

    def has_edge(self, source, target):
        return self._graph.has_edge(source, target)

    def degrees(self, *, incoming=False, weighted=False, context=None):
        return self._graph.run("degree", incoming=incoming, weighted=weighted, context=context)

    def bfs(self, source, context=None):
        """Arrow shortest hop distances; unreachable vertices have null distance."""
        return self._graph.run("bfs", source=source, context=context)

    def dijkstra(self, source, context=None):
        """Arrow shortest weighted distances; negative weights are rejected."""
        return self._graph.run("dijkstra", source=source, context=context)

    def components(self, mode="weak", context=None):
        """Return components labeled by their minimum internal ID."""
        kinds = {"weak": "wcc", "strong": "scc", "undirected": "cc"}
        if mode not in kinds:
            raise ValueError("mode must be weak, strong, or undirected")
        return self._graph.run(kinds[mode], context=context)

    def pagerank(self, *, damping=0.85, tolerance=1e-8, max_iterations=1000,
                 personalization=None, sources=None, context=None):
        """Probability-mode PageRank with L1 stopping and personalized dangling redistribution.

        ``personalization`` is a dense vector; ``sources`` is a mapping or sequence
        of (internal ID, weight) pairs. Repeated sparse IDs are combined before
        normalization. Max-iteration termination is reported separately from convergence.
        """
        if isinstance(sources, dict):
            sources = list(sources.items())
        return PageRankResult(*self._graph.pagerank(damping, tolerance, max_iterations,
                                                  personalization, sources, context))

    def induced(self, nodes):
        """Create a fixed node-induced view; membership changes require a new view."""
        return InducedView(self._graph.induced(list(nodes)))

    @property
    def node_properties(self):
        return self._graph.node_properties()

    @property
    def edge_properties(self):
        return self._graph.edge_properties()

    def write_snapshot(self, path):
        """Write a new checked IPC directory; existing directories are rejected."""
        self._graph.write_snapshot(str(path))

    @classmethod
    def read_snapshot(cls, path, *, max_encoded_bytes=1 << 30):
        """Read checked IPC; max_encoded_bytes limits input file sizes, not decoded RSS."""
        return cls(_native.Graph.read_snapshot(str(path), max_encoded_bytes))


class InducedView:
    """Immutable compact selection retaining its base snapshot."""

    def __init__(self, native):
        self._view = native

    @property
    def members(self):
        return self._view.members()

    def neighbors(self, node):
        return self._view.neighbors(node)

    def materialize(self):
        """Explicitly allocate a compact graph, preserving selected properties."""
        return Graph(self._view.materialize())
