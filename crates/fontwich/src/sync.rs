//! A lock and a one-time cell that block with `std` and spin without it.
//!
//! With `std`, a thread waiting for a family to load or a file to be read
//! sleeps rather than taking a core. Without it there is nothing to sleep
//! on, so both spin. A `no_std` build reads no files, so the only slow work a
//! waiter can spin through is a caller's `LoadFamily`.
//!
//! [`SpinRwLock`] spins in every build, for the fallback cache's lookups,
//! which are too short and too hot to pay for a blocking lock.

use core::fmt;
use core::ops::DerefMut;

/// A mutual exclusion lock: `std`'s with the `std` feature, a spin lock
/// without it.
///
/// A panic while the lock is held does not poison it. Every value it guards
/// stays consistent at each point a panic could unwind from.
#[derive(Default)]
pub(crate) struct Mutex<T>(
    #[cfg(feature = "std")] std::sync::Mutex<T>,
    #[cfg(not(feature = "std"))] spin::Mutex<T>,
);

impl<T> Mutex<T> {
    /// Locks, waiting until no other thread holds the lock.
    pub(crate) fn lock(&self) -> impl DerefMut<Target = T> + '_ {
        #[cfg(feature = "std")]
        return self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        #[cfg(not(feature = "std"))]
        return self.0.lock();
    }
}

/// A spin read-write lock in every build, for a hot critical section of a
/// few hundred nanoseconds at most that never waits on anything.
///
/// Readers share it, so lookups from many threads don't queue behind one
/// another; only a writer waits for them, and they for it.
pub(crate) type SpinRwLock<T> = spin::RwLock<T>;

// Shows no value, since reading it would take the lock.
impl<T> fmt::Debug for Mutex<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Mutex").finish_non_exhaustive()
    }
}

/// A cell written once: `std`'s `OnceLock` with the `std` feature, a spin
/// `Once` without it.
///
/// The value is held inline. A second thread asking for it while the first
/// makes it waits for that value rather than making its own.
#[derive(Default)]
pub(crate) struct Once<T>(
    #[cfg(feature = "std")] std::sync::OnceLock<T>,
    #[cfg(not(feature = "std"))] spin::Once<T>,
);

impl<T> Once<T> {
    /// An empty cell.
    pub(crate) const fn new() -> Self {
        #[cfg(feature = "std")]
        return Self(std::sync::OnceLock::new());
        #[cfg(not(feature = "std"))]
        return Self(spin::Once::new());
    }

    /// A cell already holding `value`.
    pub(crate) fn with_value(value: T) -> Self {
        #[cfg(feature = "std")]
        return Self(std::sync::OnceLock::from(value));
        #[cfg(not(feature = "std"))]
        return Self(spin::Once::initialized(value));
    }

    /// Returns the value, if the cell holds one.
    pub(crate) fn get(&self) -> Option<&T> {
        self.0.get()
    }

    /// Returns the value mutably, if the cell holds one.
    pub(crate) fn get_mut(&mut self) -> Option<&mut T> {
        self.0.get_mut()
    }

    /// Returns the value, making it with `make` if the cell is empty.
    ///
    /// Runs `make` on one thread only. Others asking meanwhile wait for its
    /// value.
    pub(crate) fn get_or_init(&self, make: impl FnOnce() -> T) -> &T {
        #[cfg(feature = "std")]
        return self.0.get_or_init(make);
        #[cfg(not(feature = "std"))]
        return self.0.call_once(make);
    }
}

impl<T: fmt::Debug> fmt::Debug for Once<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("Once").field(&self.get()).finish()
    }
}

#[cfg(all(test, feature = "std"))]
mod tests {
    use alloc::string::String;
    use alloc::vec::Vec;
    use core::sync::atomic::{AtomicU32, Ordering};

    use super::Once;

    #[test]
    fn a_cell_makes_its_value_once_however_many_threads_ask() {
        let cell = Once::new();
        let makes = AtomicU32::new(0);
        let barrier = std::sync::Barrier::new(8);
        let values: Vec<u32> = std::thread::scope(|scope| {
            let handles: Vec<_> = (0..8)
                .map(|_| {
                    scope.spawn(|| {
                        barrier.wait();
                        *cell.get_or_init(|| makes.fetch_add(1, Ordering::Relaxed) + 7)
                    })
                })
                .collect();
            handles
                .into_iter()
                .map(|handle| handle.join().expect("no thread panics"))
                .collect()
        });
        assert_eq!(makes.load(Ordering::Relaxed), 1);
        assert!(values.iter().all(|&value| value == 7));
    }

    /// Every `.rs` file under `dir`, with its text.
    fn sources(dir: &std::path::Path, out: &mut Vec<(std::path::PathBuf, String)>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                sources(&path, out);
            } else if path.extension().is_some_and(|ext| ext == "rs") {
                let text = std::fs::read_to_string(&path).expect("a source file");
                out.push((path, text));
            }
        }
    }

    #[test]
    fn no_module_but_this_one_names_the_spinning_primitives() {
        // A spin lock held across I/O or a family load keeps a waiting core
        // busy. This module alone picks between spinning and blocking, and it
        // spins only without `std`.
        let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut files = Vec::new();
        sources(&src, &mut files);
        assert!(files.len() > 1, "the sources are readable");
        for (path, text) in &files {
            if !path.ends_with("sync.rs") {
                assert!(!text.contains("spin::"), "{} names spin", path.display());
            }
        }
    }
}
