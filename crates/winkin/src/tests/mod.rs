//! What the stages' tests share: the fonts built in memory and the families
//! naming them, a [`Fixture`] that builds layouts and breaks them into lines
//! over those fonts, checking each stage's data as it goes, the styles most
//! tests set text in, and the values they compare against.
//!
//! Each stage keeps its own fonts where no other stage's tests set text in
//! them, and its own `check`, which only its data needs.

use alloc::borrow::Cow;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;
use std::fs;
use std::path::Path;
use std::println;

use fontwich::{Collection, FontKey};

use crate::data::Id;
use crate::stages::analysis::ClusterId;
use crate::stages::lines::{Area, Exclusions, InlineExtents, NoExclusions};
use crate::style::{
    ComputedStyle, FontFamilyName, FontGroup, LineGroup, TabSize, TextWrapMode, VerticalAlign,
    WhiteSpaceCollapse,
};
use crate::unit::{InlineLayoutUnit, LayoutUnit};
use crate::{
    BuildOptions, BuildReport, ComputedBlockStyle, Context, CrossExtents, Layout, LayoutBuilder,
    NodeKey,
};

// Fonts built in memory, and Ahem, which the integration tests build with
// too.
pub(crate) use testing::fonts::{
    AHEM, Form, TestAxis, TestFallback, TestFont, TestVertical, VARIATION_FONTS, ahem_fallback,
    collection, han, han_fallback, latin,
};

// Fonts --------------------------------------------------------------------

/// Arabic letters and a space, with initial, medial and final forms of the
/// dual-joining ones used here.
pub(crate) fn arabic() -> TestFont {
    let mut font = TestFont::new("Test Arabic", &[(0x20, 0x20), (0x621, 0x64A)]);
    font.joining = vec![BEH, TEH, SEEN, MEEM];
    font
}

pub(crate) const BEH: char = '\u{628}';
pub(crate) const TEH: char = '\u{62A}';
pub(crate) const SEEN: char = '\u{633}';
pub(crate) const MEEM: char = '\u{645}';

pub(crate) const AHEM_FAMILY: [FontFamilyName<'static>; 1] =
    [FontFamilyName::Named(Cow::Borrowed("Ahem"))];
pub(crate) const LATIN: [FontFamilyName<'static>; 1] =
    [FontFamilyName::Named(Cow::Borrowed("Test Latin"))];
pub(crate) const ARABIC: [FontFamilyName<'static>; 1] =
    [FontFamilyName::Named(Cow::Borrowed("Test Arabic"))];
pub(crate) const NARROW: [FontFamilyName<'static>; 1] =
    [FontFamilyName::Named(Cow::Borrowed("Test Narrow"))];
pub(crate) const PUNCT: [FontFamilyName<'static>; 1] =
    [FontFamilyName::Named(Cow::Borrowed("Test Punct"))];
pub(crate) const PLAIN_PUNCT: [FontFamilyName<'static>; 1] =
    [FontFamilyName::Named(Cow::Borrowed("Test Punct Plain"))];

// The fixture --------------------------------------------------------------

/// A context over fonts built in memory, which a stage's tests build their
/// layouts in and break them into lines through, running the stage's check
/// on each as its [`StageCheck`] says; and the families' names by key, to say
/// which draws what.
pub(crate) struct Fixture {
    pub(crate) cx: Context,
    check: StageCheck,
    names: Vec<(FontKey, String)>,
}

/// Which of a [`Fixture`]'s layouts its stage's check runs on.
#[derive(Copy, Clone)]
pub(crate) enum StageCheck {
    /// Each layout once it is built.
    Built(fn(&Layout)),
    /// Each layout once it is broken into lines.
    Placed(fn(&Layout)),
}

impl Fixture {
    /// A context over Ahem and `fonts`, falling back as `fallback` says,
    /// knowing each family by name.
    pub(crate) fn new(fonts: &[TestFont], fallback: TestFallback, check: StageCheck) -> Self {
        let mut families = vec![String::from("Ahem")];
        families.extend(fonts.iter().map(|font| font.family.clone()));
        Self::from_collection(collection(fonts, fallback), &families, check)
    }

    /// A context over `collection`, knowing `families` by name.
    pub(crate) fn from_collection(
        collection: Collection,
        families: &[String],
        check: StageCheck,
    ) -> Self {
        let names = families
            .iter()
            .filter_map(|name| {
                let family = collection.family(name)?;
                let key = family.fonts().first()?.key()?;
                Some((key, name.clone()))
            })
            .collect();
        Self {
            cx: Context::new(collection),
            check,
            names,
        }
    }

    /// Builds into `layout` in `block`, with `calls` making the content, and
    /// says what finishing said.
    pub(crate) fn build(
        &mut self,
        layout: &mut Layout,
        block: &ComputedBlockStyle<'_>,
        calls: impl FnOnce(&mut LayoutBuilder<'_>),
    ) -> BuildReport {
        let mut b = layout.builder(NodeKey(0), block, BuildOptions::default());
        calls(&mut b);
        let built = b.finish(&mut self.cx);
        if let StageCheck::Built(check) = self.check {
            check(layout);
        }
        built
    }

    /// Builds `spans` into `layout` in `block`, each an inline box in its
    /// style holding its text.
    pub(crate) fn build_spans(
        &mut self,
        layout: &mut Layout,
        block: &ComputedBlockStyle<'_>,
        spans: &[(&ComputedStyle<'_>, &str)],
    ) -> BuildReport {
        self.build(layout, block, |b| {
            for (at, (style, text)) in (0u64..).zip(spans) {
                b.open_box(NodeKey(2 * at + 1), style, None);
                b.text(NodeKey(2 * at + 2), text);
                b.close_box();
            }
        })
    }

    /// Builds `spans` into `layout` as [`build_spans`](Self::build_spans)
    /// does, in a left-to-right block whose own style is `root`.
    pub(crate) fn spans(
        &mut self,
        layout: &mut Layout,
        root: &ComputedStyle<'_>,
        spans: &[(&ComputedStyle<'_>, &str)],
    ) -> BuildReport {
        self.build_spans(layout, &ComputedBlockStyle::new(root), spans)
    }

    /// Builds `text` in an inline box of one style, in a left-to-right block
    /// of it.
    pub(crate) fn span(
        &mut self,
        layout: &mut Layout,
        style: &ComputedStyle<'_>,
        text: &str,
    ) -> BuildReport {
        self.spans(layout, style, &[(style, text)])
    }

    /// Builds `text` in `block`'s own style.
    pub(crate) fn block_text(
        &mut self,
        layout: &mut Layout,
        block: &ComputedBlockStyle<'_>,
        text: &str,
    ) {
        self.build(layout, block, |b| b.text(NodeKey(1), text));
    }

    /// Builds `text` in one style, in a left-to-right block.
    pub(crate) fn text(&mut self, layout: &mut Layout, style: &ComputedStyle<'_>, text: &str) {
        self.block_text(layout, &ComputedBlockStyle::new(style), text);
    }

    /// Breaks `layout` into lines `width` pixels long.
    pub(crate) fn lay_out(&mut self, layout: &mut Layout, width: f32) {
        self.lay_out_with(layout, Area::new(width), &mut NoExclusions);
    }

    /// Breaks `layout` into lines in `area`, around `exclusions`.
    pub(crate) fn lay_out_with(
        &mut self,
        layout: &mut Layout,
        area: Area,
        exclusions: &mut dyn Exclusions,
    ) {
        layout.break_lines(&mut self.cx, area, exclusions);
        if let StageCheck::Placed(check) = self.check {
            check(layout);
        }
    }

    /// The name of the family `font` is an instance of, where the fixture
    /// knows it.
    pub(crate) fn family_name(&self, font: FontKey) -> Option<&str> {
        self.names
            .iter()
            .find(|(key, _)| *key == font)
            .map(|(_, name)| name.as_str())
    }
}

/// A context with no fonts, which building and analysis do not read.
pub(crate) fn no_fonts() -> Context {
    Context::new(Collection::new())
}

// Styles -------------------------------------------------------------------

/// The initial style with `change` made to it.
pub(crate) fn styled(change: impl FnOnce(&mut ComputedStyle<'static>)) -> ComputedStyle<'static> {
    let mut style = ComputedStyle::initial();
    change(&mut style);
    style
}

/// The initial style, collapsing white space as `collapse` says.
pub(crate) fn white_space(collapse: WhiteSpaceCollapse) -> ComputedStyle<'static> {
    styled(|style| style.text.white_space_collapse = collapse)
}

/// The initial style, on one line however long.
pub(crate) fn nowrap() -> ComputedStyle<'static> {
    styled(|style| style.text.wrap_mode = TextWrapMode::NoWrap)
}

/// The initial style in `families`.
pub(crate) fn families_style<'a>(families: &'a [FontFamilyName<'a>]) -> ComputedStyle<'a> {
    ComputedStyle {
        font: FontGroup {
            families,
            ..FontGroup::INITIAL
        },
        ..ComputedStyle::initial()
    }
}

/// A style at `size` pixels in `families`.
pub(crate) fn sized<'a>(families: &'a [FontFamilyName<'a>], size: f32) -> ComputedStyle<'a> {
    let mut style = families_style(families);
    style.font.size = size;
    style
}

/// Ahem at `size` pixels, where every glyph is an em square.
pub(crate) fn ahem(size: f32) -> ComputedStyle<'static> {
    sized(&AHEM_FAMILY, size)
}

/// Ahem at `size` pixels under `pre-wrap`, its tab stops `spaces` apart.
pub(crate) fn tabbed(size: f32, spaces: f32) -> ComputedStyle<'static> {
    let mut style = ahem(size);
    style.text.white_space_collapse = WhiteSpaceCollapse::Preserve;
    style.text.tab_size = TabSize::Spaces(spaces);
    style
}

/// `style` with `vertical-align: align`.
pub(crate) fn vertically<'a>(style: &ComputedStyle<'a>, align: VerticalAlign) -> ComputedStyle<'a> {
    ComputedStyle {
        line: LineGroup {
            vertical_align: align,
            ..style.line
        },
        ..*style
    }
}

// Values -------------------------------------------------------------------

/// The node key `n`.
pub(crate) const fn key(n: u64) -> NodeKey {
    NodeKey(n)
}

/// Cluster `n`.
pub(crate) fn at(n: usize) -> ClusterId {
    ClusterId::new(n)
}

/// `n` pixels, as a position along a line is kept.
pub(crate) fn px(n: f32) -> InlineLayoutUnit {
    InlineLayoutUnit::from_layout(LayoutUnit::from_px(n))
}

/// Along a line, from `left` to `right`, from the line box's left.
pub(crate) fn along(left: f32, right: f32) -> InlineExtents {
    InlineExtents { left, right }
}

/// Across a line, from `over` to `under`, from the line box's over edge.
pub(crate) fn across(over: f32, under: f32) -> CrossExtents {
    CrossExtents { over, under }
}

/// Each line's text.
pub(crate) fn texts(layout: &Layout) -> Vec<String> {
    let text = &layout.content().text;
    layout
        .lines()
        .map(|line| String::from(&text[line.text_range()]))
        .collect()
}

mod caches;
mod facts;
mod fast;
mod segments;

/// The text of `name`, one of Unicode's conformance files, which the
/// workspace keeps in `support/testing/unicode/`, outside this crate. `None`,
/// after a note, where the file is absent, as it is in a packaged crate.
pub(crate) fn unicode_test_data(name: &str) -> Option<String> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../support/testing/unicode")
        .join(name);
    match fs::read_to_string(&path) {
        Ok(text) => Some(text),
        Err(error) => {
            println!("skipped: {} is absent ({error})", path.display());
            None
        }
    }
}

// Allocations counted per thread, for the tests that hold a warm stage to
// none.
pub(crate) use testing::allocator;

#[global_allocator]
static GLOBAL: allocator::Counting = allocator::Counting;
