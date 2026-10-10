//! Small in-place sorts.
//!
//! [`by`] and [`by_key`] are not stable. They return at once if the items
//! are in order, sort a few by insertion and more by heapsort. A caller
//! that needs equal items in order breaks ties by position.
//!
//! The crate sorts a charset's subtables and ranges, a layer's names and
//! the files a scan finds. `core`'s sorts cost some 4 KB of code for each
//! type they sort. These cost a few hundred bytes and allocate nothing.

use core::cmp::Ordering;

/// How many items are few enough to sort by insertion, which is quickest
/// for so few, as `core`'s sort finds.
const FEW: usize = 20;

/// Sorts `items` by `compare`, smallest first, in n log n steps however
/// many come in whatever order.
///
/// Not stable: items that compare equal come out in no order promised.
pub(crate) fn by<T>(items: &mut [T], mut compare: impl FnMut(&T, &T) -> Ordering) {
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

/// Sorts `items` by `key`, smallest first, as [`by`] does.
///
/// Not stable: items whose keys are equal come out in no order promised.
pub(crate) fn by_key<T, K: Ord>(items: &mut [T], mut key: impl FnMut(&T) -> K) {
    by(items, |a, b| key(a).cmp(&key(b)));
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

#[cfg(test)]
mod tests {
    use alloc::vec::Vec;

    use super::{by, by_key};

    /// Runs of every length up to 200, rising, falling, shuffled and
    /// shuffled with many keys the same.
    fn runs() -> Vec<Vec<(u32, usize)>> {
        let mut state = 0x2545_f491_u32;
        let mut next = move || {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            state
        };
        let mut runs = Vec::new();
        for len in 0..200u32 {
            for shape in 0..4 {
                let run = (0..len)
                    .map(|at| match shape {
                        0 => at,
                        1 => len - at,
                        2 => next() % 16,
                        _ => next(),
                    })
                    .enumerate()
                    .map(|(position, key)| (key, position))
                    .collect();
                runs.push(run);
            }
        }
        runs
    }

    #[test]
    fn the_heapsort_sorts_as_core_does() {
        for run in runs() {
            let keys: Vec<u32> = run.iter().map(|&(key, _)| key).collect();
            let mut ours = keys.clone();
            by_key(&mut ours, |&key| key);
            let mut cores = keys;
            cores.sort_unstable();
            assert_eq!(ours, cores);
        }
    }

    #[test]
    fn a_comparison_sorts_as_core_does() {
        for run in runs() {
            let mut ours = run.clone();
            by(&mut ours, |a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)));
            let mut cores = run;
            cores.sort_unstable();
            assert_eq!(ours, cores);
        }
    }
}
