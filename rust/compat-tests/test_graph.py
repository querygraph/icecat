import copy
import gc
import random
import tempfile
import unittest
from pathlib import Path

import numpy as np
import pyarrow as pa

from icebug_rust import ExecutionContext, Graph


class TestGraph(unittest.TestCase):
    def cycle(self):
        return Graph.from_csr(3, True, pa.array([1, 2, 0], type=pa.uint64()),
                              pa.array([0, 1, 2, 3], type=pa.uint64()), copy="never")

    def test_zero_copy_slice_lifetime(self):
        targets = pa.array([999, 1, 2, 0, 999], type=pa.uint64()).slice(1, 3)
        offsets = pa.array([999, 0, 1, 2, 3], type=pa.uint64()).slice(1)
        graph = Graph.from_csr(3, True, targets, offsets, copy="never")
        result = graph.neighbors(0)
        self.assertEqual(result.buffers()[1].address + result.offset * 8,
                         targets.buffers()[1].address + targets.offset * 8)
        del targets, offsets, graph
        gc.collect()
        self.assertEqual(result.to_pylist(), [1])

    def test_prepare_and_clone_ownership(self):
        graph = copy.copy(self.cycle()).prepare_incoming()
        gc.collect()
        rank = graph.pagerank()
        self.assertTrue(rank.converged)
        np.testing.assert_allclose(rank.scores(), [1 / 3] * 3)
        self.assertEqual(graph.components("strong").column("component_id").to_pylist(), [0, 0, 0])

    def test_required_incoming_and_invalid_pairs(self):
        with self.assertRaisesRegex(ValueError, "incoming"):
            self.cycle().pagerank()
        with self.assertRaisesRegex(ValueError, "together"):
            Graph.from_csr(1, True, [0], [0, 1], in_targets=[0])

    def test_invalid_csr(self):
        for targets, offsets in [([1], [0, 1]), ([0], [0, 0]), ([0, 0], [0, 2]),
                                 ([None], [0, 1]), ([0], [1, 1])]:
            with self.subTest(targets=targets, offsets=offsets), self.assertRaises(ValueError):
                Graph.from_csr(1, True, targets, offsets)
        with self.assertRaises(ValueError):
            Graph.from_csr(1, True, [0], [0, 1], weights=[float("nan")])
        with self.assertRaises(TypeError):
            Graph.from_csr(1, True, [0.5], [0, 1])

    def test_copy_policy(self):
        targets = pa.array([0], type=pa.uint64())
        offsets = pa.array([0, 1], type=pa.uint64())
        graph = Graph.from_csr(1, True, targets, offsets, copy="always")
        self.assertNotEqual(graph.neighbors(0).buffers()[1].address, targets.buffers()[1].address)
        with self.assertRaises(TypeError):
            Graph.from_csr(1, True, [0], [0, 1], copy="never")
        chunked = pa.chunked_array([pa.array([0], type=pa.uint64()), pa.array([], type=pa.uint64())])
        with self.assertRaises(ValueError):
            Graph.from_csr(1, True, chunked, offsets, copy="never")
        graph = Graph.from_csr(1, True, chunked, offsets)
        self.assertIn("rechunked", graph.import_report["targets"]["operations"])

    def test_tables_views_properties_and_snapshot(self):
        nodes = pa.table({"node_id": ["b", "a", "isolated"], "name": ["B", "A", None]})
        edges = pa.table({"source": ["a", "b"], "target": ["a", "a"],
                          "weight": [5.0, 2.0], "tag": ["loop", "link"]})
        graph = Graph.from_tables(nodes, edges, directed=False, weight="weight")
        self.assertEqual((graph.node_count, graph.edge_count, graph.self_loop_count), (3, 2, 1))
        view = graph.induced([2, 1, 1])
        del graph, nodes, edges
        gc.collect()
        self.assertEqual(view.members, [1, 2])
        compact = view.materialize()
        self.assertEqual(compact.node_properties.column("node_id").to_pylist(), ["a", "isolated"])
        self.assertEqual(compact.edge_properties.column("tag").to_pylist(), ["loop"])
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "snapshot"
            compact.write_snapshot(path)
            loaded = Graph.read_snapshot(path)
            self.assertEqual(loaded.node_properties, compact.node_properties)
            self.assertEqual(loaded.edge_properties, compact.edge_properties)
            self.assertEqual(loaded.self_loop_count, 1)
            self.assertEqual(loaded.pagerank().scores(), compact.pagerank().scores())

    def test_empty_and_isolated(self):
        empty = Graph.from_csr(0, True, [], [0], weights=[])
        self.assertTrue(empty.weighted)
        self.assertEqual(empty.pagerank().scores(), [])
        graph = Graph.from_csr(3, False, [], [0, 0, 0, 0])
        np.testing.assert_allclose(graph.pagerank().scores(), [1 / 3] * 3)
        self.assertEqual(graph.bfs(0).column("distance").to_pylist(), [0.0, None, None])

    def test_cancellation_and_budget(self):
        resources = ExecutionContext(memory_limit=1)
        with self.assertRaises(MemoryError):
            self.cycle().prepare_incoming(resources)
        self.assertEqual(resources.reserved_bytes, 0)
        resources = ExecutionContext()
        resources.cancel()
        with self.assertRaisesRegex(RuntimeError, "cancelled"):
            self.cycle().degrees(context=resources)

    def test_random_pagerank_against_linear_system(self):
        rng = random.Random(781)
        for n in range(1, 12):
            matrix = np.zeros((n, n))
            for u in range(n):
                for v in range(n):
                    if rng.random() < 0.2:
                        matrix[u, v] = rng.choice([0.0, 0.1, 2.0])
            targets, weights, offsets = [], [], [0]
            for u in range(n):
                for v in range(n):
                    if matrix[u, v] != 0:
                        targets.append(v)
                        weights.append(matrix[u, v])
                offsets.append(len(targets))
            graph = Graph.from_csr(n, True, targets, offsets, weights=weights).prepare_incoming()
            personal = np.arange(1, n + 1, dtype=float)
            personal /= personal.sum()
            transition = matrix.copy()
            for u in range(n):
                total = transition[u].sum()
                transition[u] = transition[u] / total if total else personal
            expected = np.linalg.solve(np.eye(n) - 0.85 * transition.T, 0.15 * personal)
            result = graph.pagerank(personalization=personal.tolist(), tolerance=1e-12)
            self.assertTrue(result.converged)
            np.testing.assert_allclose(result.scores(), expected, rtol=1e-9, atol=1e-11)

    def test_duplicate_sparse_sources_and_invalid_values(self):
        graph = self.cycle().prepare_incoming()
        sparse = graph.pagerank(sources=[(0, 2.0), (0, 1.0), (1, 3.0)])
        dense = graph.pagerank(personalization=[3.0, 3.0, 0.0])
        np.testing.assert_allclose(sparse.scores(), dense.scores())
        for sources in [[(0, 0.0)], [(0, float("nan"))], [(99, 1.0)]]:
            with self.assertRaises(ValueError):
                graph.pagerank(sources=sources)


if __name__ == "__main__":
    unittest.main()
