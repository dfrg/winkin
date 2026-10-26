//! What winkin's unit tests and integration tests share: fonts built in
//! memory, a global allocator that counts allocations per thread, and the
//! text corpora segmentation is timed over.
//!
//! It names nothing of winkin's, only fontwich's, so that winkin's unit
//! tests can depend on it.

#![no_std]

extern crate alloc;
extern crate std;

pub mod allocator;
pub mod corpora;
pub mod fonts;
