//! Lays out a justified paragraph with a bold span and prints its lines.
//!
//! ```text
//! cargo run -p winkin --example paragraph
//! cargo run -p winkin --example paragraph -- path/to/font.ttf 240
//! ```
//!
//! The optional arguments are a font file to set the text in and the width
//! to break at, in CSS pixels. Without a font, the text is set in Ahem, a
//! test font whose every glyph is an em square.
//!
//! For each line it prints the line box, then each item in visual order: a
//! text run's text, font size and glyphs, or an inline box's extent.

use std::borrow::Cow;

use fontwich::{Collection, LayerBuilder, Role};
use winkin::style::{ComputedStyle, FontFamilyName, FontWeight, TextAlign};
use winkin::{
    Area, BuildOptions, ComputedBlockStyle, Context, Item, Layout, NoExclusions, NodeKey,
};

const AHEM: &[u8] = include_bytes!("../../../support/testing/fonts/Ahem.ttf");

fn main() {
    let mut args = std::env::args().skip(1);
    let bytes = match args.next() {
        Some(path) => std::fs::read(&path).unwrap_or_else(|error| panic!("{path}: {error}")),
        None => AHEM.to_vec(),
    };
    let width: f32 = args
        .next()
        .and_then(|width| width.parse().ok())
        .unwrap_or(200.0);

    // The fonts the text is set in. A browser would also add the system's
    // fonts (fontwich's `system` feature) and the page's `@font-face` faces.
    let mut layer = LayerBuilder::new(Role::Application);
    let families = layer.add_data(bytes).expect("the file is a font");
    let name = families.first().expect("a font names its family").name();
    let family = [FontFamilyName::Named(Cow::Owned(name.into()))];
    let mut cx = Context::new(Collection::new().with_layer(layer.snapshot()));

    // The computed styles a host's cascade would hand in: the paragraph's,
    // and a bold span's that inherits from it.
    let mut paragraph = ComputedStyle::initial();
    paragraph.font.families = &family;
    paragraph.font.size = 16.0;
    let mut bold = paragraph;
    bold.font.weight = FontWeight::BOLD;
    let mut block = ComputedBlockStyle::new(&paragraph);
    block.text_align = TextAlign::Justify;

    // The content, in document order: `<p>A paragraph of text <b>with a bold
    // span</b>, broken into ...</p>`.
    let mut layout = Layout::new();
    let mut builder = layout.builder(&mut cx, NodeKey(0), &block, BuildOptions::default());
    builder.text(NodeKey(1), "A paragraph of text ");
    builder.open_box(NodeKey(2), &bold, None);
    builder.text(NodeKey(3), "with a bold span");
    builder.close_box();
    builder.text(
        NodeKey(4),
        ", broken into justified lines at the width asked for.",
    );
    let report = builder.finish();
    assert!(report.is_complete());

    layout.break_lines(&mut cx, Area::new(width), &mut NoExclusions);

    let text = layout.text();
    let metrics = layout.metrics();
    println!(
        "{} lines at {width} px, {} px tall",
        layout.lines().len(),
        metrics.block_end
    );
    for line in layout.lines() {
        let line_box = line.metrics();
        println!(
            "line {}: left {}, top {}, width {}, baseline {}",
            line.index(),
            line_box.left,
            line_box.top,
            line_box.width,
            line_box.baseline
        );
        for item in line.items() {
            match item {
                Item::Text(run) | Item::Generated(run) => {
                    // `None` where no font draws the run.
                    let Some(font) = run.font() else { continue };
                    let bold = if font.embolden { ", faux bold" } else { "" };
                    let glyphs: Vec<String> = run
                        .glyphs()
                        .map(|glyph| format!("{}@{}", glyph.id, glyph.x))
                        .collect();
                    println!(
                        "  text {:?} for node {}, {} px{bold}, {} px wide: {}",
                        &text[run.text_range()],
                        run.key().0,
                        font.size,
                        run.advance(),
                        glyphs.join(" ")
                    );
                }
                Item::Box(fragment) => {
                    println!(
                        "  box for node {}, {:?}",
                        fragment.key().0,
                        fragment.inline()
                    );
                }
                Item::Atomic(atomic) => {
                    println!("  atomic inline for node {}", atomic.key().0);
                }
            }
        }
    }
}
