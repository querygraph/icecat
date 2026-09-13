//! Immutable Arrow adjacency projected once from Grust's validated endpoints.
//! Row order follows Grust edge order; parallel edges remain separate slots.
use arrow_array::{Float64Array, UInt64Array};

pub(crate) struct Adjacency {
    pub offsets: UInt64Array,
    pub targets: UInt64Array,
    pub weights: Float64Array,
}
impl Adjacency {
    /// Build both orientations together, after releasing the temporary Grust
    /// string map and per-node edge lists. Prefix sums retain Grust row order.
    pub fn pair(n: usize, endpoints: &[(usize, usize)], weights: &[f64]) -> (Self, Self) {
        let mut out = vec![0u64; n + 1];
        let mut inc = vec![0u64; n + 1];
        for &(u, v) in endpoints {
            out[u + 1] += 1;
            inc[v + 1] += 1;
        }
        for i in 0..n {
            out[i + 1] += out[i];
            inc[i + 1] += inc[i];
        }
        let mut out_pos = out[..n].to_vec();
        let mut in_pos = inc[..n].to_vec();
        let m = endpoints.len();
        let (mut out_targets, mut in_targets) = (vec![0u64; m], vec![0u64; m]);
        let (mut out_weights, mut in_weights) = (vec![0.; m], vec![0.; m]);
        for (&(u, v), &w) in endpoints.iter().zip(weights) {
            let a = out_pos[u] as usize;
            let b = in_pos[v] as usize;
            out_targets[a] = v as u64;
            in_targets[b] = u as u64;
            out_weights[a] = w;
            in_weights[b] = w;
            out_pos[u] += 1;
            in_pos[v] += 1;
        }
        (
            Self {
                offsets: out.into(),
                targets: out_targets.into(),
                weights: out_weights.into(),
            },
            Self {
                offsets: inc.into(),
                targets: in_targets.into(),
                weights: in_weights.into(),
            },
        )
    }
    #[inline]
    pub fn range(&self, u: usize) -> std::ops::Range<usize> {
        self.offsets.value(u) as usize..self.offsets.value(u + 1) as usize
    }
    #[inline]
    pub fn neighbors(&self, u: usize) -> &[u64] {
        &self.targets.values()[self.range(u)]
    }
}
