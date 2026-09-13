//! Cooperative cancellation and reservations for explicitly accounted working buffers.
use crate::{Error, Result};
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};

#[derive(Debug)]
struct State {
    limit: usize,
    used: AtomicUsize,
    peak: AtomicUsize,
    cancelled: AtomicBool,
}

/// Shared resource budget. Imported buffers and allocator overhead are not charged.
#[derive(Clone, Debug)]
pub struct ExecutionContext(Arc<State>);

impl Default for ExecutionContext {
    fn default() -> Self {
        Self::new(usize::MAX)
    }
}

impl ExecutionContext {
    /// Create a context with a byte limit for tracked scratch/index allocations.
    pub fn new(limit: usize) -> Self {
        Self(Arc::new(State {
            limit,
            used: AtomicUsize::new(0),
            peak: AtomicUsize::new(0),
            cancelled: AtomicBool::new(false),
        }))
    }
    /// Cancel all operations using this context; cancellation is permanent.
    pub fn cancel(&self) {
        self.0.cancelled.store(true, Ordering::Relaxed);
    }
    /// Check cancellation at an operation or traversal boundary.
    pub fn check(&self) -> Result<()> {
        if self.0.cancelled.load(Ordering::Relaxed) {
            Err(Error::Cancelled)
        } else {
            Ok(())
        }
    }
    /// Current explicitly reserved bytes.
    pub fn reserved_bytes(&self) -> usize {
        self.0.used.load(Ordering::Relaxed)
    }
    /// Peak explicitly reserved bytes since context creation.
    pub fn peak_bytes(&self) -> usize {
        self.0.peak.load(Ordering::Relaxed)
    }
    /// Atomically reserve bytes before allocating a working buffer.
    pub fn reserve(&self, bytes: usize) -> Result<Reservation> {
        self.check()?;
        let mut used = self.reserved_bytes();
        loop {
            let available = self.0.limit.saturating_sub(used);
            if bytes > available {
                return Err(Error::MemoryLimit {
                    requested: bytes,
                    available,
                });
            }
            let next = used.checked_add(bytes).ok_or(Error::Overflow)?;
            match self.0.used.compare_exchange_weak(
                used,
                next,
                Ordering::Relaxed,
                Ordering::Relaxed,
            ) {
                Ok(_) => {
                    self.0.peak.fetch_max(next, Ordering::Relaxed);
                    return Ok(Reservation {
                        context: self.clone(),
                        bytes,
                    });
                }
                Err(actual) => used = actual,
            }
        }
    }
}

/// A reservation follows the allocation's lifetime and releases on error or drop.
#[derive(Debug)]
pub struct Reservation {
    context: ExecutionContext,
    bytes: usize,
}
impl Drop for Reservation {
    fn drop(&mut self) {
        self.context.0.used.fetch_sub(self.bytes, Ordering::Relaxed);
    }
}

/// Checked byte-size calculation for fixed-width buffers.
pub fn bytes_for(count: usize, width: usize) -> Result<usize> {
    count.checked_mul(width).ok_or(Error::Overflow)
}
