//! The builder: a rebuild of the same content, or of similar content,
//! allocates nothing warm.

use super::count_allocations;
use winkin::style::{
    ComputedStyle, EdgesGroup, FontFamilyName, FontFeature, FontGroup, FontVariation, FontWeight,
    GenericFamily, Language, Sides, Tag, TextCase, TextGroup, TextTransform, TextWrapMode,
    UnicodeBidi, WhiteSpaceCollapse, WhiteSpaceTrim,
};
use winkin::{BoxSize, BuildOptions, ComputedBlockStyle, Context, FloatSide, NodeKey};

#[test]
fn math_auto_allocates_nothing_warm() {
    let mut layout = winkin::Layout::new();
    let mut cx = Context::new(fontwich::Collection::new());
    let mut math = ComputedStyle::initial();
    math.text.transform = TextTransform::MATH_AUTO;
    let block = ComputedBlockStyle::new(&math);
    for source in ["i", "hi", "∞"] {
        for map_source in [false, true] {
            let mut build = || {
                let mut options = BuildOptions::default();
                options.map_source = map_source;
                let mut b = layout.builder(NodeKey(0), &block, options);
                for key in 1..100 {
                    b.text(NodeKey(key), source);
                }
                assert!(b.finish(&mut cx).is_complete());
            };
            build();
            let warm = count_allocations(build);
            assert_eq!(warm, 0, "{source}, map_source={map_source}");
        }
    }
}

/// A masked field rebuilt keystroke by keystroke, as an editor rebuilds it,
/// and its lines broken and its carets walked, allocates nothing warm: with
/// and without the offset map, and under a case transform.
#[test]
fn masked_text_allocates_nothing_warm() {
    use super::test_fonts::{self, TestFont, ahem_fallback};
    use winkin::selection::{Granularity, MotionDirection, Position, Selection};
    use winkin::style::TextSecurity;
    let masks = TestFont::new(
        "Test Masks",
        &[(0x2022, 0x2022), (0x25A0, 0x25A0), (0x25E6, 0x25E6)],
    );
    let mut cx = Context::new(test_fonts::collection(&[masks], ahem_fallback()));
    let mut layout = winkin::Layout::new();
    let families = [FontFamilyName::named("Ahem")];
    let mut style = ComputedStyle::initial();
    style.font.families = &families;
    style.text.security = TextSecurity::Disc;
    style.text.white_space_collapse = WhiteSpaceCollapse::Preserve;
    let mut upper = style;
    upper.text.transform.case = TextCase::Uppercase;
    let password = "hunter2 e\u{301}\u{1F600} stra\u{DF}e";
    for style in [style, upper] {
        let block = ComputedBlockStyle::new(&style);
        for map_source in [false, true] {
            let mut typed = 0;
            let mut keystroke = || {
                typed = typed % password.len() + 1;
                while !password.is_char_boundary(typed) {
                    typed += 1;
                }
                let mut options = BuildOptions::default();
                options.map_source = map_source;
                let mut b = layout.builder(NodeKey(0), &block, options);
                b.text(NodeKey(1), &password[..typed]);
                assert!(b.finish(&mut cx).is_complete());
                super::relayout(&mut layout, &mut cx, &[400.0, 60.0]);
                let mut caret = Selection::from(Position::from(0));
                for _ in 0..4 {
                    caret.modify(
                        &layout,
                        MotionDirection::Forward.moving(Granularity::Character),
                    );
                }
                let _ = layout.node_position(caret.focus());
            };
            for _ in 0..password.len() {
                keystroke();
            }
            let warm = count_allocations(&mut keystroke);
            assert_eq!(warm, 0, "map_source={map_source}");
        }
    }
}

/// The styles a document is set in, with lists the way an engine holds
/// them: its own, lent for each call.
struct Styles {
    families: Vec<FontFamilyName<'static>>,
    code: Vec<FontFamilyName<'static>>,
    features: Vec<FontFeature>,
    variations: Vec<FontVariation>,
}

impl Styles {
    fn new() -> Self {
        Self {
            families: vec![
                FontFamilyName::Named("Georgia".to_string().into()),
                FontFamilyName::Generic(GenericFamily::Serif),
            ],
            code: vec![
                FontFamilyName::Named("Consolas".to_string().into()),
                FontFamilyName::Generic(GenericFamily::Monospace),
            ],
            features: vec![FontFeature::new(Tag::new(b"smcp"), 1)],
            variations: vec![FontVariation::new(Tag::new(b"wght"), 650.0)],
        }
    }

    fn root(&self) -> ComputedStyle<'_> {
        ComputedStyle {
            font: FontGroup {
                families: &self.families,
                ..FontGroup::INITIAL
            },
            text: TextGroup {
                language: Language::parse("en").ok(),
                ..TextGroup::INITIAL
            },
            ..ComputedStyle::initial()
        }
    }
}

/// A document's text, formatted before anything is counted.
struct Words {
    plain: Vec<String>,
    padded: Vec<String>,
    kept: Vec<String>,
    spaced: Vec<String>,
}

impl Words {
    fn new(count: usize, word: &str) -> Self {
        let plain: Vec<String> = (0..count).map(|n| format!("{word}{n}")).collect();
        Self {
            padded: plain.iter().map(|w| format!("  {w}\n\t")).collect(),
            kept: plain.iter().map(|w| format!("{w}  \n  {w}")).collect(),
            spaced: plain.iter().map(|w| format!("{w} ")).collect(),
            plain,
        }
    }
}

/// A document using every call the builder takes and every white space
/// rule, with `words` as its text.
fn document(layout: &mut winkin::Layout, cx: &mut Context, styles: &Styles, words: &Words) {
    let root = styles.root();
    let first_line = ComputedStyle {
        font: FontGroup {
            size: 20.0,
            ..root.font
        },
        ..root
    };
    let bold = ComputedStyle {
        font: FontGroup {
            features: &styles.features,
            weight: FontWeight::BOLD,
            ..root.font
        },
        ..root
    };
    let code = ComputedStyle {
        font: FontGroup {
            families: &styles.code,
            variations: &styles.variations,
            ..root.font
        },
        ..root
    };
    let mut pre = code;
    pre.text.white_space_collapse = WhiteSpaceCollapse::Preserve;
    let mut nowrap = root;
    nowrap.text.wrap_mode = TextWrapMode::NoWrap;
    let mut boxed = root;
    boxed.edges = EdgesGroup {
        padding: Sides::from_px(3.0),
        ..EdgesGroup::INITIAL
    };
    boxed.bidi.unicode_bidi = UnicodeBidi::Isolate;
    boxed.text.white_space_trim = WhiteSpaceTrim {
        discard_inner: true,
        ..WhiteSpaceTrim::NONE
    };
    let mut annotation = root;
    annotation.text.language = Language::parse("ja").ok();
    annotation.text.hyphenate_character = Some("=");
    let mut kept_lines = root;
    kept_lines.text.white_space_collapse = WhiteSpaceCollapse::PreserveBreaks;
    // Transforms: capitals, which may grow the text, capitalize, which
    // reads the text before, and full width over kept spaces.
    let mut upper = root;
    upper.text.transform.case = TextCase::Uppercase;
    let mut capital = root;
    capital.text.transform.case = TextCase::Capitalize;
    let mut wide = pre;
    wide.text.transform.full_width = true;
    // A first letter larger and in capitals, and larger again on the
    // first line.
    let mut letter = upper;
    letter.font.size = 48.0;
    let mut first_letter = letter;
    first_letter.font.size = 60.0;

    let block = ComputedBlockStyle {
        first_line: Some(&first_line),
        ..ComputedBlockStyle::new(&root)
    };
    let mut b = layout.builder(NodeKey(0), &block, BuildOptions::default());
    let mut key = 0;
    let mut next = || {
        key += 1;
        NodeKey(key)
    };
    // Its punctuation in a text of its own, and its letter in the next,
    // the first of the words.
    b.set_first_letter(next(), &letter, Some(&first_letter));
    b.text(next(), "\u{201C} ");
    for (at, word) in words.plain.iter().enumerate() {
        match at % 8 {
            0 => {
                b.open_box(next(), &bold, Some(&first_line));
                b.text(next(), word);
                b.close_box();
                b.text(next(), "  ");
            }
            1 => {
                b.open_box(next(), &pre, None);
                b.text(next(), &words.padded[at]);
                b.close_box();
            }
            2 => {
                b.open_box(next(), &nowrap, None);
                b.text(next(), &words.spaced[at]);
                b.close_box();
                b.text(next(), " \n ");
                b.break_opportunity();
            }
            3 => {
                b.atomic(
                    next(),
                    &boxed,
                    None,
                    BoxSize {
                        inline: 12.0,
                        block: 10.0,
                        baseline: Some(8.0),
                    },
                );
                b.float(next(), &root, FloatSide::Right, BoxSize::default());
                b.text(next(), word);
            }
            4 => {
                b.open_ruby(next(), &root, None);
                b.text(next(), word);
                b.open_annotation(next(), &annotation, None);
                b.text(next(), word);
                b.close_annotation();
                b.close_ruby();
            }
            5 => {
                b.open_box(next(), &boxed, None);
                b.text(next(), &words.padded[at]);
                b.close_box();
                b.line_break(next());
            }
            6 => {
                b.open_box(next(), &kept_lines, None);
                b.text(next(), &words.kept[at]);
                b.close_box();
                b.open_box(next(), &upper, None);
                b.text(next(), "straße ");
                b.close_box();
                b.open_box(next(), &capital, None);
                b.text(next(), word);
                b.text(next(), " o'brien");
                b.close_box();
                b.open_box(next(), &wide, None);
                b.text(next(), &words.spaced[at]);
                b.close_box();
            }
            _ => {
                b.text(next(), " ");
                b.text(next(), word);
                b.text(next(), "\u{200B}\n");
            }
        }
    }
    // One left open, for finishing to close.
    b.open_box(next(), &boxed, None);
    let built = b.finish(cx);
    assert!(built.is_complete());
}

/// A warm rebuild allocates nothing, of the same content or of similar
/// content no larger: every table, the style arenas and their hash
/// tables keep their capacity.
#[test]
fn a_rebuild_allocates_nothing_warm() {
    let styles = Styles::new();
    let mut cx = Context::new(fontwich::Collection::new());
    let mut layout = winkin::Layout::new();
    let same = Words::new(200, "word");
    // Similar: other words, no longer, in the same styles.
    let similar = Words::new(180, "mot");
    let cold = count_allocations(|| document(&mut layout, &mut cx, &styles, &same));
    assert!(cold > 0, "a cold layout grows, so the count moves");
    let warm = count_allocations(|| document(&mut layout, &mut cx, &styles, &same));
    assert_eq!(warm, 0, "rebuilding the same content allocated");
    let warm = count_allocations(|| document(&mut layout, &mut cx, &styles, &similar));
    assert_eq!(warm, 0, "rebuilding similar content allocated");
    // And a layout can take a whole other document and come back.
    document(&mut layout, &mut cx, &styles, &Words::new(20, "x"));
    let warm = count_allocations(|| document(&mut layout, &mut cx, &styles, &same));
    assert_eq!(warm, 0, "rebuilding after a smaller document allocated");
}
