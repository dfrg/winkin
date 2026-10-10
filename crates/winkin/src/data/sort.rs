//! The crate's in-place sorts: an insertion sort for a few items and a
//! heapsort for more.
//!
//! [`sort_by`] and [`sort_by_key`] return at once where the items are in
//! order already, and are not stable. [`stable_sort_by_key`] is the
//! insertion sort alone, for the few lists whose equal keys keep their
//! order. The crate sorts a line's shifted boxes, the segments of a line
//! that bidi reorders, a paragraph's bracket pairs, the room ruby columns
//! take, and a font's feature settings. These come a few at a time.
//!
//! `core`'s sort is about 4 KB of code for each type it sorts. These are a
//! few hundred bytes and allocate nothing. They are as quick for a few
//! items, and the heapsort takes n log n steps for any input. `core`'s is
//! two or three times quicker on hundreds of items.

use core::cmp::Ordering;

/// How many items are few enough to sort by insertion, which is quickest
/// for so few, as `core`'s sort finds.
const FEW: usize = 20;

/// Sorts `items` by `compare`, smallest first.
///
/// Not stable: items that compare equal come out in no order promised.
pub(crate) fn sort_by<T>(items: &mut [T], mut compare: impl FnMut(&T, &T) -> Ordering) {
    if items.is_sorted_by(|a, b| compare(a, b) != Ordering::Greater) {
        return;
    }
    if items.len() <= FEW {
        insert_each(items, &mut compare);
        return;
    }
    // A heap of them all, the largest on top; then the top swapped to the
    // end, and the heap before it made whole again, until one is left.
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
/// Not stable: items whose keys are equal come out in no order promised.
pub(crate) fn sort_by_key<T, K: Ord>(items: &mut [T], mut key: impl FnMut(&T) -> K) {
    sort_by(items, |a, b| key(a).cmp(&key(b)));
}

/// Sorts `items` by `key`, smallest first, keeping items with equal keys in
/// the order they came.
///
/// Moves each item back past those before it with a greater key: n² steps
/// at worst, so it is for short lists.
pub(crate) fn stable_sort_by_key<T, K: Ord>(items: &mut [T], mut key: impl FnMut(&T) -> K) {
    insert_each(items, &mut |a, b| key(a).cmp(&key(b)));
}

/// Sorts `items` by moving each back past those before it that compare
/// greater, which keeps equal items in order.
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

/// Moves the item at `node` of `heap` down until neither of the items below
/// it is larger.
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
