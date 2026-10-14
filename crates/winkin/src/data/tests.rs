//! Data primitive tests: table ids, growth and limits, ranked bits, the hash,
//! the sort, sorted tables, runs and cursors, the LRU cache, and flags.

use alloc::vec;
use alloc::vec::Vec;
use core::hash::Hasher;

use super::IdRange;
use super::table::NARROW_RECORD;
use super::*;

define_id! {
    /// An id small enough to fill in a test.
    struct TinyId(u8);
}

define_id! {
    struct WideId(u32);
}

#[test]
fn a_table_hands_out_its_ids_in_order() {
    let mut table = Table::<WideId, &str>::new();
    let a = table.push("a").unwrap();
    let b = table.push("b").unwrap();
    assert_eq!((a.get(), b.get()), (0, 1));
    assert_eq!(table[b], "b");
    assert_eq!(table.next_id().get(), 2);
    assert_eq!(table.slice(a..table.next_id()), ["a", "b"]);
    assert_eq!(table.get_slice(b..table.next_id()), Some(&["b"][..]));
    assert_eq!(table.get_slice(b..a), None);
    assert_eq!(table.get_slice(a..WideId::new(3)), None);
    assert_eq!(table.ids().collect::<Vec<_>>(), [a, b]);
}

#[test]
fn clearing_keeps_the_allocation() {
    let mut table = Table::<WideId, u64>::new();
    for n in 0..100 {
        assert!(table.push(n).is_some());
    }
    let bytes = table.heap_bytes();
    table.clear();
    assert!(table.is_empty());
    assert_eq!(table.heap_bytes(), bytes);
}

/// An unused table reserves exactly; a used one keeps enough room and
/// doubles otherwise.
#[test]
fn a_table_makes_room_exactly_where_it_never_held_any() {
    let mut table = Table::<WideId, u32>::new();
    table.reserve(21);
    assert_eq!(table.heap_bytes(), 21 * 4);
    table.clear();
    table.reserve(5);
    assert_eq!(table.heap_bytes(), 21 * 4, "less keeps the capacity");
    table.reserve(22);
    assert_eq!(table.heap_bytes(), 42 * 4, "more doubles");
    // Capped at what the ids name.
    let mut tiny = Table::<TinyId, u8>::new();
    tiny.reserve(1000);
    assert_eq!(tiny.heap_bytes(), 255);
    let mut bits = BitTable::<WideId>::new();
    bits.reserve(65);
    assert_eq!(bits.heap_bytes(), 2 * 8);
}

/// A table of records wider than [`NARROW_RECORD`] starts with room for one
/// and doubles, capped at what its ids name. A narrower one grows as a
/// vector does.
#[test]
fn a_table_starts_with_room_for_one_and_doubles() {
    let mut table = Table::<WideId, [u8; 8]>::new();
    for (pushed, room) in [(1, 1), (2, 2), (3, 4), (4, 4), (5, 8)] {
        while table.len() < pushed {
            assert!(table.push([0; 8]).is_some());
        }
        assert_eq!(table.heap_bytes(), room * 8, "{pushed} pushed");
    }
    let mut tiny = Table::<TinyId, [u8; NARROW_RECORD + 1]>::new();
    while tiny.push([0; NARROW_RECORD + 1]).is_some() {}
    assert_eq!(tiny.heap_bytes(), 255 * (NARROW_RECORD + 1));
    let mut narrow = Table::<WideId, u32>::new();
    assert!(narrow.push(0).is_some());
    assert_eq!(
        narrow.heap_bytes(),
        4 * 4,
        "a narrow record's table as a vector's"
    );
}

#[test]
fn an_id_names_up_to_its_type_and_no_further() {
    assert_eq!(TinyId::try_new(255).map(TinyId::get), Some(255));
    assert_eq!(TinyId::try_new(256), None);
}

#[test]
fn a_full_table_refuses_another_without_panicking() {
    let mut table = Table::<TinyId, u8>::new();
    for n in 0..255 {
        assert_eq!(table.push(n as u8).map(TinyId::get), Some(n));
    }
    assert_eq!(table.remaining(), 0);
    // The 256th is refused; the length stays an id.
    assert_eq!(table.push(0), None);
    assert_eq!(table.len(), 255);
    assert_eq!(table.next_id().get(), 255);
    assert_eq!(table.ids().count(), 255);
}

#[test]
fn a_table_extends_whole_or_not_at_all() {
    let mut table = Table::<TinyId, u8>::new();
    assert_eq!(table.last_id(), None);
    let first = table.extend((0..200).map(|n| n as u8));
    assert_eq!(first, Some(TinyId::new(0)..TinyId::new(200)));
    assert_eq!(table.last_id(), Some(TinyId::new(199)));
    // 55 more fit; 56 do not, and none of them is kept.
    assert_eq!(table.extend(0..56u8), None);
    assert_eq!(table.len(), 200);
    assert_eq!(
        table.extend(0..55u8),
        Some(TinyId::new(200)..TinyId::new(255))
    );
}

#[test]
fn setting_past_the_end_fills_up_to_it() {
    let mut table = Table::<WideId, u32>::new();
    table.set_growing(WideId::new(3), 7, 0);
    assert_eq!(table.as_slice(), [0, 0, 0, 7]);
    table.set_growing(WideId::new(1), 5, 0);
    assert_eq!(table.as_slice(), [0, 5, 0, 7]);
    let mut tiny = Table::<TinyId, u8>::new();
    tiny.set_growing(TinyId::new(255), 1, 0);
    assert!(
        tiny.is_empty(),
        "an id past what the table holds sets nothing"
    );
}

#[test]
fn ids_walk_a_range_both_ways() {
    let range = WideId::new(3)..WideId::new(6);
    assert_eq!(
        range.clone().ids().map(WideId::get).collect::<Vec<_>>(),
        [3, 4, 5]
    );
    assert_eq!(
        range.ids().rev().map(WideId::get).collect::<Vec<_>>(),
        [5, 4, 3]
    );
}

/// A ranked table's count matches a naive count, across empty words and past
/// the last word.
#[test]
fn a_ranked_table_counts_what_is_set_through_an_id() {
    let set = [0, 1, 63, 64, 65, 200, 400, 401, 511];
    let mut table = RankedBitTable::<WideId>::new();
    assert_eq!(table.count_through(WideId::new(5)), 0, "none set");
    for &at in &set {
        table.set(WideId::new(at));
    }
    for at in 0..700 {
        let counted = set.iter().filter(|&&held| held <= at).count();
        assert_eq!(table.count_through(WideId::new(at)), counted, "{at}");
    }
    table.clear();
    assert_eq!(table.count_through(WideId::new(400)), 0, "cleared");
}

#[test]
fn tails_of_different_lengths_hash_apart() {
    let hash = |bytes: &[u8]| {
        let mut fx = FxHasher::new();
        fx.write(bytes);
        fx.finish()
    };
    assert_ne!(hash(b"a"), hash(b"a\0"));
    assert_ne!(hash(b""), hash(b"\0"));
    assert_eq!(hash(b"Segoe UI"), hash(b"Segoe UI"));
}

#[test]
fn sorts_as_core_does() {
    // Every length below 200: rising, falling, random with many equal keys,
    // and random.
    let mut state = 0x2545_f491_u32;
    let mut next = || {
        state ^= state << 13;
        state ^= state >> 17;
        state ^= state << 5;
        state
    };
    for len in 0..200 {
        for shape in 0..4 {
            let items: Vec<u32> = (0..len)
                .map(|at| match shape {
                    0 => at,
                    1 => len - at,
                    2 => next() % 16,
                    _ => next(),
                })
                .collect();
            let mut ours = items.clone();
            sort_by_key(&mut ours, |&item| item);
            let mut cores = items;
            cores.sort_unstable();
            assert_eq!(ours, cores, "{len} items of shape {shape}");
        }
    }
}

#[test]
fn sorts_by_the_key_alone() {
    let mut pairs = [(3, 'c'), (1, 'a'), (2, 'b'), (0, 'z')];
    sort_by_key(&mut pairs, |pair| pair.0);
    assert_eq!(pairs, [(0, 'z'), (1, 'a'), (2, 'b'), (3, 'c')]);
}

/// A sorted table finds each key it holds and none between or past them;
/// [`find_sorted`] does the same over a slice.
#[test]
fn a_sorted_table_finds_its_keys_and_no_others() {
    let mut table = SortedTable::<(WideId, char)>::new();
    assert!(table.is_empty());
    assert_eq!(table.get(WideId::new(3)), None, "an empty table");
    for (at, name) in [(1, 'a'), (4, 'b'), (9, 'c')] {
        table.push((WideId::new(at), name));
    }
    for (at, found) in [(1, Some('a')), (4, Some('b')), (9, Some('c'))] {
        assert_eq!(table.get(WideId::new(at)).map(|&(_, name)| name), found);
    }
    for at in [0, 2, 5, 8, 10] {
        assert_eq!(table.get(WideId::new(at)), None, "{at}");
    }
    let rest = table.as_slice().get(1..).unwrap_or_default();
    assert_eq!(
        find_sorted(rest, WideId::new(4)),
        Some(&(WideId::new(4), 'b'))
    );
    assert_eq!(find_sorted(rest, WideId::new(1)), None);
    table.clear();
    assert!(table.is_empty());
}

/// A run known by its start.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
struct Wide(WideId);

impl Run for Wide {
    type Position = WideId;

    fn start(&self) -> WideId {
        self.0
    }
}

/// Runs starting at 0, 3, 3 and 7 and ending at 10, one empty:
/// - each position belongs to the last run starting at or before it;
/// - a forward cursor agrees with the search;
/// - each span ends at the next start or the end.
#[test]
fn runs_find_the_run_holding_a_position_as_a_cursor_does() {
    let mut runs = Runs::<TinyId, Wide>::new();
    assert_eq!(runs.containing(WideId::new(0)), None, "no runs");
    for start in [0, 3, 3, 7] {
        assert!(runs.push(Wide(WideId::new(start))).is_some());
    }
    let end = WideId::new(10);
    let mut cursor = runs.cursor(TinyId::new(0), end);
    for at in 0..10 {
        let at = WideId::new(at);
        runs.step_to(&mut cursor, at, end);
        assert_eq!(runs.containing(at), Some(cursor.id()), "{at:?}");
        assert!(runs.span(cursor.id(), end).contains(&at), "{at:?}");
    }
    assert_eq!(
        runs.span(TinyId::new(1), end),
        WideId::new(3)..WideId::new(3)
    );
    assert_eq!(runs.span(TinyId::new(3), end), WideId::new(7)..end);
    assert_eq!(runs.span(TinyId::new(4), end), end..end, "past the last");
    assert_eq!(runs.next_start(TinyId::new(3)), None);
    assert_eq!(runs.first_from(WideId::new(3)), TinyId::new(1));
    assert_eq!(runs.first_from(WideId::new(4)), TinyId::new(3));
    assert_eq!(runs.rest(TinyId::new(2)).len(), 2);
}

/// A run cursor over the same runs:
/// - stepping visits each run, the empty one too, then stays past the last;
/// - `step_to` passes runs ending at or before the position, never the last;
/// - `cursor_containing` gives the same cursor.
#[test]
fn a_run_cursor_holds_where_its_run_ends() {
    let mut runs = Runs::<TinyId, Wide>::new();
    let end = WideId::new(10);
    assert_eq!(runs.cursor_containing(WideId::new(0), end), None, "no runs");
    for start in [0, 3, 3, 7] {
        assert!(runs.push(Wide(WideId::new(start))).is_some());
    }
    let mut cursor = runs.cursor(TinyId::new(0), end);
    let mut seen = vec![(cursor.id(), cursor.end())];
    for _ in 0..5 {
        runs.step(&mut cursor, end);
        seen.push((cursor.id(), cursor.end()));
    }
    let ends: Vec<_> = seen.iter().map(|&(id, at)| (id.get(), at.get())).collect();
    assert_eq!(ends, [(0, 3), (1, 3), (2, 7), (3, 10), (4, 10), (4, 10)]);
    let mut cursor = runs.cursor(TinyId::new(0), end);
    for at in 0..=10 {
        let at = WideId::new(at);
        runs.step_to(&mut cursor, at, end);
        let from = runs.containing(at).unwrap_or(TinyId::new(0));
        assert_eq!(cursor.id(), from, "{at:?}");
        assert_eq!(cursor.end(), runs.span(from, end).end, "{at:?}");
        assert_eq!(runs.cursor_containing(at, end), Some(cursor), "{at:?}");
    }
    assert_eq!(cursor.id(), TinyId::new(3), "never past the last");
}

/// A cache finds its four most recent entries without hashing and older ones
/// by hash; on a miss it returns the hash.
#[test]
fn a_cache_finds_the_entries_it_last_found_without_hashing() {
    let mut cache = LruCache::<WideId, u32>::new(usize::MAX);
    let hash = |n: u32| u64::from(n).wrapping_mul(0x9e37_79b9);
    for n in 0..6 {
        assert_eq!(cache.insert(hash(n), n), Some(WideId::new(n as usize)));
    }
    assert_eq!(cache.len(), 6);
    // The last four added need no hash.
    for n in 2..6 {
        let found = cache.find_recent(|| panic!("{n} is recent"), |&held| held == n);
        assert_eq!(found, Ok(WideId::new(n as usize)));
    }
    // An older one is found by hash and becomes recent.
    let older = cache.find_recent(|| hash(0), |&held| held == 0);
    assert_eq!(older, Ok(WideId::new(0)));
    let again = cache.find_recent(|| panic!("0 is recent now"), |&held| held == 0);
    assert_eq!(again, Ok(WideId::new(0)));
    assert_eq!(cache.find_recent(|| 7, |&held| held == 7), Err(7));
    assert_eq!(cache.find(hash(1), |&held| held == 1), Some(WideId::new(1)));
    cache.clear();
    assert_eq!(cache.find(hash(1), |&held| held == 1), None, "cleared");
}

/// A cache trims the least recently used entries to its capacity and reuses
/// them. A find counts as a use; a peek does not.
#[test]
fn a_cache_trims_the_entries_used_longest_ago() {
    let mut cache = LruCache::<WideId, u32>::new(3);
    let hash = |n: u32| u64::from(n).wrapping_mul(0x9e37_79b9);
    for n in 0..5 {
        assert!(cache.insert(hash(n), n).is_some());
    }
    assert_eq!(cache.len(), 5, "adding never evicts");
    // 0 is used; 1 is only peeked at.
    assert!(cache.find(hash(0), |&held| held == 0).is_some());
    assert!(cache.peek(hash(1), |&held| held == 1).is_some());
    let mut evicted = Vec::new();
    cache.trim(|_, n| evicted.push(n));
    assert_eq!(evicted, [1, 2]);
    assert_eq!(cache.len(), 3);
    let held = |cache: &LruCache<WideId, u32>| {
        let mut held: Vec<u32> = cache.iter().map(|(_, &n)| n).collect();
        held.sort_unstable();
        held
    };
    assert_eq!(held(&cache), [0, 3, 4]);
    for n in [1, 2] {
        assert_eq!(cache.find(hash(n), |&held| held == n), None, "{n} is gone");
    }
    // Freed entries are reused, so the entries do not grow.
    let entries = cache.heap_bytes();
    for n in 5..7 {
        let id = cache.insert(hash(n), n).expect("an entry");
        assert!(id.get() < 5, "{id:?} reuses an entry");
    }
    assert_eq!(cache.heap_bytes(), entries);
    cache.trim(|_, _| {});
    assert_eq!(held(&cache), [0, 5, 6]);
    // A capacity of zero keeps nothing past a trim.
    cache.set_capacity(0);
    cache.trim(|_, _| {});
    assert_eq!(cache.len(), 0);
    assert!(cache.iter().next().is_none());
}

/// A cache drops the entries `retain` refuses and keeps finding the rest.
#[test]
fn a_cache_retains_what_it_is_told_to() {
    let mut cache = LruCache::<WideId, u32>::new(usize::MAX);
    let hash = |n: u32| u64::from(n).wrapping_mul(0x9e37_79b9);
    for n in 0..6 {
        assert!(cache.insert(hash(n), n).is_some());
    }
    let mut dropped = Vec::new();
    cache.retain(|_, &n| n % 2 == 0, |_, n| dropped.push(n));
    assert_eq!(dropped, [1, 3, 5]);
    for n in 0..6 {
        let found = cache.find(hash(n), |&held| held == n).is_some();
        assert_eq!(found, n % 2 == 0, "{n}");
    }
    assert_eq!(cache.remove(WideId::new(0)), Some(0));
    assert_eq!(cache.remove(WideId::new(0)), None, "removed once");
}

define_flags! {
    /// Flags for the test.
    struct TestFlags(u8) {
        const A = 1 << 0;
        const B = 1 << 1;
    }
}

/// A set of flags contains another set only if it holds every flag in it.
#[test]
fn flags_contain_every_flag_of_a_set_they_hold() {
    let mut flags = TestFlags::NONE;
    assert!(flags.contains(TestFlags::NONE));
    assert!(!flags.contains(TestFlags::A));
    flags.insert(TestFlags::A);
    assert!(flags.contains(TestFlags::A));
    assert!(!flags.contains(TestFlags::A.union(TestFlags::B)));
    flags.insert(TestFlags::B);
    assert!(flags.contains(TestFlags::A.union(TestFlags::B)));
    assert_eq!(TestFlags::default(), TestFlags::NONE);
}
