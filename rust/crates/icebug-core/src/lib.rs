//! Immutable, validated CSR graphs with Arrow-owned buffers.
pub mod builder;
pub mod execution;
pub mod graph;
pub mod view;

pub use builder::{Edge, from_edges, from_tables};
pub use execution::{ExecutionContext, Reservation};
pub use graph::{CsrAdjacency, Graph, NodeId};
pub use view::InducedView;

/// A failure at a checked graph, algorithm, or resource boundary.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("invalid graph: {0}")]
    InvalidGraph(String),
    #[error("invalid option: {0}")]
    InvalidOption(String),
    #[error("node {0} does not exist")]
    InvalidNode(u64),
    #[error("incoming adjacency is required; call prepare_incoming first")]
    MissingIncoming,
    #[error("operation cancelled")]
    Cancelled,
    #[error("memory budget exceeded: requested {requested} bytes, available {available}")]
    MemoryLimit { requested: usize, available: usize },
    #[error("size or numeric overflow")]
    Overflow,
    #[error(transparent)]
    Arrow(#[from] arrow_schema::ArrowError),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

/// Result of a checked graph operation.
pub type Result<T> = std::result::Result<T, Error>;
