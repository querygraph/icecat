//! Python bindings for immutable graphs.
use arrow_array::{Float64Array, RecordBatch, UInt64Array};
use arrow_select::concat::concat_batches;
use icebug_algorithms::{PageRankOptions, Personalization};
use icebug_core::{CsrAdjacency, Error, ExecutionContext, Graph, InducedView, NodeId};
use pyo3::{
    exceptions::{PyMemoryError, PyRuntimeError, PyTypeError, PyValueError},
    prelude::*,
};
use pyo3_arrow::{PyArray, PyRecordBatch, PyTable};
use std::sync::Arc;

fn error(e: Error) -> PyErr {
    match e {
        Error::MemoryLimit { .. } => PyMemoryError::new_err(e.to_string()),
        Error::Cancelled => PyRuntimeError::new_err(e.to_string()),
        _ => PyValueError::new_err(e.to_string()),
    }
}
fn u64_array(array: PyArray) -> PyResult<UInt64Array> {
    array
        .array()
        .as_any()
        .downcast_ref::<UInt64Array>()
        .cloned()
        .ok_or_else(|| PyTypeError::new_err("expected UInt64 Arrow array"))
}
fn f64_array(array: PyArray) -> PyResult<Float64Array> {
    array
        .array()
        .as_any()
        .downcast_ref::<Float64Array>()
        .cloned()
        .ok_or_else(|| PyTypeError::new_err("expected Float64 Arrow array"))
}
fn context(value: Option<&PyContext>) -> ExecutionContext {
    value.map_or_else(ExecutionContext::default, |c| c.inner.clone())
}
fn export(py: Python<'_>, batch: RecordBatch) -> PyResult<Bound<'_, PyAny>> {
    PyRecordBatch::new(batch).into_pyarrow(py)
}

/// Shared cancellation and tracked working-memory budget.
#[pyclass(name = "ExecutionContext", frozen, module = "icebug_rust._native")]
struct PyContext {
    inner: ExecutionContext,
}
#[pymethods]
impl PyContext {
    #[new]
    #[pyo3(signature=(memory_limit=None))]
    fn new(memory_limit: Option<usize>) -> Self {
        Self {
            inner: ExecutionContext::new(memory_limit.unwrap_or(usize::MAX)),
        }
    }
    fn cancel(&self) {
        self.inner.cancel();
    }
    #[getter]
    fn reserved_bytes(&self) -> usize {
        self.inner.reserved_bytes()
    }
    #[getter]
    fn peak_bytes(&self) -> usize {
        self.inner.peak_bytes()
    }
}

#[pyclass(name = "Graph", frozen, module = "icebug_rust._native")]
struct PyGraph {
    inner: Graph,
}
#[pymethods]
impl PyGraph {
    #[staticmethod]
    #[pyo3(signature=(n,directed,targets,offsets,weights=None,in_targets=None,in_offsets=None,in_weights=None))]
    #[allow(clippy::too_many_arguments)]
    fn from_csr(
        py: Python<'_>,
        n: usize,
        directed: bool,
        targets: PyArray,
        offsets: PyArray,
        weights: Option<PyArray>,
        in_targets: Option<PyArray>,
        in_offsets: Option<PyArray>,
        in_weights: Option<PyArray>,
    ) -> PyResult<Self> {
        let targets = u64_array(targets)?;
        let offsets = u64_array(offsets)?;
        let weights = weights.map(f64_array).transpose()?;
        let incoming = match (in_targets, in_offsets) {
            (Some(t), Some(o)) => Some((
                u64_array(t)?,
                u64_array(o)?,
                in_weights.map(f64_array).transpose()?,
            )),
            (None, None) if in_weights.is_none() => None,
            _ => {
                return Err(PyValueError::new_err(
                    "incoming targets and offsets must be provided together; weights require incoming topology",
                ));
            }
        };
        py.detach(move || {
            let out = CsrAdjacency::try_new(n, targets, offsets, weights)?;
            let incoming = incoming
                .map(|(t, o, w)| CsrAdjacency::try_new(n, t, o, w))
                .transpose()?;
            Graph::try_new(directed, out, incoming).map(|inner| Self { inner })
        })
        .map_err(error)
    }
    #[staticmethod]
    #[pyo3(signature=(nodes,edges,directed=true,weight=None,context=None))]
    fn from_tables(
        py: Python<'_>,
        nodes: PyTable,
        edges: PyTable,
        directed: bool,
        weight: Option<String>,
        context: Option<&PyContext>,
    ) -> PyResult<Self> {
        let ctx = self::context(context);
        let (nodes, node_schema) = nodes.into_inner();
        let (mut edges, edge_schema) = edges.into_inner();
        py.detach(move || {
            let nodes = concat_batches(&node_schema, &nodes)?;
            if edges.is_empty() {
                edges.push(RecordBatch::new_empty(edge_schema));
            }
            icebug_core::from_tables(nodes, &edges, weight.as_deref(), directed, &ctx)
                .map(|inner| Self { inner })
        })
        .map_err(error)
    }
    fn node_count(&self) -> usize {
        self.inner.node_count()
    }
    fn edge_count(&self) -> usize {
        self.inner.edge_count()
    }
    fn self_loop_count(&self) -> usize {
        self.inner.self_loop_count()
    }
    fn is_directed(&self) -> bool {
        self.inner.is_directed()
    }
    fn is_weighted(&self) -> bool {
        self.inner.is_weighted()
    }
    fn snapshot_id(&self) -> u64 {
        self.inner.snapshot_id()
    }
    fn clone_snapshot(&self) -> Self {
        Self {
            inner: self.inner.clone(),
        }
    }
    fn has_edge(&self, u: u64, v: u64) -> PyResult<bool> {
        self.inner.has_edge(NodeId(u), NodeId(v)).map_err(error)
    }
    fn neighbors<'py>(&self, py: Python<'py>, u: u64) -> PyResult<Bound<'py, PyAny>> {
        let range = self.inner.outgoing().range(NodeId(u)).map_err(error)?;
        let array = self
            .inner
            .outgoing()
            .targets()
            .slice(range.start, range.len());
        PyArray::from_array_ref(Arc::new(array))
            .to_pyarrow(py)
            .map(|a| a.clone().unbind().into_bound(py))
    }
    #[pyo3(signature=(context=None))]
    fn prepare_incoming(&self, py: Python<'_>, context: Option<&PyContext>) -> PyResult<Self> {
        let ctx = self::context(context);
        let graph = self.inner.clone();
        py.detach(move || graph.prepare_incoming(&ctx).map(|inner| Self { inner }))
            .map_err(error)
    }
    fn induced(&self, members: Vec<u64>) -> PyResult<PyView> {
        InducedView::new(self.inner.clone(), members)
            .map(|inner| PyView { inner })
            .map_err(error)
    }
    #[pyo3(signature=(kind,source=None,incoming=false,weighted=false,context=None))]
    fn run<'py>(
        &self,
        py: Python<'py>,
        kind: &str,
        source: Option<u64>,
        incoming: bool,
        weighted: bool,
        context: Option<&PyContext>,
    ) -> PyResult<Bound<'py, PyAny>> {
        let ctx = self::context(context);
        let g = self.inner.clone();
        let batch = py
            .detach(move || match kind {
                "degree" => icebug_algorithms::degrees(&g, incoming, weighted, &ctx),
                "bfs" => icebug_algorithms::bfs(
                    &g,
                    NodeId(source.ok_or_else(|| Error::InvalidOption("source required".into()))?),
                    &ctx,
                ),
                "dijkstra" => icebug_algorithms::dijkstra(
                    &g,
                    NodeId(source.ok_or_else(|| Error::InvalidOption("source required".into()))?),
                    &ctx,
                ),
                "cc" => icebug_algorithms::connected_components(&g, &ctx),
                "wcc" => icebug_algorithms::weakly_connected_components(&g, &ctx),
                "scc" => icebug_algorithms::strongly_connected_components(&g, &ctx),
                _ => Err(Error::InvalidOption("unknown algorithm".into())),
            })
            .map_err(error)?;
        export(py, batch)
    }
    #[pyo3(signature=(damping=0.85,tolerance=1e-8,max_iterations=1000,dense=None,sparse=None,context=None))]
    #[allow(clippy::too_many_arguments)]
    fn pagerank<'py>(
        &self,
        py: Python<'py>,
        damping: f64,
        tolerance: f64,
        max_iterations: usize,
        dense: Option<Vec<f64>>,
        sparse: Option<Vec<(u64, f64)>>,
        context: Option<&PyContext>,
    ) -> PyResult<(Bound<'py, PyAny>, usize, f64, bool)> {
        if dense.is_some() && sparse.is_some() {
            return Err(PyValueError::new_err(
                "choose dense or sparse personalization",
            ));
        }
        let personalization = if let Some(d) = dense {
            Personalization::Dense(d)
        } else if let Some(s) = sparse {
            Personalization::Sparse(s.into_iter().map(|(u, w)| (NodeId(u), w)).collect())
        } else {
            Personalization::Uniform
        };
        let options = PageRankOptions {
            damping,
            tolerance,
            max_iterations,
            personalization,
        };
        let ctx = self::context(context);
        let graph = self.inner.clone();
        let result = py
            .detach(move || icebug_algorithms::pagerank(&graph, &options, &ctx))
            .map_err(error)?;
        Ok((
            export(py, result.scores)?,
            result.iterations,
            result.residual,
            result.converged,
        ))
    }
    fn node_properties<'py>(&self, py: Python<'py>) -> PyResult<Option<Bound<'py, PyAny>>> {
        self.inner
            .node_properties()
            .map(|b| export(py, b.clone()))
            .transpose()
    }
    fn edge_properties<'py>(&self, py: Python<'py>) -> PyResult<Option<Bound<'py, PyAny>>> {
        self.inner
            .edge_properties()
            .map(|b| export(py, b.clone()))
            .transpose()
    }
    fn write_snapshot(&self, py: Python<'_>, path: String) -> PyResult<()> {
        let graph = self.inner.clone();
        py.detach(move || icebug_io::write_snapshot(&graph, path))
            .map_err(error)
    }
    #[staticmethod]
    #[pyo3(signature=(path,max_encoded_bytes=1073741824))]
    fn read_snapshot(py: Python<'_>, path: String, max_encoded_bytes: u64) -> PyResult<Self> {
        py.detach(move || {
            icebug_io::read_snapshot(path, max_encoded_bytes).map(|inner| Self { inner })
        })
        .map_err(error)
    }
}

#[pyclass(name = "InducedView", frozen, module = "icebug_rust._native")]
struct PyView {
    inner: InducedView,
}
#[pymethods]
impl PyView {
    fn members(&self) -> Vec<u64> {
        self.inner.members().to_vec()
    }
    fn neighbors(&self, u: u64) -> PyResult<Vec<u64>> {
        self.inner.neighbors(NodeId(u)).map_err(error)
    }
    fn materialize(&self, py: Python<'_>) -> PyResult<PyGraph> {
        let view = self.inner.clone();
        py.detach(move || view.materialize().map(|inner| PyGraph { inner }))
            .map_err(error)
    }
}

#[pymodule]
fn _native(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<PyContext>()?;
    m.add_class::<PyGraph>()?;
    m.add_class::<PyView>()?;
    Ok(())
}
