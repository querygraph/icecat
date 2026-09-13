//! Immutable node-induced selections with explicit compact materialization.
use crate::{
    Error, Graph, NodeId, Result,
    builder::{Edge, from_edges, take_batch},
};
use arrow_array::UInt64Array;

/// A fixed selection retaining its base; IDs are compact ranks in ascending base-ID order.
#[derive(Clone, Debug)]
pub struct InducedView {
    base: Graph,
    members: Vec<u64>,
}
impl InducedView {
    /// Create a selection. Repeated IDs are ignored; unknown IDs fail.
    pub fn new(base: Graph, mut members: Vec<u64>) -> Result<Self> {
        if let Some(&bad) = members.iter().find(|&&u| u >= base.node_count() as u64) {
            return Err(Error::InvalidNode(bad));
        }
        members.sort_unstable();
        members.dedup();
        Ok(Self { base, members })
    }
    /// Mapping from compact IDs to base IDs.
    pub fn members(&self) -> &[u64] {
        &self.members
    }
    /// Retained base snapshot.
    pub fn base(&self) -> &Graph {
        &self.base
    }
    /// Filter outgoing neighbors, returning compact IDs. This scans the base row.
    pub fn neighbors(&self, u: NodeId) -> Result<Vec<u64>> {
        let base_u = *self
            .members
            .get(usize::try_from(u.0).map_err(|_| Error::InvalidNode(u.0))?)
            .ok_or(Error::InvalidNode(u.0))?;
        Ok(self
            .base
            .outgoing()
            .neighbors(NodeId(base_u))?
            .iter()
            .filter_map(|v| self.members.binary_search(v).ok().map(|i| i as u64))
            .collect())
    }
    /// Build compact CSR with matching node and logical-edge properties.
    pub fn materialize(&self) -> Result<Graph> {
        let mut edges = vec![];
        let mut property_rows = vec![];
        for (u, &base_u) in self.members.iter().enumerate() {
            for (slot, (base_v, weight)) in self
                .base
                .outgoing()
                .range(NodeId(base_u))?
                .zip(self.base.outgoing().edges(NodeId(base_u))?)
            {
                if let Ok(v) = self.members.binary_search(&base_v) {
                    if !self.base.is_directed() && v < u {
                        continue;
                    }
                    edges.push(Edge {
                        source: u as u64,
                        target: v as u64,
                        weight,
                    });
                    if let Some(ids) = self.base.slot_edge_ids() {
                        property_rows.push(ids.value(slot));
                    }
                }
            }
        }
        let graph = from_edges(
            self.members.len(),
            self.base.is_directed(),
            self.base.is_weighted(),
            &edges,
        )?;
        let nodes = self
            .base
            .node_properties()
            .map(|b| take_batch(b, &self.members))
            .transpose()?;
        let props = self
            .base
            .edge_properties()
            .map(|b| take_batch(b, &property_rows))
            .transpose()?;
        // Edge list order is canonical (source, target); associate reverse slots with the same row.
        let ids = if props.is_some() {
            let mut ids = vec![0; graph.outgoing().slot_count()];
            for (id, e) in edges.iter().enumerate() {
                let mut assign = |u, v| -> Result<()> {
                    let range = graph.outgoing().range(NodeId(u))?;
                    let pos = graph
                        .outgoing()
                        .neighbors(NodeId(u))?
                        .binary_search(&v)
                        .map_err(|_| Error::InvalidGraph("materialized edge missing".into()))?;
                    ids[range.start + pos] = id as u64;
                    Ok(())
                };
                assign(e.source, e.target)?;
                if !graph.is_directed() && e.source != e.target {
                    assign(e.target, e.source)?;
                }
            }
            Some(UInt64Array::from(ids))
        } else {
            None
        };
        graph.with_properties(nodes, props, ids)
    }
}
