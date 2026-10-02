//! Deterministic RNG support for reproducible shape generation.
//!
//! Wraps `ChaCha8Rng` to provide a seedable, platform-independent
//! random number generator.

use rand::SeedableRng;
use rand_chacha::ChaCha8Rng;

/// Creates a deterministic RNG seeded from the given value.
///
/// Two calls with the same seed will always produce the same sequence,
/// regardless of platform.
#[must_use]
pub(crate) fn create_rng(seed: u64) -> ChaCha8Rng {
    ChaCha8Rng::seed_from_u64(seed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::RngExt;

    #[test]
    fn deterministic_across_calls() {
        let mut rng1 = create_rng(42);
        let mut rng2 = create_rng(42);
        let seq1: Vec<u64> = (0..100).map(|_| rng1.random()).collect();
        let seq2: Vec<u64> = (0..100).map(|_| rng2.random()).collect();
        assert_eq!(seq1, seq2);
    }

    #[test]
    fn different_seeds_differ() {
        let mut rng1 = create_rng(1);
        let mut rng2 = create_rng(2);
        let v1: u64 = rng1.random();
        let v2: u64 = rng2.random();
        assert_ne!(v1, v2);
    }
}
