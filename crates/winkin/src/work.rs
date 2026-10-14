//! Counts of the steps the stages take and the searches they make.
//!
//! Each loop whose length depends on input counts one step per pass. A test
//! builds the same shape of content at two sizes and compares the steps
//! each stage took. That catches a stage whose steps grow faster than its
//! input and output, with no clock to make the test flaky.
//!
//! Each search counts a seek: a halving, a rank on start bits, or a lookup
//! of a row by position. A test holds a stage to the seeks its entries need,
//! so a walk that loses its place and searches again shows up as a count.
//!
//! Counting happens only in the crate's tests built with debug assertions,
//! per thread, so tests running side by side do not count each other's.
//! Elsewhere [`step`] and [`seek`] are empty: in the library, and in the
//! release builds the timing tests run in.

#[cfg(test)]
use core::cell::Cell;

#[cfg(all(test, debug_assertions))]
std::thread_local! {
    static STEPS: Cell<u64> = const { Cell::new(0) };
    static SEEKS: Cell<u64> = const { Cell::new(0) };
}

/// Counts one step of a walk, in debug test builds only.
#[inline(always)]
pub(crate) fn step() {
    #[cfg(all(test, debug_assertions))]
    STEPS.with(|steps| steps.set(steps.get().wrapping_add(1)));
}

/// Counts one seek, a search for a row by position, in debug test builds
/// only.
#[inline(always)]
pub(crate) fn seek() {
    #[cfg(all(test, debug_assertions))]
    SEEKS.with(|seeks| seeks.set(seeks.get().wrapping_add(1)));
}

#[cfg(test)]
std::thread_local! {
    static GENERAL_ONLY: Cell<bool> = const { Cell::new(false) };
}

/// Returns whether a stage may take a fast path where its facts allow one.
///
/// Always true, except on a test's thread inside `general_paths`, where
/// tests check that each fast path matches the general path.
#[inline(always)]
pub(crate) fn fast_paths() -> bool {
    #[cfg(test)]
    if GENERAL_ONLY.with(Cell::get) {
        return false;
    }
    true
}

/// Runs `f` with every fast path closed on this thread.
#[cfg(test)]
pub(crate) fn general_paths<R>(f: impl FnOnce() -> R) -> R {
    let was = GENERAL_ONLY.with(|general| general.replace(true));
    let result = f();
    GENERAL_ONLY.with(|general| general.set(was));
    result
}

/// Returns the steps counted on this thread since the last call, and
/// resets the count.
#[cfg(all(test, debug_assertions))]
pub(crate) fn take() -> u64 {
    STEPS.with(|steps| steps.replace(0))
}

/// Returns the seeks counted on this thread since the last call, and
/// resets the count.
#[cfg(all(test, debug_assertions))]
pub(crate) fn take_seeks() -> u64 {
    SEEKS.with(|seeks| seeks.replace(0))
}
