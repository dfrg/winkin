//! A lock that blocks with `std` and spins without it.
//!
//! With `std`, a thread waiting for another to parse a font sleeps rather
//! than taking a core. Without it there is nothing to sleep on, so it spins.

use core::ops::DerefMut;
#[cfg(feature = "std")]
use std::sync::{Mutex as Blocking, PoisonError};

#[cfg(not(feature = "std"))]
use spin::Mutex as Blocking;

/// A mutual exclusion lock: `std`'s with the `std` feature, a spin lock
/// without it.
///
/// A panic while the lock is held does not poison it. Every value it guards
/// stays consistent at each point a panic could unwind from.
pub(crate) struct Mutex<T>(Blocking<T>);

impl<T> Mutex<T> {
    /// A lock guarding `value`.
    pub(crate) fn new(value: T) -> Self {
        Self(Blocking::new(value))
    }

    /// Locks, waiting until no other thread holds the lock.
    pub(crate) fn lock(&self) -> impl DerefMut<Target = T> + '_ {
        #[cfg(feature = "std")]
        return self.0.lock().unwrap_or_else(PoisonError::into_inner);
        #[cfg(not(feature = "std"))]
        return self.0.lock();
    }
}
