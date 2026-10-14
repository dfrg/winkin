//! The crate's one hash, for interning tables and context caches.
//!
//! hashbrown's default seeds itself from a process-wide static, and the crate
//! keeps no such state. A collision costs a comparison, never a wrong answer,
//! so a fixed, fast hash is enough: the Fx hash, as in fontwich, a rotate, an
//! xor and a multiply per word.

use core::hash::{Hash, Hasher};

/// The Fx hasher, which hashes a word at a time.
pub(crate) struct FxHasher(u64);

/// The Fx multiplier: odd, with its bits spread.
const K: u64 = 0xf135_7aea_2e62_a9c5;

impl FxHasher {
    /// A hasher that has seen nothing.
    pub(crate) const fn new() -> Self {
        Self(0)
    }

    fn add(&mut self, word: u64) {
        self.0 = (self.0.rotate_left(5) ^ word).wrapping_mul(K);
    }
}

impl Hasher for FxHasher {
    fn write(&mut self, bytes: &[u8]) {
        let (words, rest) = bytes.as_chunks::<8>();
        for word in words {
            self.add(u64::from_le_bytes(*word));
        }
        if !rest.is_empty() {
            let mut last = [0u8; 8];
            if let Some(tail) = last.get_mut(..rest.len()) {
                tail.copy_from_slice(rest);
            }
            // Mixes in the length so tails differing only in zeros differ.
            let len = u64::try_from(rest.len()).unwrap_or(0);
            self.add(u64::from_le_bytes(last) ^ (len << 56));
        }
    }

    fn write_u8(&mut self, n: u8) {
        self.add(u64::from(n));
    }

    fn write_u16(&mut self, n: u16) {
        self.add(u64::from(n));
    }

    fn write_u32(&mut self, n: u32) {
        self.add(u64::from(n));
    }

    fn write_u64(&mut self, n: u64) {
        self.add(n);
    }

    fn write_usize(&mut self, n: usize) {
        self.add(u64::try_from(n).unwrap_or(u64::MAX));
    }

    /// The hash, rotated so well-mixed bits reach the top seven, which
    /// hashbrown reads as control bytes. The multiply alone leaves them weak
    /// for short keys.
    fn finish(&self) -> u64 {
        self.0.rotate_left(26)
    }
}

/// The hash of `value` under [`FxHasher`].
pub(crate) fn hash_one(value: &impl Hash) -> u64 {
    let mut fx = FxHasher::new();
    value.hash(&mut fx);
    fx.finish()
}
