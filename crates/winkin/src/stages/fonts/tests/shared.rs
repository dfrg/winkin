//! Tests of fonts shared between contexts:
//! - sharing contexts set text as private ones do, on one thread or two;
//! - a second sharing context parses no font;
//! - the set keeps at most its limit;
//! - retaining a collection drops a removed web font.

use std::thread;

use super::*;
use crate::{Area, NoExclusions, SharedFonts};

/// Ahem, Test Han Acute and Test Three Axes; and those under a web font,
/// Test Plain Hyphen, in a document layer.
fn test_fonts() -> (Collection, Collection) {
    let base = collection(&[han(), variable()], han_fallback("Test Han Acute"));
    let mut document = LayerBuilder::new(Role::Document);
    assert!(document.add_data(plain().build()).is_ok());
    let with_web = base.clone().with_layer(document.snapshot());
    (base, with_web)
}

/// Every glyph's id, position and advance.
type Glyphs = Vec<(u32, f32, f32, f32)>;

/// Sets Latin, Han, a bold variable font and the web font in `cx`, in lines
/// 120 pixels long, and returns every glyph.
fn set(cx: &mut Context) -> Glyphs {
    let ahem = [FontFamilyName::named("Ahem")];
    let axes = [FontFamilyName::named("Test Three Axes")];
    let web = [FontFamilyName::named("Test Plain Hyphen")];
    let root = families_style(&ahem);
    let mut bold = families_style(&axes);
    bold.font.weight = FontWeight::new(700.0);
    let plain = families_style(&web);
    let spans = [
        (&root, "ab 漢字 cd "),
        (&bold, "bold text "),
        (&plain, "web"),
    ];
    let mut layout = Layout::new();
    let block = ComputedBlockStyle::new(&root);
    let mut b = layout.builder(NodeKey(0), &block, BuildOptions::default());
    for (at, (style, text)) in (0u64..).zip(spans) {
        b.open_box(NodeKey(2 * at + 1), style, None);
        b.text(NodeKey(2 * at + 2), text);
        b.close_box();
    }
    assert!(b.finish(cx).is_complete());
    layout.break_lines(cx, Area::new(120.0), &mut NoExclusions);
    let mut glyphs = Vec::new();
    for line in layout.lines() {
        for item in line.items() {
            if let crate::Item::Text(run) = item {
                glyphs.extend(run.glyphs().map(|g| (g.id, g.x, g.y, g.advance)));
            }
        }
    }
    glyphs
}

/// Two contexts sharing fonts set text as a private context does, glyph for
/// glyph, whichever parsed the fonts.
#[test]
fn contexts_sharing_fonts_set_text_as_private_ones_do() {
    let (_, fonts) = test_fonts();
    let private = set(&mut Context::new(fonts.clone()));
    assert!(private.len() > 10);
    let shared = SharedFonts::default();
    let mut first = Context::with_shared_fonts(fonts.clone(), shared.clone());
    let mut second = Context::with_shared_fonts(fonts, shared);
    assert_eq!(set(&mut first), private);
    assert_eq!(set(&mut second), private);
    assert_eq!(set(&mut second), private, "warm");
}

/// Contexts on two threads sharing one set of fonts set text as a private
/// context does.
#[test]
fn contexts_on_two_threads_share_fonts() {
    let (_, fonts) = test_fonts();
    let private = set(&mut Context::new(fonts.clone()));
    let shared = SharedFonts::default();
    let results: Vec<Glyphs> = thread::scope(|scope| {
        let threads: Vec<_> = (0..2)
            .map(|_| {
                let (fonts, shared) = (fonts.clone(), shared.clone());
                scope.spawn(move || set(&mut Context::with_shared_fonts(fonts, shared)))
            })
            .collect();
        threads
            .into_iter()
            .map(|thread| thread.join().expect("no panic"))
            .collect()
    });
    assert_eq!(results, [private.clone(), private]);
}

/// A second context sharing fonts with one that set the same text parses
/// no font: it cuts its instances from the faces the first made.
#[test]
fn a_second_sharing_context_parses_no_font() {
    let (_, fonts) = test_fonts();
    let shared = SharedFonts::default();
    let mut first = Context::with_shared_fonts(fonts.clone(), shared.clone());
    set(&mut first);
    let (held, made) = shared.counts();
    assert_eq!(
        (held, made),
        (4, 4),
        "Ahem, Han, the variable font, the web font"
    );
    let mut second = Context::with_shared_fonts(fonts, shared.clone());
    set(&mut second);
    assert_eq!(shared.counts(), (4, 4));
    assert_eq!(second.font_context().counts()[3], 4, "its own faces");
    // Clearing a context's caches keeps it sharing.
    second.clear_caches();
    set(&mut second);
    assert_eq!(shared.counts(), (4, 4));
    assert!(shared.heap_bytes() > 0);
}

/// The set keeps at most its limit, dropping the fonts asked for longest
/// ago; a context still sets text in every font.
#[test]
fn shared_fonts_keep_at_most_their_limit() {
    let (_, fonts) = test_fonts();
    let private = set(&mut Context::new(fonts.clone()));
    let shared = SharedFonts::new(2);
    let mut cx = Context::with_shared_fonts(fonts.clone(), shared.clone());
    assert_eq!(set(&mut cx), private);
    assert_eq!(shared.counts(), (2, 4));
    let none = SharedFonts::new(0);
    assert_eq!(
        set(&mut Context::with_shared_fonts(fonts, none.clone())),
        private
    );
    assert_eq!(none.counts(), (0, 4));
}

/// Retaining a collection without a web font drops that font from the set,
/// and keeps the rest.
#[test]
fn retaining_a_collection_drops_a_removed_web_font() {
    let shared = SharedFonts::default();
    let (base, fonts) = test_fonts();
    set(&mut Context::with_shared_fonts(
        fonts.clone(),
        shared.clone(),
    ));
    shared.retain(&fonts);
    assert_eq!(shared.counts(), (4, 4));
    shared.retain(&base);
    assert_eq!(shared.counts(), (3, 4), "the web font goes");
    let (other, _) = test_fonts();
    shared.retain(&other);
    assert_eq!(
        shared.counts(),
        (0, 4),
        "another layer of the same fonts holds none"
    );
}
