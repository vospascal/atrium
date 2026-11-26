//! A small seeded random source: xorshift64*, as the engine's plan uses
//! (`src/plan/voice.rs`). The workspace has no random crate, and a recipe's
//! choices must come out the same from the same seed on every platform.

#[derive(Clone, Debug)]
pub(crate) struct Random {
    state: u64,
}

impl Random {
    pub(crate) fn new(seed: u64) -> Self {
        // Never zero, which xorshift would stay at.
        Self {
            state: seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1,
        }
    }

    /// A value in [0, 1).
    pub(crate) fn unit(&mut self) -> f64 {
        self.state ^= self.state >> 12;
        self.state ^= self.state << 25;
        self.state ^= self.state >> 27;
        let value = self.state.wrapping_mul(0x2545_F491_4F6C_DD1D);
        (value >> 11) as f64 / (1u64 << 53) as f64
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_same_seed_gives_the_same_values_in_range() {
        let mut first = Random::new(7);
        let mut second = Random::new(7);
        for _ in 0..1000 {
            let value = first.unit();
            assert_eq!(value, second.unit());
            assert!((0.0..1.0).contains(&value));
        }
        assert_ne!(Random::new(7).unit(), Random::new(8).unit());
    }
}
