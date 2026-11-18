//! Cached fallback answers and the layers they were read from.

use super::key::FallbackKey;
use crate::Family;
use crate::hash::HashMap;
use crate::layer::Layer;
use crate::sync::SpinRwLock;
use alloc::sync::Arc;

/// A collection's fallback answers: the installed families for each key it
/// has been asked, stamped with the layers they were read from.
///
/// Empty until asked, and bounded by the finite key space. The lock is held
/// only to look an answer up or put one in, never while a backend is asked
/// or names are resolved, so two threads missing one key both work it out
/// and the first answer stands: a backend's answer is the same either way.
/// Lookups share it, so only putting an answer in waits.
#[derive(Default)]
pub(crate) struct FallbackCache(SpinRwLock<Answers>);

#[derive(Default)]
struct Answers {
    /// The fallback layers the answers were read from, as [`stamp`] gives
    /// them.
    stamp: u64,
    by_key: HashMap<FallbackKey, Arc<[Family]>>,
}

impl FallbackCache {
    /// The answer for `key`, if there is one for the layers `stamp` names.
    ///
    /// Answers read from other layers are left for [`insert`](Self::insert)
    /// to drop, so a lookup only reads.
    pub(crate) fn get(&self, stamp: u64, key: &FallbackKey) -> Option<Arc<[Family]>> {
        let answers = self.0.read();
        if answers.stamp != stamp {
            return None;
        }
        answers.by_key.get(key).cloned()
    }

    /// Keeps `families` for `key`, unless another thread got there first,
    /// and gives back the answer that stands.
    pub(crate) fn insert(
        &self,
        stamp: u64,
        key: FallbackKey,
        families: Arc<[Family]>,
    ) -> Arc<[Family]> {
        let mut answers = self.0.write();
        answers.restamp(stamp);
        answers.by_key.entry(key).or_insert(families).clone()
    }

    /// The map and the answers it holds; the families are the layers'.
    pub(crate) fn heap(&self) -> usize {
        use crate::heap::ARC;
        use core::mem::{size_of, size_of_val};
        let answers = self.0.read();
        answers.by_key.capacity() * (size_of::<(FallbackKey, Arc<[Family]>)>() + 1)
            + answers
                .by_key
                .values()
                .map(|families| ARC + size_of_val(&**families))
                .sum::<usize>()
    }
}

impl Answers {
    /// Drops every answer if they were read from other layers than `stamp`
    /// names.
    fn restamp(&mut self, stamp: u64) {
        if self.stamp != stamp {
            self.by_key.clear();
            self.stamp = stamp;
        }
    }
}

impl core::fmt::Debug for FallbackCache {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("FallbackCache")
            .field("keys", &self.0.read().by_key.len())
            .finish()
    }
}

/// What names the fallback layers `layers` are: each one's address and
/// generation, mixed. Answers read from one set of layers are not served
/// for another.
pub(crate) fn stamp<'a>(layers: impl Iterator<Item = &'a Arc<Layer>>) -> u64 {
    let mut stamp: u64 = 0xcbf2_9ce4_8422_2325;
    for layer in layers {
        for word in [Arc::as_ptr(layer) as usize as u64, layer.generation()] {
            stamp = (stamp.rotate_left(5) ^ word).wrapping_mul(0x5170_8a59_c7f9_2b6b);
        }
    }
    stamp
}
