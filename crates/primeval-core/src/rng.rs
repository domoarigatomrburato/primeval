//! Deterministic RNG support for reproducible shape generation.
//!
//! Wraps `ChaCha8Rng` to provide a seedable, platform-independent
//! random number generator.

use rand::SeedableRng;
use rand_chacha::ChaCha8Rng;

/// Creates a deterministic RNG seeded from the given value, for tests and
/// benches that drive a [`crate::worker::WorkerCtx`] directly.
///
/// Two calls with the same seed will always produce the same sequence,
/// regardless of platform.
#[cfg(any(test, feature = "bench"))]
#[must_use]
pub(crate) fn create_rng(seed: u64) -> ChaCha8Rng {
    ChaCha8Rng::seed_from_u64(seed)
}

/// Creates the RNG for search round `round` of step `step` under `seed`.
///
/// The 256-bit ChaCha key is `seed` followed by `step` (both little-endian,
/// the remaining 16 bytes zero) and the ChaCha stream is `round`. Distinct
/// `(seed, step, round)` triples therefore select distinct key and stream
/// pairs, so no two rounds share or overlap a stream, and the derivation
/// does no arithmetic that could overflow.
#[must_use]
pub(crate) fn round_rng(seed: u64, step: u64, round: u64) -> ChaCha8Rng {
    let mut key = [0_u8; 32];
    key[..8].copy_from_slice(&seed.to_le_bytes());
    key[8..16].copy_from_slice(&step.to_le_bytes());
    let mut rng = ChaCha8Rng::from_seed(key);
    rng.set_stream(round);
    rng
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

    fn first_words(mut rng: ChaCha8Rng) -> Vec<u64> {
        (0..8).map(|_| rng.random()).collect()
    }

    #[test]
    fn round_rngs_are_reproducible_and_do_not_overlap() {
        assert_eq!(
            first_words(round_rng(42, 3, 5)),
            first_words(round_rng(42, 3, 5))
        );

        let triples = [
            (42, 0, 0),
            (42, 0, 1),
            (43, 0, 0),
            (42, 1, 0),
            (43, 1, 0),
            (0, 0, 0),
            (u64::MAX, 0, 0),
            (u64::MAX, u64::MAX, 15),
        ];
        let streams: Vec<Vec<u64>> = triples
            .iter()
            .map(|&(seed, step, round)| first_words(round_rng(seed, step, round)))
            .collect();
        for (i, left) in streams.iter().enumerate() {
            for (j, right) in streams.iter().enumerate().skip(i + 1) {
                assert_ne!(left, right, "{:?} and {:?}", triples[i], triples[j]);
            }
        }
    }
}
