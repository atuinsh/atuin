use rand::SeedableRng;
use rand::rngs::StdRng;
use time::OffsetDateTime;
use time::macros::datetime;

/// Utility used to create a benchmarking context.
///
/// Generally useful for establishing stable and robust benchmarks. It's an anti-pattern to use a
/// bare `rand` accessor as that causes benchmarks to be non-deterministic.
pub struct BenchCtx {
    rng: StdRng,
}

impl BenchCtx {
    // Changing any of these values will result in irreproducible benchmarks.
    const SEED_RNG: u64 = 42;
    const SEED_NOW: OffsetDateTime = datetime!(2026-01-01 12:59:59 -5);

    pub fn new() -> Self {
        Self {
            rng: StdRng::seed_from_u64(Self::SEED_RNG),
        }
    }

    /// Access a random number generator which is stable across the given benchmark.
    pub fn rng(&mut self) -> &mut StdRng {
        &mut self.rng
    }

    /// Get the timestamp recognized as the current timestamp in the stable benchmarking
    /// environment.
    ///
    /// Using the standard library will provide timestamps which are not stable and will result in
    /// irreproducible benchmarks.
    pub fn now(&self) -> OffsetDateTime {
        Self::SEED_NOW
    }
}
