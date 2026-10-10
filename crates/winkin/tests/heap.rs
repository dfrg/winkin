//! Holds what a layout and a context say they keep on the heap to what the
//! allocator says they took.
//!
//! A global allocator that keeps, per thread, the bytes live: every
//! allocation adds its size, every free takes it away. Built into a new
//! layout in a warm context, a document leaves live exactly what the new
//! layout keeps, since a warm context takes nothing more, as
//! `tests/allocations/` holds. So a layout's
//! [`heap_bytes`](Layout::heap_bytes) must come to that, stage by stage
//! summed, or some table went uncounted. The documents reach every stage: a
//! first line restyled in its own variant, boxes with edges, bidi, soft
//! hyphens, ruby with annotation lines and emphasis marks.
//!
//! A context's count leaves out what harfrust and fontwich keep inside the
//! values its caches hold, so it is held under what the allocator says the
//! context took, not to it.

extern crate alloc;

use std::alloc::{GlobalAlloc, Layout as Allocation, System};
use std::borrow::Cow;
use std::cell::Cell;

use fontwich::{Collection, LayerBuilder, Role};
use testing::fonts::{AHEM, TestFallback, TestFont};
use winkin::style::{
    ComputedStyle, EmphasisSide, FontFamilyName, FontGroup, Sides, TextAlign, TextCase,
};
use winkin::{
    Area, BuildOptions, ComputedBlockStyle, Context, ContextHeap, Layout, LayoutHeap, NoExclusions,
    NodeKey,
};

struct Counting;

thread_local! {
    // `const` and a type with no destructor, so reading it never allocates
    // and never registers anything itself.
    static LIVE: Cell<isize> = const { Cell::new(0) };
}

/// Adds `bytes`, which is negative for a free, to this thread's live bytes.
fn note(bytes: isize) {
    let _ = LIVE.try_with(|live| live.set(live.get() + bytes));
}

/// A size as the tally keeps it: no allocation is past `isize::MAX`.
fn signed(size: usize) -> isize {
    isize::try_from(size).unwrap_or(isize::MAX)
}

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Allocation) -> *mut u8 {
        let ptr = unsafe { System.alloc(layout) };
        if !ptr.is_null() {
            note(signed(layout.size()));
        }
        ptr
    }

    unsafe fn alloc_zeroed(&self, layout: Allocation) -> *mut u8 {
        let ptr = unsafe { System.alloc_zeroed(layout) };
        if !ptr.is_null() {
            note(signed(layout.size()));
        }
        ptr
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Allocation, new_size: usize) -> *mut u8 {
        let moved = unsafe { System.realloc(ptr, layout, new_size) };
        if !moved.is_null() {
            note(signed(new_size) - signed(layout.size()));
        }
        moved
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Allocation) {
        note(-signed(layout.size()));
        unsafe { System.dealloc(ptr, layout) }
    }
}

#[global_allocator]
static GLOBAL: Counting = Counting;

/// The bytes live on this thread.
fn live() -> isize {
    LIVE.with(Cell::get)
}

/// Ahem; Latin and Hebrew with ligatures and a kerning pair; Arabic with
/// joining forms.
fn collection() -> Collection {
    let mut latin = TestFont::new("Test Latin", &[(0x20, 0x7E), (0x5D0, 0x5EA)]);
    latin.ligatures = vec![vec!['f', 'f', 'i'], vec!['f', 'i'], vec!['f', 'l']];
    latin.kerning = vec![('A', 'V', -100)];
    let mut arabic = TestFont::new("Test Arabic", &[(0x20, 0x20), (0x621, 0x64A)]);
    arabic.joining = "\u{628}\u{62A}\u{633}\u{644}\u{645}\u{646}\u{64A}"
        .chars()
        .collect();
    let mut layer = LayerBuilder::new(Role::Application);
    assert!(layer.add_data(AHEM).is_ok());
    for font in [latin, arabic] {
        assert!(layer.add_data(font.build()).is_ok());
    }
    layer.set_fallback_override(TestFallback::new().family("Ahem"));
    Collection::new().with_layer(layer.snapshot())
}

/// The families the documents are set in.
const FAMILIES: [FontFamilyName<'static>; 2] = [
    FontFamilyName::Named(Cow::Borrowed("Test Latin")),
    FontFamilyName::Named(Cow::Borrowed("Test Arabic")),
];

/// What a document is: prose, prose whose first line is restyled, ruby, or
/// prose recorded with its offset map, its first line in capitals.
#[derive(Copy, Clone, Debug)]
enum Document {
    Prose,
    FirstLine,
    Ruby,
    Mapped,
}

/// Builds `paragraphs` paragraphs of `document` into `layout`.
///
/// Prose is Latin with ligatures, kerned pairs and soft hyphens, Hebrew and
/// Arabic, a box with edges in every paragraph, justified; its first line
/// restyled, larger and spaced, where asked. Ruby is columns with two
/// annotations and emphasis marks over a base, in the same prose. Mapped
/// prose keeps its offset map, and its first line's text in capitals.
fn build(layout: &mut Layout, cx: &mut Context, document: Document, paragraphs: u64) {
    let root = ComputedStyle {
        font: FontGroup {
            families: &FAMILIES,
            ..FontGroup::INITIAL
        },
        ..ComputedStyle::initial()
    };
    let mut boxed = root;
    boxed.edges.padding = Sides::from_px(2.5);
    boxed.paints = true;
    let mut first = ComputedStyle {
        font: FontGroup {
            size: 26.0,
            ..root.font
        },
        ..root
    };
    first.text.letter_spacing = 1.25;
    let small = ComputedStyle {
        font: FontGroup {
            size: 8.0,
            ..root.font
        },
        ..root
    };
    let mut marked = root;
    marked.text.emphasis.marks = true;
    marked.text.emphasis.position.side = EmphasisSide::Over;
    let mut capitals = root;
    capitals.text.transform.case = TextCase::Uppercase;
    let block = ComputedBlockStyle {
        text_align: TextAlign::Justify,
        first_line: match document {
            Document::FirstLine => Some(&first),
            Document::Mapped => Some(&capitals),
            Document::Prose | Document::Ruby => None,
        },
        ..ComputedBlockStyle::new(&root)
    };
    let mut options = BuildOptions::default();
    options.map_source = matches!(document, Document::Mapped);
    let mut b = layout.builder(NodeKey(0), &block, options);
    let mut key = 0;
    let mut next = || {
        key += 1;
        NodeKey(key)
    };
    for _ in 0..paragraphs {
        b.text(next(), "office af\u{AD}flu\u{AD}ent AVAIL waf\u{AD}fles ");
        b.text(
            next(),
            "\u{5E9}\u{5DC}\u{5D5}\u{5DD} (12) \u{628}\u{62A}\u{633} ",
        );
        b.open_box(next(), &boxed, None);
        b.text(next(), "a box with edges ");
        b.close_box();
        if matches!(document, Document::Ruby) {
            b.open_ruby(next(), &root, None);
            b.open_box(next(), &marked, None);
            b.text(next(), "fit base");
            b.close_box();
            b.open_annotation(next(), &small, None);
            b.text(next(), "a longer reading");
            b.close_annotation();
            b.open_annotation(next(), &small, None);
            b.text(next(), "fl");
            b.close_annotation();
            b.close_ruby();
        }
        b.text(next(), " and the flat end of it all.");
        b.line_break(next());
    }
    assert!(b.finish(cx).is_complete());
}

/// Breaks `layout` at each of `widths` in turn.
fn break_each(layout: &mut Layout, cx: &mut Context, widths: &[f32]) {
    for &width in widths {
        layout.break_lines(cx, Area::new(width), &mut NoExclusions);
        assert!(layout.lines().len() > 0);
    }
}

const DOCUMENTS: [Document; 4] = [
    Document::Prose,
    Document::FirstLine,
    Document::Ruby,
    Document::Mapped,
];
const WIDTHS: [f32; 3] = [240.0, 97.5, 600.0];

/// A layout's record, what it takes beside what it keeps on the heap, is
/// 1,376 bytes at most on a 64-bit target.
///
/// - The stages' first-line variants are boxed, made only where a layout
///   has one.
/// - The tables a plain label, article or sample never fills are boxed too,
///   one box a type, made only where a layout has any: the content's offset
///   map, side text, atomic inlines, floats and absolutely positioned boxes;
///   the feature, variation and hyphen lists; the generated text's fonts;
///   the measured text's edge costs, shifts, em boxes, ruby and letter; line
///   layout's justified lines and static positions. So a small layout, whose
///   record is most of it, holds their pointers alone.
/// - The used fonts are one table, each sharing its instance's record.
/// - The analysis keeps no table of levels, which its runs hold. The levels
///   of the bidi controls that split a line's runs take 8, boxed.
/// - The content's facts take 120 bytes: the four tables the builder lowers
///   the styles into, the boxed table of percentage edges, and the block's
///   own facts. The measure stage's text
///   metrics take 24, their table's.
/// - The layout keeps no style table. The builder's memo of the styles it
///   lowered takes 8, boxed, and its index of atomic inline keys 8, boxed.
/// - The shaped text keeps each cluster's advance, 24 bytes, so that new
///   box sizes are measured without shaping again.
/// - The lines keep how far the first line's annotations move an initial
///   letter: 4 bytes, 8 with padding.
///
/// The whole of a layout is this and its heap.
#[test]
#[cfg(target_pointer_width = "64")]
fn a_layout_record_is_1376_bytes_at_most() {
    let record = size_of::<Layout>();
    assert!(record <= 1376, "a layout's record is {record} bytes");
}

/// A layout never built keeps nothing.
#[test]
fn a_new_layout_keeps_nothing() {
    let before = live();
    let layout = Layout::new();
    assert_eq!(live(), before);
    assert_eq!(layout.heap_bytes(), LayoutHeap::default());
    assert_eq!(layout.heap_bytes().total(), 0);
}

/// Built into a new layout in a warm context, and broken at several widths,
/// each document leaves live exactly what the layout says it keeps: every
/// table is counted, and nothing is counted twice.
#[test]
fn a_layout_keeps_what_it_says_it_keeps() {
    for document in DOCUMENTS {
        let mut cx = Context::new(collection());
        let mut warm = Layout::new();
        build(&mut warm, &mut cx, document, 12);
        break_each(&mut warm, &mut cx, &WIDTHS);
        let before = live();
        let mut layout = Layout::new();
        build(&mut layout, &mut cx, document, 12);
        break_each(&mut layout, &mut cx, &WIDTHS);
        let kept = live() - before;
        let heap = layout.heap_bytes();
        assert_eq!(
            isize::try_from(heap.total()).ok(),
            Some(kept),
            "{document:?}: {heap:?}"
        );
        // Every stage the document reaches holds something; the builder's
        // containers were open while it ran.
        for (stage, bytes) in [
            ("content", heap.content),
            ("analysis", heap.analysis),
            ("fonts", heap.fonts),
            ("shaped", heap.shaped),
            ("measured", heap.measured),
            ("lines", heap.lines),
            ("builder", heap.builder),
        ] {
            assert!(bytes > 0, "{document:?}: {stage} keeps nothing");
        }
        // And the two layouts of one document keep the same.
        assert_eq!(heap, warm.heap_bytes(), "{document:?}");
    }
}

/// A warm rebuild of the same content, and a relayout at widths broken at
/// before, keep every stage's capacity where it was, and the context's too.
#[test]
fn a_warm_rebuild_keeps_the_same_capacity() {
    for document in DOCUMENTS {
        let mut cx = Context::new(collection());
        let mut layout = Layout::new();
        build(&mut layout, &mut cx, document, 12);
        break_each(&mut layout, &mut cx, &WIDTHS);
        let (heap, context) = (layout.heap_bytes(), cx.heap_bytes());
        for _ in 0..2 {
            build(&mut layout, &mut cx, document, 12);
            assert_eq!(layout.heap_bytes(), heap, "{document:?}: rebuilt");
            break_each(&mut layout, &mut cx, &WIDTHS);
            assert_eq!(layout.heap_bytes(), heap, "{document:?}: broken again");
            assert_eq!(cx.heap_bytes(), context, "{document:?}: the context");
        }
        // Less content in the same layout keeps the capacity the more took.
        build(&mut layout, &mut cx, document, 3);
        break_each(&mut layout, &mut cx, &WIDTHS);
        assert_eq!(layout.heap_bytes(), heap, "{document:?}: less content");
    }
}

/// Nine times the content keeps more in every stage that grows with it, by
/// about as much: between four and a half and eighteen times, as tables
/// that grow by doubling land. The stages whose tables are made room for to
/// their length before they are filled -- the clusters, the glyph words,
/// the prefix, the runs, an entry an item -- keep nine times within a few
/// percent, where doubling would have landed them on eight.
#[test]
fn the_heap_grows_with_the_content() {
    let mut cx = Context::new(collection());
    let mut small = Layout::new();
    build(&mut small, &mut cx, Document::Prose, 7);
    break_each(&mut small, &mut cx, &[240.0]);
    let mut large = Layout::new();
    build(&mut large, &mut cx, Document::Prose, 63);
    break_each(&mut large, &mut cx, &[240.0]);
    let (small, large) = (small.heap_bytes(), large.heap_bytes());
    for (stage, less, more) in [
        ("content", small.content, large.content),
        ("analysis", small.analysis, large.analysis),
        ("shaped", small.shaped, large.shaped),
        ("measured", small.measured, large.measured),
        ("lines", small.lines, large.lines),
    ] {
        assert!(
            (9 * less / 2..=18 * less).contains(&more),
            "{stage}: {less} to {more} bytes"
        );
    }
    // The measure stage's within ten: its rare tables' box, which the soft
    // hyphens' line-edge costs make, is the same in both, and those costs,
    // one a hyphen, grow by doubling.
    for (stage, less, more, within) in [
        ("analysis", small.analysis, large.analysis, 5),
        ("shaped", small.shaped, large.shaped, 5),
        ("measured", small.measured, large.measured, 10),
    ] {
        assert!(
            (9 * less * (100 - within) / 100..=9 * less * (100 + within) / 100).contains(&more),
            "{stage}: {less} to {more} bytes, not nine times"
        );
    }
    // The used fonts and the runs' do not grow with the paragraphs.
    assert!(large.fonts < 8 * small.fonts, "{small:?} {large:?}");
    assert_eq!(small.first_line, 0);
    assert_eq!(large.first_line, 0);
}

/// The first line's variant is counted on its own, and only where the
/// first line is set otherwise: it covers the first paragraph, so it keeps
/// less than the text's own stages of a document of several.
#[test]
fn the_first_line_variant_is_counted_apart() {
    let mut cx = Context::new(collection());
    let mut plain = Layout::new();
    build(&mut plain, &mut cx, Document::Prose, 12);
    let mut restyled = Layout::new();
    build(&mut restyled, &mut cx, Document::FirstLine, 12);
    let (plain, restyled) = (plain.heap_bytes(), restyled.heap_bytes());
    assert_eq!(plain.first_line, 0);
    assert!(restyled.first_line > 0);
    assert!(restyled.first_line < restyled.shaped + restyled.measured);
    // Its own stages, the first line aside, keep what the plain one's do.
    assert_eq!(restyled.analysis, plain.analysis);
    assert_eq!(restyled.shaped, plain.shaped);
}

/// A context keeps its caches and every stage's scratch, and no more than
/// the allocator says it took: what harfrust and fontwich keep inside the
/// values it holds is theirs, uncounted.
#[test]
fn a_context_keeps_no_more_than_it_took() {
    let fonts = collection();
    let before = live();
    let mut cx = Context::new(fonts);
    let new = cx.heap_bytes();
    // Nothing made or shaped yet.
    assert_eq!(new.total(), 0, "a new context: {new:?}");
    let mut layout = Layout::new();
    for document in DOCUMENTS {
        build(&mut layout, &mut cx, document, 12);
        break_each(&mut layout, &mut cx, &WIDTHS);
    }
    let took = live() - before - isize::try_from(layout.heap_bytes().total()).unwrap_or(0);
    let heap: ContextHeap = cx.heap_bytes();
    assert!(heap.shaping > 0 && heap.scratch > 0, "{heap:?}");
    assert!(heap.fonts > new.fonts, "{heap:?}");
    assert!(
        isize::try_from(heap.total()).is_ok_and(|total| total <= took),
        "{heap:?} against {took} bytes taken"
    );
}
