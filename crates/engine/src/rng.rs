//! The random source trig probability draws from.
//!
//! Injectable, as it is in `js/midi.js`, so the condition tests are
//! deterministic — that is the whole reason the JS threads an `rng` parameter
//! through `shouldPlay` rather than calling `Math.random` inline, and it ports
//! unchanged.
//!
//! No `rand` dependency: the engine thread must not allocate (PLAN.md §4), the
//! quality bar for "does this trig fire at 60%" is low, and a hand-rolled
//! generator is one that can be seeded identically across runs for a bug report.

/// A source of uniform values in `[0, 1)`, matching `Math.random`'s contract.
pub trait Rng {
    fn next_f64(&mut self) -> f64;
}

/// xorshift64*, seeded. Fast, allocation-free, and reproducible from its seed.
///
/// Deliberately not cryptographic and deliberately not `rand`: what this decides
/// is whether a hi-hat fires this bar.
#[derive(Debug, Clone)]
pub struct XorShift64 {
    state: u64,
}

impl XorShift64 {
    /// A zero seed would leave xorshift stuck at zero forever, so it is mapped
    /// to a non-zero constant rather than rejected — a caller passing 0 wants a
    /// default, not an error.
    pub fn new(seed: u64) -> Self {
        Self {
            state: if seed == 0 { 0x9e37_79b9_7f4a_7c15 } else { seed },
        }
    }
}

impl Rng for XorShift64 {
    fn next_f64(&mut self) -> f64 {
        let mut x = self.state;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.state = x;
        let v = x.wrapping_mul(0x2545_F491_4F6C_DD1D);
        // Top 53 bits: exactly the mantissa of an f64, so every value is
        // representable and the distribution has no gaps.
        (v >> 11) as f64 / (1u64 << 53) as f64
    }
}

/// An `Rng` reading from a fixed script, for tests that need a named draw rather
/// than a seed. Repeats its last value once the script runs out, so a test only
/// has to state the draws it cares about.
#[derive(Debug, Clone)]
pub struct ScriptedRng {
    values: Vec<f64>,
    next: usize,
    pub draws: usize,
}

impl ScriptedRng {
    pub fn new(values: &[f64]) -> Self {
        assert!(!values.is_empty(), "a scripted rng needs at least one value");
        Self {
            values: values.to_vec(),
            next: 0,
            draws: 0,
        }
    }

    /// The JS tests' `always` — an rng that passes any probability check.
    pub fn always() -> Self {
        Self::new(&[0.0])
    }

    /// The JS tests' `never` — fails anything under 100%.
    pub fn never() -> Self {
        Self::new(&[0.999])
    }
}

impl Rng for ScriptedRng {
    fn next_f64(&mut self) -> f64 {
        self.draws += 1;
        let v = self.values[self.next.min(self.values.len() - 1)];
        self.next += 1;
        v
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn xorshift_stays_in_range_and_repeats_from_a_seed() {
        let mut a = XorShift64::new(12345);
        let mut b = XorShift64::new(12345);
        for _ in 0..10_000 {
            let v = a.next_f64();
            assert!((0.0..1.0).contains(&v), "{v} out of [0,1)");
            assert_eq!(v, b.next_f64(), "same seed must give the same sequence");
        }
    }

    #[test]
    fn a_zero_seed_does_not_stick_at_zero() {
        let mut r = XorShift64::new(0);
        let first = r.next_f64();
        assert_ne!(first, r.next_f64());
    }

    #[test]
    fn different_seeds_diverge() {
        let (mut a, mut b) = (XorShift64::new(1), XorShift64::new(2));
        assert_ne!(a.next_f64(), b.next_f64());
    }

    #[test]
    fn a_scripted_rng_repeats_its_last_value() {
        let mut r = ScriptedRng::new(&[0.1, 0.2]);
        assert_eq!(r.next_f64(), 0.1);
        assert_eq!(r.next_f64(), 0.2);
        assert_eq!(r.next_f64(), 0.2);
        assert_eq!(r.draws, 3);
    }
}
