//! Heap usage of tables.
//!
//! [`HeapBytes::heap_bytes`] counts a table's capacity, used or kept for the
//! next build. Each stage's data sums its own tables.
//! [`Layout::heap_bytes`](crate::Layout::heap_bytes) and
//! [`Context::heap_bytes`](crate::Context::heap_bytes) report the sums per
//! stage as [`LayoutHeap`](crate::LayoutHeap) and
//! [`ContextHeap`](crate::ContextHeap).
//!
//! Shared memory is not counted here: font bytes shared with the collection
//! are reported apart, as `ContextHeap::font_data`. Values from read-fonts,
//! harfrust and fontwich that do not report their size count at their own
//! size only. Segmenter data is compiled in and takes no heap.

use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec::Vec;
use core::mem::size_of;

use hashbrown::HashTable;

/// A value that keeps bytes on the heap.
pub(crate) trait HeapBytes {
    /// The heap bytes its capacity takes, used or kept for the next build.
    /// Excludes memory it shares.
    fn heap_bytes(&self) -> usize;
}

/// Implements [`HeapBytes`] for a struct as the sum of its fields.
///
/// Fields after the `;` keep nothing on the heap and are not counted.
///
/// ```ignore
/// heap_bytes! {
///     /// Its tables; the initial letter holds nothing on the heap.
///     RareMeasures { edge_costs, shifts, em_boxes, rubies, kept_boxes; initial_letter }
/// }
/// ```
///
/// The impl destructures the struct, so a field missing here fails to
/// compile instead of going uncounted. A struct that owns more than its
/// fields' sum writes the impl out.
macro_rules! heap_bytes {
    (
        $(#[$meta:meta])*
        $name:ident {
            $first:ident $(, $field:ident)*
            $(; $($(#[$skip_meta:meta])* $skip:ident),+)?
        }
    ) => {
        impl $crate::data::HeapBytes for $name {
            $(#[$meta])*
            fn heap_bytes(&self) -> usize {
                let Self {
                    $first,
                    $($field,)*
                    $($($(#[$skip_meta])* $skip: _,)+)?
                } = self;
                $crate::data::HeapBytes::heap_bytes($first)
                    $(+ $crate::data::HeapBytes::heap_bytes($field))*
            }
        }
    };
}
pub(crate) use heap_bytes;

impl<T> HeapBytes for Vec<T> {
    /// The vector's capacity in bytes. Heap memory its items own is not
    /// counted; their owner counts it.
    fn heap_bytes(&self) -> usize {
        self.capacity().saturating_mul(size_of::<T>())
    }
}

impl HeapBytes for String {
    fn heap_bytes(&self) -> usize {
        self.capacity()
    }
}

impl<T> HeapBytes for HashTable<T> {
    /// The buckets and control bytes hashbrown allocated.
    fn heap_bytes(&self) -> usize {
        self.allocation_size()
    }
}

impl<T: HeapBytes> HeapBytes for Option<Box<T>> {
    /// A box of rare tables, made the first time a build needs one. Counts
    /// the box and its tables, or zero for `None`.
    fn heap_bytes(&self) -> usize {
        self.as_deref()
            .map_or(0, |rare| size_of::<T>() + rare.heap_bytes())
    }
}
