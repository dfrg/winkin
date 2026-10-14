//! The crate's in-place sorts: insertion sort for a few items, heapsort for
//! more.
//!
//! [`sort_by`] and [`sort_by_key`] are unstable and return at once on sorted
//! input. [`stable_sort_by_key`] is insertion sort alone. The crate sorts
//! short lists: a line's shifted boxes, a reordered line's box parts, bracket
//! pairs, ruby columns, feature settings and hyphenated texts.
//!
//! `core`'s sort is about 4 KB of code per type; these are a few hundred
//! bytes and allocate nothing. They match it on a few items and take
//! n log n steps on any input, but `core`'s is two to three times faster on
//! hundreds of items.

use core::cmp::Ordering;

/// The most items sorted by insertion, which is fastest for so few, as in
/// `core`'s sort.
const FEW: usize = 20;

/// Sorts `items` by `compare`, smallest first.
///
/// Not stable: equal items come out in no promised order.
pub(crate) fn sort_by<T>(items: &mut [T], mut compare: impl FnMut(&T, &T) -> Ordering) {
    if items.is_sorted_by(|a, b| compare(a, b) != Ordering::Greater) {
        return;
    }
    if items.len() <= FEW {
        insert_each(items, &mut compare);
        return;
    }
    // Heapsort: build a max-heap, then swap the top to the end and restore
    // the heap before it until one item is left.
    for node in (0..items.len() / 2).rev() {
        sift_down(items, node, &mut compare);
    }
    for end in (1..items.len()).rev() {
        items.swap(0, end);
        if let Some(heap) = items.get_mut(..end) {
            sift_down(heap, 0, &mut compare);
        }
    }
}

/// Sorts `items` by `key`, smallest first, as [`sort_by`] does.
///
/// Not stable: items with equal keys come out in no promised order.
pub(crate) fn sort_by_key<T, K: Ord>(items: &mut [T], mut key: impl FnMut(&T) -> K) {
    sort_by(items, |a, b| key(a).cmp(&key(b)));
}

/// Sorts `items` by `key`, smallest first, keeping equal keys in order.
///
/// Insertion sort: n² steps at worst, so only for short lists.
pub(crate) fn stable_sort_by_key<T, K: Ord>(items: &mut [T], mut key: impl FnMut(&T) -> K) {
    insert_each(items, &mut |a, b| key(a).cmp(&key(b)));
}

/// Sorts `items` by insertion, which keeps equal items in order.
fn insert_each<T>(items: &mut [T], compare: &mut impl FnMut(&T, &T) -> Ordering) {
    for at in 1..items.len() {
        let mut to = at;
        while let Some(before) = to.checked_sub(1) {
            match (items.get(before), items.get(to)) {
                (Some(earlier), Some(item)) if compare(earlier, item) == Ordering::Greater => {
                    items.swap(before, to);
                }
                _ => break,
            }
            to = before;
        }
    }
}

/// Moves the item at `node` down `heap` until neither child is larger.
fn sift_down<T>(heap: &mut [T], mut node: usize, compare: &mut impl FnMut(&T, &T) -> Ordering) {
    loop {
        let left = node.saturating_mul(2).saturating_add(1);
        let right = left.saturating_add(1);
        let child = match (heap.get(left), heap.get(right)) {
            (Some(left_item), Some(right_item)) => {
                if compare(left_item, right_item) == Ordering::Less {
                    right
                } else {
                    left
                }
            }
            (Some(_), None) => left,
            (None, _) => return,
        };
        match (heap.get(node), heap.get(child)) {
            (Some(sinking), Some(larger)) if compare(sinking, larger) == Ordering::Less => {}
            _ => return,
        }
        heap.swap(node, child);
        node = child;
    }
}
