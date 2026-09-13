//! Validated Arrow CSR storage. No unchecked indexing or foreign pointers.
use crate::{Error, ExecutionContext, Reservation, Result, execution::bytes_for};
use arrow_array::{Array, Float64Array, RecordBatch, UInt64Array};
use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};

/// Dense internal vertex ID; external labels live in a node property table.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct NodeId(pub u64);

/// One sorted, unique, null-free adjacency direction, owning its Arrow buffers.
#[derive(Clone, Debug)]
pub struct CsrAdjacency {
    offsets: UInt64Array,
    targets: UInt64Array,
    weights: Option<Float64Array>,
    has_negative_weights: bool,
}

impl CsrAdjacency {
    /// Validate structure and adopt the arrays without copying their value buffers.
    pub fn try_new(
        n: usize,
        targets: UInt64Array,
        offsets: UInt64Array,
        weights: Option<Float64Array>,
    ) -> Result<Self> {
        if offsets.len() != n.checked_add(1).ok_or(Error::Overflow)? {
            return Err(Error::InvalidGraph("offsets must have n+1 entries".into()));
        }
        if offsets.null_count() != 0 || targets.null_count() != 0 {
            return Err(Error::InvalidGraph("topology cannot contain nulls".into()));
        }
        if offsets.value(0) != 0 || offsets.value(n) != targets.len() as u64 {
            return Err(Error::InvalidGraph(
                "offsets must start at zero and end at target length".into(),
            ));
        }
        if let Some(w) = &weights
            && (w.len() != targets.len()
                || w.null_count() != 0
                || w.values().iter().any(|x| !x.is_finite()))
        {
            return Err(Error::InvalidGraph(
                "weights must be finite, null-free and match targets".into(),
            ));
        }
        for u in 0..n {
            let start = usize::try_from(offsets.value(u)).map_err(|_| Error::Overflow)?;
            let end = usize::try_from(offsets.value(u + 1)).map_err(|_| Error::Overflow)?;
            if start > end || end > targets.len() {
                return Err(Error::InvalidGraph(
                    "offsets must be monotone and in bounds".into(),
                ));
            }
            let row = &targets.values()[start..end];
            if row.iter().any(|&v| v >= n as u64) || row.windows(2).any(|pair| pair[0] >= pair[1]) {
                return Err(Error::InvalidGraph(
                    "neighbors must be in range, sorted and unique".into(),
                ));
            }
        }
        let has_negative_weights = weights
            .as_ref()
            .is_some_and(|w| w.values().iter().any(|x| *x < 0.0));
        Ok(Self {
            has_negative_weights,
            offsets,
            targets,
            weights,
        })
    }
    /// Number of vertices.
    pub fn node_count(&self) -> usize {
        self.offsets.len() - 1
    }
    /// Number of adjacency slots, including one slot per self-loop.
    pub fn slot_count(&self) -> usize {
        self.targets.len()
    }
    /// CSR offsets, relative to the logical target array (including sliced inputs).
    pub fn offsets(&self) -> &UInt64Array {
        &self.offsets
    }
    /// CSR neighbor IDs.
    pub fn targets(&self) -> &UInt64Array {
        &self.targets
    }
    /// Explicit weights; an empty array still denotes a weighted graph.
    pub fn weights(&self) -> Option<&Float64Array> {
        self.weights.as_ref()
    }
    /// Whether any stored weight is negative, cached when validating immutable buffers.
    pub fn has_negative_weights(&self) -> bool {
        self.has_negative_weights
    }
    /// Borrow one neighborhood, checking the vertex ID.
    pub fn neighbors(&self, node: NodeId) -> Result<&[u64]> {
        let range = self.range(node)?;
        Ok(&self.targets.values()[range])
    }
    /// Iterate target and weight pairs, synthesizing unit weights if unweighted.
    pub fn edges(&self, node: NodeId) -> Result<impl Iterator<Item = (u64, f64)> + '_> {
        Ok(self
            .range(node)?
            .map(|slot| (self.targets.value(slot), self.weight_at(slot))))
    }
    /// Checked slot range for a vertex.
    pub fn range(&self, node: NodeId) -> Result<std::ops::Range<usize>> {
        let u = usize::try_from(node.0).map_err(|_| Error::InvalidNode(node.0))?;
        if u >= self.node_count() {
            return Err(Error::InvalidNode(node.0));
        }
        Ok(self.offsets.value(u) as usize..self.offsets.value(u + 1) as usize)
    }
    fn weight_at(&self, slot: usize) -> f64 {
        self.weights.as_ref().map_or(1.0, |w| w.value(slot))
    }
    fn slot(&self, u: usize, v: u64) -> Option<usize> {
        let start = self.offsets.value(u) as usize;
        let end = self.offsets.value(u + 1) as usize;
        self.targets.values()[start..end]
            .binary_search(&v)
            .ok()
            .map(|i| i + start)
    }
}

static NEXT_ID: AtomicU64 = AtomicU64::new(1);

#[derive(Debug)]
struct Snapshot {
    id: u64,
    directed: bool,
    edges: usize,
    loops: usize,
    out: CsrAdjacency,
    incoming: Option<CsrAdjacency>,
    nodes: Option<RecordBatch>,
    edge_properties: Option<RecordBatch>,
    slot_edges: Option<UInt64Array>,
    reservations: Vec<Arc<Reservation>>,
}

/// An immutable graph. Clones share topology, properties, and prepared-index owners.
#[derive(Clone, Debug)]
pub struct Graph(Arc<Snapshot>);

impl Graph {
    /// Construct a graph; validate reciprocal undirected edges or a supplied transpose.
    pub fn try_new(
        directed: bool,
        out: CsrAdjacency,
        incoming: Option<CsrAdjacency>,
    ) -> Result<Self> {
        if !directed && incoming.is_some() {
            return Err(Error::InvalidGraph(
                "undirected graphs share outgoing adjacency; omit incoming".into(),
            ));
        }
        let reverse = if directed {
            incoming.as_ref()
        } else {
            Some(&out)
        };
        if let Some(reverse) = reverse {
            if reverse.node_count() != out.node_count()
                || reverse.slot_count() != out.slot_count()
                || reverse.weights.is_some() != out.weights.is_some()
            {
                return Err(Error::InvalidGraph(
                    "incoming adjacency shape or weightedness differs".into(),
                ));
            }
            for u in 0..out.node_count() {
                for slot in out.range(NodeId(u as u64))? {
                    let v = out.targets.value(slot) as usize;
                    let rev = reverse.slot(v, u as u64).ok_or_else(|| {
                        Error::InvalidGraph("missing reciprocal/transpose edge".into())
                    })?;
                    if out.weight_at(slot) != reverse.weight_at(rev) {
                        return Err(Error::InvalidGraph(
                            "reciprocal/transpose weights differ".into(),
                        ));
                    }
                }
            }
        }
        let loops = (0..out.node_count())
            .filter(|&u| out.slot(u, u as u64).is_some())
            .count();
        let edges = if directed {
            out.slot_count()
        } else {
            out.slot_count().checked_add(loops).ok_or(Error::Overflow)? / 2
        };
        Ok(Self(Arc::new(Snapshot {
            id: NEXT_ID.fetch_add(1, Ordering::Relaxed),
            directed,
            edges,
            loops,
            out,
            incoming,
            nodes: None,
            edge_properties: None,
            slot_edges: None,
            reservations: vec![],
        })))
    }
    /// Process-local snapshot identity, shared by clones and prepared handles.
    pub fn snapshot_id(&self) -> u64 {
        self.0.id
    }
    /// Number of dense internal vertices.
    pub fn node_count(&self) -> usize {
        self.0.out.node_count()
    }
    /// Number of logical edges, not adjacency slots.
    pub fn edge_count(&self) -> usize {
        self.0.edges
    }
    /// Number of self-loops.
    pub fn self_loop_count(&self) -> usize {
        self.0.loops
    }
    /// Whether edges are directed.
    pub fn is_directed(&self) -> bool {
        self.0.directed
    }
    /// Whether explicit weights exist, including for empty graphs.
    pub fn is_weighted(&self) -> bool {
        self.0.out.weights.is_some()
    }
    /// Outgoing topology.
    pub fn outgoing(&self) -> &CsrAdjacency {
        &self.0.out
    }
    /// Incoming topology, or a clear capability error instead of an empty neighborhood.
    pub fn incoming(&self) -> Result<&CsrAdjacency> {
        if !self.is_directed() {
            Ok(&self.0.out)
        } else {
            self.0.incoming.as_ref().ok_or(Error::MissingIncoming)
        }
    }
    /// Node properties aligned to internal vertex order.
    pub fn node_properties(&self) -> Option<&RecordBatch> {
        self.0.nodes.as_ref()
    }
    /// Properties aligned to logical edge IDs.
    pub fn edge_properties(&self) -> Option<&RecordBatch> {
        self.0.edge_properties.as_ref()
    }
    /// Outgoing slot to logical edge property row mapping.
    pub fn slot_edge_ids(&self) -> Option<&UInt64Array> {
        self.0.slot_edges.as_ref()
    }
    /// Check whether an edge exists; missing vertices are errors.
    pub fn has_edge(&self, u: NodeId, v: NodeId) -> Result<bool> {
        self.outgoing().range(v)?;
        Ok(self.outgoing().neighbors(u)?.binary_search(&v.0).is_ok())
    }
    /// Attach immutable properties after validating lengths and edge associations.
    pub fn with_properties(
        &self,
        nodes: Option<RecordBatch>,
        edges: Option<RecordBatch>,
        slot_edges: Option<UInt64Array>,
    ) -> Result<Self> {
        if nodes
            .as_ref()
            .is_some_and(|b| b.num_rows() != self.node_count())
            || edges
                .as_ref()
                .is_some_and(|b| b.num_rows() != self.edge_count())
        {
            return Err(Error::InvalidGraph(
                "property row counts differ from graph".into(),
            ));
        }
        if edges.is_some() && slot_edges.is_none() {
            return Err(Error::InvalidGraph(
                "edge properties require slot mapping".into(),
            ));
        }
        if let Some(ids) = &slot_edges {
            if ids.len() != self.outgoing().slot_count() || ids.null_count() != 0 {
                return Err(Error::InvalidGraph("invalid slot mapping shape".into()));
            }
            let mut seen = vec![false; self.edge_count()];
            for u in 0..self.node_count() {
                for slot in self.outgoing().range(NodeId(u as u64))? {
                    let id = ids.value(slot);
                    if id >= self.edge_count() as u64 {
                        return Err(Error::InvalidGraph("edge mapping out of range".into()));
                    }
                    let v = self.outgoing().targets.value(slot);
                    if !self.is_directed() && v < u as u64 {
                        let reverse = self
                            .outgoing()
                            .slot(v as usize, u as u64)
                            .ok_or_else(|| Error::InvalidGraph("missing reciprocal edge".into()))?;
                        if ids.value(reverse) != id {
                            return Err(Error::InvalidGraph(
                                "reciprocal edge property IDs differ".into(),
                            ));
                        }
                    } else if std::mem::replace(&mut seen[id as usize], true) {
                        return Err(Error::InvalidGraph(
                            "logical edges share a property row".into(),
                        ));
                    }
                }
            }
            if seen.iter().any(|x| !x) {
                return Err(Error::InvalidGraph("unused edge property row".into()));
            }
        }
        Ok(Self(Arc::new(Snapshot {
            id: NEXT_ID.fetch_add(1, Ordering::Relaxed),
            directed: self.0.directed,
            edges: self.0.edges,
            loops: self.0.loops,
            out: self.0.out.clone(),
            incoming: self.0.incoming.clone(),
            nodes,
            edge_properties: edges,
            slot_edges,
            reservations: self.0.reservations.clone(),
        })))
    }
    /// Build incoming CSR once per returned handle, charging its retained and scratch buffers.
    pub fn prepare_incoming(&self, ctx: &ExecutionContext) -> Result<Self> {
        ctx.check()?;
        if self.incoming().is_ok() {
            return Ok(self.clone());
        }
        let n = self.node_count();
        let a = self.outgoing().slot_count();
        let size = bytes_for(n.checked_add(1).ok_or(Error::Overflow)?, 8)?
            .checked_add(bytes_for(a, if self.is_weighted() { 16 } else { 8 })?)
            .ok_or(Error::Overflow)?;
        let reservation = Arc::new(ctx.reserve(size)?);
        let _scratch = ctx.reserve(bytes_for(n, 8)?)?;
        let mut offsets = vec![0u64; n + 1];
        for (i, &v) in self.outgoing().targets.values().iter().enumerate() {
            if i % 4096 == 0 {
                ctx.check()?;
            }
            offsets[v as usize + 1] += 1;
        }
        for u in 0..n {
            offsets[u + 1] += offsets[u];
        }
        let mut cursor = offsets[..n].to_vec();
        let mut targets = vec![0u64; a];
        let mut weights = self.is_weighted().then(|| vec![0.0; a]);
        for u in 0..n {
            ctx.check()?;
            for (i, (v, w)) in self.outgoing().edges(NodeId(u as u64))?.enumerate() {
                if i % 4096 == 0 {
                    ctx.check()?;
                }
                let slot = cursor[v as usize] as usize;
                cursor[v as usize] += 1;
                targets[slot] = u as u64;
                if let Some(weights) = &mut weights {
                    weights[slot] = w;
                }
            }
        }
        let incoming =
            CsrAdjacency::try_new(n, targets.into(), offsets.into(), weights.map(Into::into))?;
        let mut reservations = self.0.reservations.clone();
        reservations.push(reservation);
        Ok(Self(Arc::new(Snapshot {
            id: self.0.id,
            directed: true,
            edges: self.0.edges,
            loops: self.0.loops,
            out: self.0.out.clone(),
            incoming: Some(incoming),
            nodes: self.0.nodes.clone(),
            edge_properties: self.0.edge_properties.clone(),
            slot_edges: self.0.slot_edges.clone(),
            reservations,
        })))
    }
}
