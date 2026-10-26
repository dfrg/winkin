//! Choosing a font from a family: CSS Fonts 4 font matching (section 5.2), and
//! what to synthesize for what the chosen font lacks.
//!
//! The steps are the spec's: width first, then style, then weight, each
//! narrowing the fonts the next step considers. Where fontique's
//! implementation departs from the spec, the spec is followed:
//!
//! - **Styles are an italic value and an oblique angle**, told apart. An
//!   upright font is italic 0 and oblique 0deg; an italic font is italic 1;
//!   an oblique font is its angle. An italic request tries italic values, then
//!   oblique angles, and upright fonts last; fontique ranks them on one scale.
//! - **The oblique threshold is 14deg**, Blink's `kItalicThreshold`, where
//!   the spec now says 11deg. An italic request with no italic to offer
//!   aims at 14deg among obliques, Blink's `kItalicSlopeValue`, so
//!   a face whose range holds 14deg ties with any other that does, and
//!   weight decides.
//! - **Shallow oblique requests look shallower first**, then steeper, then at
//!   italics in the same order; and with `font-synthesis-style: auto`, an
//!   upright font to shear comes before them, but only in a family with no
//!   italic or oblique font at all, as the spec's note on synthesis says.
//! - **The `ital` axis never satisfies an oblique request.**
//! - **Negative angles mirror positive ones**, as the spec says, rather than
//!   being written out case by case.
//!
//! And two things the spec asks for that fontique does not do:
//!
//! - **Variable fonts match by range.** A font with a `wght` axis offers every
//!   weight the axis covers, likewise `wdth` for width, `slnt` for oblique
//!   angles and `ital` for italic values. A value inside a font's range
//!   matches it exactly; outside, the font competes as the nearest value its
//!   range reaches.
//! - **Nothing is allocated.** Keeping the fonts tied at each step is the same
//!   as taking the least of each font's width, style and weight ranks, in
//!   that order, so a match is one pass over the family's fonts rather than a
//!   list of candidates collected and filtered three times.

use parlance::{FontStyle, FontVariation, Tag};

use super::{Attributes, Axis, FaceStyle, Font};

/// The angle `oblique` means when no angle is given, and the one a faux italic
/// leans by.
const DEFAULT_OBLIQUE: f32 = 14.0;

/// The threshold between shallow and steep oblique requests, and the oblique
/// angle an italic value of 1 corresponds to: Blink's `kItalicThreshold` and
/// `kItalicSlopeValue`.
const THRESHOLD: f32 = 14.0;

/// Every font in `fonts` tied for the best match, by index: in order, or
/// from the last when `reverse`.
///
/// `synthesize_style` is CSS's `font-synthesis-style: auto`: with it, an
/// oblique request in a family with nothing italic or oblique matches an
/// upright font, to be sheared.
///
/// More than one where fonts are alike for the request: two files
/// describing themselves the same, or the faces of a family CSS splits by
/// `unicode-range`, which a caller tries in turn for a character.
pub(crate) fn matching(
    fonts: &[Font],
    request: Attributes,
    synthesize_style: bool,
    reverse: bool,
) -> impl Iterator<Item = usize> + '_ {
    let ranker = Ranker::new(fonts, request, synthesize_style);
    // One pass finds the best and whether anything ties it. Usually nothing
    // does, and that font is the answer; a second pass collects ties.
    let mut best: Option<(usize, u64)> = None;
    let mut tied = false;
    for (at, font) in fonts.iter().enumerate() {
        let bound = best.map_or(u64::MAX, |(_, rank)| rank);
        match ranker.rank(font, bound) {
            Some(rank) if best.is_none_or(|(_, held)| rank < held) => {
                best = Some((at, rank));
                tied = false;
            }
            Some(rank) if best.is_some_and(|(_, held)| rank == held) => tied = true,
            _ => {}
        }
    }
    let alone = best.filter(|_| !tied).map(|(at, _)| at);
    let order = (0..fonts.len()).map(move |at| if reverse { fonts.len() - 1 - at } else { at });
    alone.into_iter().chain(order.filter(move |&at| {
        tied && best.is_some_and(|(_, best)| ranker.rank(&fonts[at], best) == Some(best))
    }))
}

/// Ranks fonts for one request.
///
/// The spec narrows by width, then style, then weight, keeping the fonts
/// tied at each step: the same as the least of the three ranks taken in that
/// order, which one pass finds. The three are packed into one integer, width
/// highest, so that order is the integer's.
struct Ranker {
    width: f32,
    weight: f32,
    style: FontStyle,
    synthesize_style: bool,
    /// Most fonts are static, and a static upright or italic font's style
    /// rank is the same as every other's: those two are ranked once.
    upright: Rank,
    italic: Rank,
}

impl Ranker {
    fn new(fonts: &[Font], request: Attributes, synthesize_style: bool) -> Self {
        // CSS shears an upright font for an oblique request only where the
        // family has no italic or oblique font to offer: "for families that
        // lack any italic or oblique faces". Asked only when it matters.
        let synthesize_style = synthesize_style
            && matches!(request.style, FontStyle::Oblique(_))
            && !fonts.iter().any(leans);
        let style_of = |styles| style_rank(styles, request.style, synthesize_style);
        Self {
            width: request.width.ratio() * 100.0,
            weight: request.weight.value(),
            style: request.style,
            synthesize_style,
            upright: style_of(static_styles(FontStyle::Normal)),
            italic: style_of(static_styles(FontStyle::Italic)),
        }
    }

    /// `font`'s rank, or `None` where it is already behind `bound` at some
    /// step: a font behind the best at one step is not ranked at the next.
    fn rank(&self, font: &Font, bound: u64) -> Option<u64> {
        // A variable font's axes, looked through once, and a face's
        // descriptors; a static font needs only its attributes.
        let ranged =
            (!font.axes().is_empty() || font.face.is_some()).then(|| Ranges::from_font(font));
        let point = |value: f32| (value, value);
        let widths = ranged.as_ref().map_or_else(
            || point(font.width().ratio() * 100.0),
            |ranges| ranges.width,
        );
        let by_width = u64::from(width_rank(widths, self.width)) << (2 * RANK_BITS);
        if by_width > bound {
            return None;
        }
        let style_of = |styles| style_rank(styles, self.style, self.synthesize_style);
        let by_style = match (&ranged, font.style()) {
            (Some(ranges), _) => style_of(ranges.styles),
            (None, FontStyle::Normal) => self.upright,
            (None, FontStyle::Italic) => self.italic,
            (None, oblique) => style_of(static_styles(oblique)),
        };
        let by_style = u64::from(by_style) << RANK_BITS;
        if by_width | by_style > bound {
            return None;
        }
        let weights = ranged
            .as_ref()
            .map_or_else(|| point(font.weight().value()), |ranges| ranges.weight);
        Some(by_width | by_style | u64::from(weight_rank(weights, self.weight)))
    }
}

/// Where a font stands for one step: a tier, then a key within it, lower
/// first, packed into [`RANK_BITS`] bits. Fonts tied on both go on to the next
/// step.
type Rank = u32;

/// A tier above, a key below.
const RANK_BITS: u32 = 20;
const KEY_BITS: u32 = 16;

/// The rank of a font that has what was asked for.
const EXACT: Rank = 0;

/// A rank from a tier and a key, which is never negative. The key is kept to
/// a 64th, and at most 1023: a width or weight 1023 away from the request
/// ranks with anything further.
///
/// A key that is not a number comes from a request that is not one, which a
/// caller can make: the cast takes it to 0, so every font is as near as any
/// other, and matching answers rather than panicking.
fn rank(tier: u8, key: f32) -> Rank {
    debug_assert!((key >= 0.0 || key.is_nan()) && tier < 1 << (RANK_BITS - KEY_BITS));
    let key = ((key * 64.0) as u32).min((1 << KEY_BITS) - 1);
    (u32::from(tier) << KEY_BITS) | key
}

/// What a font reaches: a range for each of width and weight, and its
/// styles. A static font's ranges are single values; a variable font's are
/// its axes', found in one look through them.
struct Ranges {
    width: (f32, f32),
    weight: (f32, f32),
    styles: Styles,
}

impl Ranges {
    fn from_font(font: &Font) -> Self {
        let width = font.width().ratio() * 100.0;
        let weight = font.weight().value();
        let mut ranges = Self {
            width: (width, width),
            weight: (weight, weight),
            styles: static_styles(font.style()),
        };
        let mut slnt = None;
        for axis in font.axes() {
            // In CSS's units, whatever the axis states its own in: a GX
            // `wght` of 0.48 to 3.2 is 192 to 1280, and matching a request
            // against 0.48 to 3.2 would answer every one of them with the
            // heaviest the font has.
            let scale = super::attributes::css_scale(axis);
            let range = (axis.min * scale, axis.max * scale);
            match &axis.tag.to_bytes() {
                b"wdth" => ranges.width = range,
                b"wght" => ranges.weight = range,
                b"ital" => ranges.styles.italic = Some(range),
                b"slnt" => slnt = Some(range),
                _ => {}
            }
        }
        // `slnt` counts counter-clockwise, CSS clockwise. A font that slants
        // is upright at 0, so has italic 0 even with no `ital` axis.
        if let Some((min, max)) = slnt {
            ranges.styles.oblique = Some((-max, -min));
            ranges.styles.italic.get_or_insert((0.0, 0.0));
        }
        // What an `@font-face` rule declares is what the face is, for
        // matching and for how far its axes may go.
        if let Some(face) = &font.face {
            let d = &face.descriptors;
            if let Some((lo, hi)) = d.weight {
                ranges.weight = ordered(lo.value(), hi.value());
            }
            if let Some((lo, hi)) = d.width {
                ranges.width = ordered(lo.ratio() * 100.0, hi.ratio() * 100.0);
            }
            match d.style {
                None => {}
                Some(FaceStyle::Normal) => ranges.styles = static_styles(FontStyle::Normal),
                Some(FaceStyle::Italic) => ranges.styles = static_styles(FontStyle::Italic),
                Some(FaceStyle::Oblique(lo, hi)) => {
                    ranges.styles = Styles {
                        italic: None,
                        oblique: Some(ordered(lo, hi)),
                    }
                }
            }
        }
        ranges
    }
}

/// CSS: an exact width first; then, for a condensed or normal request,
/// narrower widths nearest first, then wider; for an expanded request, the
/// other way about.
fn width_rank((lo, hi): (f32, f32), width: f32) -> Rank {
    let near = clamp(width, lo, hi);
    if near == width {
        EXACT
    } else if (near < width) == (width <= 100.0) {
        rank(1, (width - near).abs())
    } else {
        rank(2, (width - near).abs())
    }
}

/// CSS: the weight asked for first. Then, for 400 to 500, heavier up to 500,
/// then lighter, then heavier past 500; below 400, lighter then heavier;
/// above 500, heavier then lighter. Nearest first within each.
fn weight_rank((lo, hi): (f32, f32), weight: f32) -> Rank {
    let near = clamp(weight, lo, hi);
    let distance = (weight - near).abs();
    if near == weight {
        EXACT
    } else if (400.0..=500.0).contains(&weight) {
        if near > weight && near <= 500.0 {
            rank(1, distance)
        } else if near < weight {
            rank(2, distance)
        } else {
            rank(3, distance)
        }
    } else if (weight < 400.0) == (near < weight) {
        rank(1, distance)
    } else {
        rank(2, distance)
    }
}

/// A font's styles as the spec sees them: the italic values it reaches and
/// the oblique angles it reaches, in CSS's clockwise degrees. `None` where it
/// has no such value at all: an italic font is not an oblique one.
#[derive(Copy, Clone)]
struct Styles {
    italic: Option<(f32, f32)>,
    oblique: Option<(f32, f32)>,
}

/// Whether `font` is, or can be, italic or oblique.
fn leans(font: &Font) -> bool {
    let styles = if font.axes().is_empty() && font.face.is_none() {
        static_styles(font.style())
    } else {
        Ranges::from_font(font).styles
    };
    styles.italic.is_some_and(|(_, hi)| hi > 0.0)
        || styles
            .oblique
            .is_some_and(|(lo, hi)| lo != 0.0 || hi != 0.0)
}

/// A range given either way round.
fn ordered(a: f32, b: f32) -> (f32, f32) {
    if a <= b { (a, b) } else { (b, a) }
}

/// `value` held within `lo` to `hi`: [`f32::clamp`], without its assertion
/// that `lo` is at most `hi`. The ranges here are in order where they are
/// made, so the answer is the same; and the assertion's message, which
/// formats both bounds, would link float formatting into every build.
pub(super) fn clamp(value: f32, lo: f32, hi: f32) -> f32 {
    let value = if value < lo { lo } else { value };
    if value > hi { hi } else { value }
}

/// A static font's styles, from its attributes.
fn static_styles(style: FontStyle) -> Styles {
    match style {
        FontStyle::Normal => Styles {
            italic: Some((0.0, 0.0)),
            oblique: Some((0.0, 0.0)),
        },
        FontStyle::Italic => Styles {
            italic: Some((1.0, 1.0)),
            oblique: None,
        },
        FontStyle::Oblique(angle) => {
            let angle = angle.unwrap_or(DEFAULT_OBLIQUE);
            Styles {
                italic: None,
                oblique: Some((angle, angle)),
            }
        }
    }
}

/// Whether a range holds `x`.
fn holds(range: Option<(f32, f32)>, x: f32) -> bool {
    range.is_some_and(|(lo, hi)| lo <= x && x <= hi)
}

/// The least value a range reaches at or above `x`.
fn nearest_above(range: Option<(f32, f32)>, x: f32) -> Option<f32> {
    range.and_then(|(lo, hi)| (hi >= x).then(|| lo.max(x)))
}

/// The greatest value a range reaches at or below `x`.
fn nearest_below(range: Option<(f32, f32)>, x: f32) -> Option<f32> {
    range.and_then(|(lo, hi)| (lo <= x).then(|| hi.min(x)))
}

/// The greatest value a range reaches strictly between 0 and `x`: the
/// spec's "below the desired value, until 0 is hit, positive values only".
fn below_to_zero(range: Option<(f32, f32)>, x: f32) -> Option<f32> {
    nearest_below(range, x).filter(|&value| value > 0.0 && value < x)
}

/// The spec's style step, as a rank: a tier per step it lists, and within a
/// tier the order it checks values in. A value checked in descending order
/// ranks by how far it is below where the checking starts: at or below 0,
/// by its negation; below a bound, by the bound less it.
fn style_rank(offered: Styles, style: FontStyle, synthesize_style: bool) -> Rank {
    let last = rank(9, 0.0);
    match style {
        FontStyle::Normal => {
            // Oblique values at or above 0 ascending; then italic values at
            // or above 0 ascending; then oblique, then italic, below 0
            // descending.
            if let Some(value) = nearest_above(offered.oblique, 0.0) {
                rank(0, value)
            } else if let Some(value) = nearest_above(offered.italic, 0.0) {
                rank(1, value)
            } else if let Some(value) = nearest_below(offered.oblique, 0.0) {
                rank(2, -value)
            } else if let Some(value) = nearest_below(offered.italic, 0.0) {
                rank(3, -value)
            } else {
                last
            }
        }
        FontStyle::Italic => {
            // Italic values holding 1; above 1 ascending; below 1 descending
            // to 0. Then oblique at or above 14deg ascending, then below
            // descending to 0. Then italic, then oblique, at or below 0
            // descending.
            if holds(offered.italic, 1.0) {
                EXACT
            } else if let Some(value) = nearest_above(offered.italic, 1.0) {
                rank(1, value)
            } else if let Some(value) = below_to_zero(offered.italic, 1.0) {
                rank(2, 1.0 - value)
            } else if let Some(value) = nearest_above(offered.oblique, THRESHOLD) {
                rank(3, value)
            } else if let Some(value) = below_to_zero(offered.oblique, THRESHOLD) {
                rank(4, THRESHOLD - value)
            } else if let Some(value) = nearest_below(offered.italic, 0.0) {
                rank(5, -value)
            } else if let Some(value) = nearest_below(offered.oblique, 0.0) {
                rank(6, -value)
            } else {
                last
            }
        }
        FontStyle::Oblique(angle) => {
            let angle = angle.unwrap_or(DEFAULT_OBLIQUE);
            // A negative angle follows the same steps "with the negated
            // values and opposite directions": mirror the font and ask for
            // the positive angle.
            let (angle, offered) = if angle < 0.0 {
                let mirror = |range: Option<(f32, f32)>| range.map(|(lo, hi)| (-hi, -lo));
                (
                    -angle,
                    Styles {
                        italic: mirror(offered.italic),
                        oblique: mirror(offered.oblique),
                    },
                )
            } else {
                (angle, offered)
            };
            let steep = angle >= THRESHOLD;
            if holds(offered.oblique, angle) {
                return EXACT;
            }
            // Steep: above ascending, then below descending to 0. Shallow:
            // below descending to 0, then above ascending.
            let above = nearest_above(offered.oblique, angle);
            let below = below_to_zero(offered.oblique, angle).map(|value| angle - value);
            let (first, second) = if steep {
                (above, below)
            } else {
                (below, above)
            };
            if let Some(key) = first {
                return rank(1, key);
            }
            if let Some(key) = second {
                return rank(2, key);
            }
            // An upright font, to shear, where synthesis is allowed and the
            // family has nothing that leans.
            if synthesize_style && holds(offered.oblique, 0.0) {
                return rank(3, 0.0);
            }
            // Italic values, in the same two orders about 1.
            let above = nearest_above(offered.italic, 1.0);
            let below = below_to_zero(offered.italic, 1.0).map(|value| 1.0 - value);
            let (first, second) = if steep {
                (above, below)
            } else {
                (below, above)
            };
            if let Some(key) = first {
                rank(4, key)
            } else if let Some(key) = second {
                rank(5, key)
            } else if let Some(value) = nearest_below(offered.oblique, 0.0) {
                rank(6, -value)
            } else if let Some(value) = nearest_below(offered.italic, 0.0) {
                rank(7, -value)
            } else {
                last
            }
        }
    }
}

/// Adjustments for rendering a matched font.
///
/// Contains variation settings and synthetic bold or slant. Axis values
/// are clamped to the font ranges. The `slnt` axis uses the opposite sign
/// from CSS angles; oblique requests do not use `ital`.
///
/// Weights of 600 or above request synthetic bold if the font cannot
/// reach them. Fonts with a `wght` axis extending above 400 are not
/// emboldened unless face descriptors declare their weights.
///
/// Italic requests use `ital`, then `slnt` if it supports italic, then
/// synthetic skew. The italic target is 14°, regardless of face descriptors.
/// Normal requests reset a slanted default within the declared oblique range.
///
/// Apply synthetic styling as permitted by `font-synthesis`.
/// For `font-synthesis: none`, apply only variation settings.
#[derive(Copy, Clone, Debug)]
pub struct Synthesis {
    vars: [FontVariation; 6],
    len: u8,
    embolden: bool,
    skew: f32,
}

impl Default for Synthesis {
    fn default() -> Self {
        Self {
            vars: [FontVariation::new(Tag::new(b"    "), 0.0); 6],
            len: 0,
            embolden: false,
            skew: 0.0,
        }
    }
}

impl PartialEq for Synthesis {
    fn eq(&self, other: &Self) -> bool {
        self.variation_settings() == other.variation_settings()
            && self.embolden == other.embolden
            && self.skew == other.skew
    }
}

impl Synthesis {
    /// Returns the variation settings to apply.
    ///
    /// Includes matching axes and pinned named-instance coordinates. Values
    /// equal to the font's axis defaults are omitted.
    pub fn variation_settings(&self) -> &[FontVariation] {
        &self.vars[..usize::from(self.len)]
    }

    /// Returns `true` if synthetic bold is needed.
    pub fn embolden(&self) -> bool {
        self.embolden
    }

    /// Returns the synthetic slant angle, if needed.
    ///
    /// The angle is in clockwise degrees, as in CSS.
    pub fn skew(&self) -> Option<f32> {
        (self.skew != 0.0).then_some(self.skew)
    }

    /// Returns `true` if any adjustment is needed.
    pub fn any(&self) -> bool {
        self.len != 0 || self.embolden || self.skew != 0.0
    }

    /// Sets `axis` from a value in CSS's units.
    fn set_css(&mut self, axis: &Axis, value: f32) {
        self.set(axis, value / super::attributes::css_scale(axis));
    }

    /// Sets `axis` from a value in its own.
    fn set(&mut self, axis: &Axis, value: f32) {
        let value = clamp(value, axis.min, axis.max);
        if value == axis.default {
            return;
        }
        if let Some(held) = self.vars[..usize::from(self.len)]
            .iter_mut()
            .find(|held| held.tag == axis.tag)
        {
            held.value = value;
        } else if usize::from(self.len) < self.vars.len() {
            self.vars[usize::from(self.len)] = FontVariation::new(axis.tag, value);
            self.len += 1;
        }
    }
}

/// What `font` needs to look like `request`.
pub(super) fn synthesis(font: &Font, request: Attributes) -> Synthesis {
    let mut synthesis = Synthesis::default();
    // How far its axes may go: their own ranges, within what a face
    // declares.
    let ranges = Ranges::from_font(font);
    let within = |value: f32, (lo, hi): (f32, f32)| clamp(value, lo, hi);
    if let Some(wdth) = font.axis(b"wdth") {
        synthesis.set_css(wdth, within(request.width.ratio() * 100.0, ranges.width));
    }

    // An axis the font is held at — a named instance the platform matched by
    // name — is set wherever CSS does not drive it, since its default is what
    // a shaper draws otherwise. `opsz` for "Segoe UI Variable Display".
    for axis in font.axes() {
        if axis.min == axis.max {
            synthesis.set(axis, axis.min);
        }
    }

    let wanted = request.weight.value();
    let reached = within(wanted, ranges.weight);
    if let Some(wght) = font.axis(b"wght") {
        synthesis.set_css(wght, reached);
    }
    // Bold asked for, and the font cannot get there. A face declared bold is
    // bold, whatever its file says. A `wght` axis reaching past 400 is never
    // emboldened unless a face declares its weight, as in Blink's
    // `FontCustomPlatformData`.
    let varies_to_bold = font
        .axis(b"wght")
        .is_some_and(|wght| wght.max * super::attributes::css_scale(wght) > 400.0);
    let declared = font
        .face
        .as_ref()
        .is_some_and(|face| face.descriptors.weight.is_some());
    synthesis.embolden = wanted >= 600.0 && reached < 600.0 && (declared || !varies_to_bold);

    let italics = ranges.styles.italic.filter(|&(_, hi)| hi > 0.0);
    let obliques = ranges.styles.oblique;
    let leans = |font: &Font| font.style() != FontStyle::Normal;
    match request.style {
        FontStyle::Normal => {
            // A variable font whose default leans can be stood up.
            if let Some(ital) = font.axis(b"ital")
                && ital.default > 0.0
            {
                synthesis.set(
                    ital,
                    within(0.0, ranges.styles.italic.unwrap_or((0.0, 0.0))),
                );
            }
            // `slnt` goes to the nearest the font reaches to upright: its
            // default where that leans, or a declared range's nearest end,
            // clamping the request into a face's slope range.
            if let Some(slnt) = font.axis(b"slnt") {
                synthesis.set(slnt, -within(0.0, obliques.unwrap_or((0.0, 0.0))));
            }
        }
        // An axis that reaches italic is driven whatever the font declares,
        // so a face declared `oblique 0deg 15deg` with only a `slnt` axis
        // leans to 14deg, as Blink's `FontCustomPlatformData` sets it. Only
        // a font that cannot lean otherwise is sheared.
        FontStyle::Italic => match (font.axis(b"ital"), font.axis(b"slnt"), obliques) {
            (Some(ital), _, _) if italics.is_some() => {
                synthesis.set(ital, within(1.0, italics.unwrap_or((0.0, 0.0))));
            }
            (_, Some(slnt), Some(range)) if range.1 > 0.0 => {
                synthesis.set(slnt, -within(DEFAULT_OBLIQUE, range));
            }
            _ if !leans(font) => synthesis.skew = DEFAULT_OBLIQUE,
            // Asked for italic, and it already leans.
            _ => {}
        },
        FontStyle::Oblique(angle) if !leans(font) => {
            // The `ital` axis is not used for an oblique request.
            let angle = angle.unwrap_or(DEFAULT_OBLIQUE);
            let leans_that_way =
                |(lo, hi): (f32, f32)| if angle < 0.0 { lo < 0.0 } else { hi > 0.0 };
            match (font.axis(b"slnt"), obliques) {
                (Some(slnt), Some(range)) if leans_that_way(range) => {
                    synthesis.set(slnt, -within(angle, range));
                }
                _ => synthesis.skew = angle,
            }
        }
        FontStyle::Oblique(angle) => {
            // A leaning font asked for another angle it reaches.
            if let Some(slnt) = font.axis(b"slnt")
                && let Some(range) = obliques
            {
                synthesis.set(slnt, -within(angle.unwrap_or(DEFAULT_OBLIQUE), range));
            }
        }
    }
    synthesis
}

#[cfg(test)]
mod tests {
    use alloc::vec::Vec;

    use parlance::{FontStyle, FontWeight, FontWidth};

    use super::*;

    /// The font best matching `request`: the first CSS tries of those
    /// tied, as `Family::match_index` has it.
    fn best(fonts: &[Font], request: Attributes, synthesize_style: bool) -> Option<usize> {
        matching(fonts, request, synthesize_style, false).next()
    }
    use crate::test_fonts::{font_with_tables, fvar, os2, post};

    const NORMAL: FontStyle = FontStyle::Normal;
    const ITALIC: FontStyle = FontStyle::Italic;

    const fn oblique(angle: f32) -> FontStyle {
        FontStyle::Oblique(Some(angle))
    }

    /// A static font: a width class (5 is normal), a style and a weight.
    fn font(width: u16, style: FontStyle, weight: u16) -> Font {
        variable(width, style, weight, &[])
    }

    /// The same, with axes as `(tag, min, default, max)`.
    fn variable(
        width: u16,
        style: FontStyle,
        weight: u16,
        axes: &[([u8; 4], f32, f32, f32)],
    ) -> Font {
        let (selection, angle) = match style {
            FontStyle::Normal => (0, 0.0),
            FontStyle::Italic => (0x0001, 0.0),
            // Counter-clockwise in the font.
            FontStyle::Oblique(angle) => (0x0200, -angle.unwrap_or(0.0)),
        };
        let mut tables = Vec::from([
            (*b"OS/2", os2(weight, width, selection)),
            (*b"post", post(angle)),
        ]);
        if !axes.is_empty() {
            tables.push((*b"fvar", fvar(axes)));
        }
        Font::from_data(font_with_tables(&tables), 0)
    }

    fn request(width: f32, style: FontStyle, weight: f32) -> Attributes {
        Attributes {
            width: FontWidth::from_ratio(width / 100.0),
            style,
            weight: FontWeight::new(weight),
        }
    }

    /// The weight chosen from static fonts of these weights.
    fn weights(fonts: &[u16], weight: f32) -> u16 {
        let fonts: Vec<Font> = fonts
            .iter()
            .map(|&weight| font(5, NORMAL, weight))
            .collect();
        let at = best(&fonts, request(100.0, NORMAL, weight), true).unwrap();
        fonts[at].weight().value() as u16
    }

    /// The style chosen from static fonts of these styles.
    fn chosen_style(fonts: &[FontStyle], style: FontStyle, synthesize: bool) -> FontStyle {
        let fonts: Vec<Font> = fonts.iter().map(|&style| font(5, style, 400)).collect();
        let at = best(&fonts, request(100.0, style, 400.0), synthesize).unwrap();
        fonts[at].style()
    }

    fn settings(synthesis: &Synthesis) -> Vec<([u8; 4], f32)> {
        synthesis
            .variation_settings()
            .iter()
            .map(|setting| (setting.tag.to_bytes(), setting.value))
            .collect()
    }

    #[test]
    fn an_empty_family_matches_nothing() {
        assert_eq!(best(&[], Attributes::default(), true), None);
    }

    #[test]
    fn a_request_that_is_not_a_number_still_matches() {
        let fonts: Vec<Font> = [400, 700].map(|weight| font(5, NORMAL, weight)).into();
        for request in [
            request(f32::NAN, NORMAL, 400.0),
            request(100.0, NORMAL, f32::NAN),
            request(100.0, oblique(f32::NAN), 400.0),
        ] {
            for synthesize in [false, true] {
                assert!(best(&fonts, request, synthesize).is_some());
                assert!(
                    matching(&fonts, request, synthesize, false)
                        .next()
                        .is_some()
                );
                let _ = synthesis(&fonts[0], request);
            }
        }
    }

    #[test]
    fn weights_follow_the_spec() {
        // 400 to 500: heavier up to 500, then lighter, then heavier.
        assert_eq!(weights(&[300, 500, 700], 400.0), 500);
        assert_eq!(weights(&[300, 600], 400.0), 300);
        assert_eq!(weights(&[400, 600], 450.0), 400);
        assert_eq!(weights(&[600, 700], 450.0), 600);
        // Below 400: lighter, nearest first, then heavier.
        assert_eq!(weights(&[200, 100, 400], 300.0), 200);
        assert_eq!(weights(&[400, 500], 300.0), 400);
        // Above 500: heavier, nearest first, then lighter.
        assert_eq!(weights(&[400, 900, 800], 700.0), 800);
        assert_eq!(weights(&[400, 500], 700.0), 500);
    }

    #[test]
    fn widths_follow_the_spec() {
        let fonts = [
            font(4, NORMAL, 400),
            font(6, NORMAL, 400),
            font(8, NORMAL, 400),
        ];
        let width = |percent: f32| {
            let at = best(&fonts, request(percent, NORMAL, 400.0), true).unwrap();
            fonts[at].width().ratio() * 100.0
        };
        // Normal and narrower look narrower first; wider, wider first.
        assert_eq!(width(100.0), 87.5);
        assert_eq!(width(75.0), 87.5);
        assert_eq!(width(125.0), 150.0);
        assert_eq!(width(200.0), 150.0);
    }

    #[test]
    fn width_comes_before_style_and_weight() {
        let fonts = [font(4, NORMAL, 400), font(5, ITALIC, 700)];
        assert_eq!(best(&fonts, request(100.0, NORMAL, 400.0), true), Some(1));
    }

    #[test]
    fn normal_prefers_oblique_to_italic() {
        // fontique takes the italic: it ranks both on one scale.
        assert_eq!(
            chosen_style(&[ITALIC, oblique(10.0)], NORMAL, true),
            oblique(10.0)
        );
        assert_eq!(
            chosen_style(&[ITALIC, oblique(10.0), NORMAL], NORMAL, true),
            NORMAL
        );
    }

    #[test]
    fn italic_looks_at_obliques_from_14_degrees() {
        // At or past the threshold, ascending: 20 before 12, as in Blink's `FontSelectionAlgorithm`.
        assert_eq!(
            chosen_style(&[oblique(12.0), oblique(20.0), NORMAL], ITALIC, true),
            oblique(20.0)
        );
        assert_eq!(
            chosen_style(&[oblique(20.0), oblique(15.0), NORMAL], ITALIC, true),
            oblique(15.0)
        );
        // Then below it, descending.
        assert_eq!(
            chosen_style(&[oblique(4.0), oblique(8.0), NORMAL], ITALIC, true),
            oblique(8.0)
        );
        // Upright last.
        assert_eq!(chosen_style(&[NORMAL], ITALIC, true), NORMAL);
        assert_eq!(chosen_style(&[oblique(20.0), ITALIC], ITALIC, true), ITALIC);
    }

    #[test]
    fn italic_ties_obliques_holding_14_degrees_and_weight_decides() {
        // `oblique-last-resort-weight-selection`: both ranges hold 14deg, so
        // the weight asked for picks the first.
        let face = |lo: f32, hi: f32, weight: f32| {
            let descriptors = super::super::FaceDescriptors {
                style: Some(FaceStyle::Oblique(lo, hi)),
                weight: Some((FontWeight::new(weight), FontWeight::new(weight))),
                ..Default::default()
            };
            font(5, NORMAL, 400).with_face(super::super::face::Face::new(
                super::super::FaceId(0),
                descriptors,
            ))
        };
        let fonts = [face(14.0, 30.0, 400.0), face(5.0, 14.0, 700.0)];
        assert_eq!(best(&fonts, request(100.0, ITALIC, 400.0), false), Some(0));
        assert_eq!(best(&fonts, request(100.0, ITALIC, 700.0), false), Some(1));
    }

    #[test]
    fn obliques_follow_the_spec() {
        // Steep: steeper first, then shallower.
        assert_eq!(
            chosen_style(&[oblique(12.0), oblique(25.0)], oblique(20.0), true),
            oblique(25.0)
        );
        // Shallow: shallower first, then steeper.
        assert_eq!(
            chosen_style(&[oblique(3.0), oblique(8.0)], oblique(5.0), true),
            oblique(3.0)
        );
        assert_eq!(
            chosen_style(&[oblique(8.0), NORMAL], oblique(5.0), true),
            oblique(8.0)
        );
        // Negative angles mirror.
        assert_eq!(
            chosen_style(&[oblique(-5.0), oblique(-15.0)], oblique(-10.0), true),
            oblique(-5.0)
        );
        assert_eq!(
            chosen_style(&[oblique(5.0), oblique(-15.0)], oblique(-10.0), true),
            oblique(-15.0)
        );
    }

    #[test]
    fn an_oblique_request_shears_an_upright_font_only_in_a_family_that_cannot_lean() {
        // A family with an italic takes it: CSS makes artificial obliques
        // only for families that lack any italic or oblique font.
        assert_eq!(chosen_style(&[NORMAL, ITALIC], oblique(14.0), true), ITALIC);
        assert_eq!(
            chosen_style(&[ITALIC, NORMAL], oblique(14.0), false),
            ITALIC
        );
        // One with nothing that leans: the upright font, to shear.
        let fonts = [font(5, NORMAL, 700), font(5, NORMAL, 400)];
        let at = best(&fonts, request(100.0, oblique(14.0), 400.0), true).unwrap();
        assert_eq!(fonts[at].weight(), FontWeight::new(400.0));
    }

    #[test]
    fn a_variable_font_matches_anything_in_its_range() {
        let fonts = [
            variable(5, NORMAL, 400, &[(*b"wght", 100.0, 400.0, 600.0)]),
            font(5, NORMAL, 700),
        ];
        let weight = |weight: f32| best(&fonts, request(100.0, NORMAL, weight), true);
        assert_eq!(weight(550.0), Some(0));
        assert_eq!(weight(100.0), Some(0));
        // Heavier than 500 looks heavier first: the static 700, not the
        // axis's 600.
        assert_eq!(weight(650.0), Some(1));
        // Nothing reaches 900, so the nearest below it.
        assert_eq!(weight(900.0), Some(1));

        let fonts = [
            font(5, NORMAL, 400),
            variable(4, NORMAL, 400, &[(*b"wdth", 50.0, 87.5, 150.0)]),
        ];
        assert_eq!(best(&fonts, request(62.5, NORMAL, 400.0), true), Some(1));
        assert_eq!(best(&fonts, request(100.0, NORMAL, 400.0), true), Some(0));
    }

    #[test]
    fn a_slant_axis_offers_obliques_and_an_italic_axis_italics() {
        let fonts = [
            font(5, ITALIC, 400),
            variable(5, NORMAL, 400, &[(*b"slnt", -15.0, 0.0, 0.0)]),
        ];
        let at = |style| best(&fonts, request(100.0, style, 400.0), false);
        assert_eq!(at(oblique(10.0)), Some(1));
        assert_eq!(at(ITALIC), Some(0));
        assert_eq!(at(NORMAL), Some(1));

        // `ital` is not used for an oblique request, nor an oblique for an
        // italic one while there is an italic value to be had.
        let fonts = [
            variable(5, NORMAL, 400, &[(*b"ital", 0.0, 0.0, 1.0)]),
            font(5, oblique(10.0), 400),
        ];
        let at = |style| best(&fonts, request(100.0, style, 400.0), true);
        assert_eq!(at(ITALIC), Some(0));
        assert_eq!(at(oblique(10.0)), Some(1));
        assert_eq!(at(oblique(20.0)), Some(1));
    }

    #[test]
    fn slnt_is_set_counter_clockwise() {
        let font = variable(5, NORMAL, 400, &[(*b"slnt", -15.0, 0.0, 0.0)]);
        let synthesis = font.synthesis(request(100.0, oblique(10.0), 400.0));
        assert_eq!(settings(&synthesis), [(*b"slnt", -10.0)]);
        assert_eq!(synthesis.skew(), None);
        // Past the axis, clamped.
        let synthesis = font.synthesis(request(100.0, oblique(20.0), 400.0));
        assert_eq!(settings(&synthesis), [(*b"slnt", -15.0)]);
        // Italic, where there is no `ital`, leans by the default angle.
        let synthesis = font.synthesis(request(100.0, ITALIC, 400.0));
        assert_eq!(settings(&synthesis), [(*b"slnt", -14.0)]);
    }

    #[test]
    fn italic_sets_slnt_in_a_face_declared_oblique() {
        // `slnt-variable`: a face declared `oblique 0deg 15deg` with only a
        // `slnt` axis leans to 14deg for italic, as Blink sets it, and
        // stays upright for normal.
        let face = |lo: f32, hi: f32, axis: ([u8; 4], f32, f32, f32)| {
            let descriptors = super::super::FaceDescriptors {
                style: Some(FaceStyle::Oblique(lo, hi)),
                ..Default::default()
            };
            variable(5, NORMAL, 400, &[axis]).with_face(super::super::face::Face::new(
                super::super::FaceId(0),
                descriptors,
            ))
        };
        let font = face(0.0, 15.0, (*b"slnt", -15.0, 0.0, 0.0));
        let synthesis = font.synthesis(request(100.0, ITALIC, 400.0));
        assert_eq!(settings(&synthesis), [(*b"slnt", -14.0)]);
        assert_eq!(synthesis.skew(), None);
        assert!(!font.synthesis(request(100.0, NORMAL, 400.0)).any());
        // `font-slant-1`: the face's 10deg is as far as italic goes.
        let font = face(0.0, 10.0, (*b"slnt", -10.0, 0.0, 0.0));
        let synthesis = font.synthesis(request(100.0, ITALIC, 400.0));
        assert_eq!(settings(&synthesis), [(*b"slnt", -10.0)]);
        // `slnt-backslant-variable`: both ways, italic leans forward.
        let font = face(-15.0, 15.0, (*b"slnt", -15.0, 0.0, 15.0));
        let synthesis = font.synthesis(request(100.0, ITALIC, 400.0));
        assert_eq!(settings(&synthesis), [(*b"slnt", -14.0)]);
        let synthesis = font.synthesis(request(100.0, oblique(-14.0), 400.0));
        assert_eq!(settings(&synthesis), [(*b"slnt", 14.0)]);
    }

    #[test]
    fn normal_sets_slnt_to_the_nearest_end_of_a_declared_oblique_range() {
        // `stretch-distance-over-weight-distance`: a face declared `oblique
        // 45deg 67.5deg` asked for normal leans 45deg, as Blink clamps the
        // request's slope into the face's range; one declared `oblique
        // -20deg` leans back 20deg.
        let face = |lo: f32, hi: f32| {
            let descriptors = super::super::FaceDescriptors {
                style: Some(FaceStyle::Oblique(lo, hi)),
                ..Default::default()
            };
            variable(5, NORMAL, 400, &[(*b"slnt", -90.0, 0.0, 90.0)]).with_face(
                super::super::face::Face::new(super::super::FaceId(0), descriptors),
            )
        };
        let slnt = |font: &Font, style| settings(&font.synthesis(request(100.0, style, 400.0)));
        assert_eq!(slnt(&face(45.0, 67.5), NORMAL), [(*b"slnt", -45.0)]);
        assert_eq!(slnt(&face(-20.0, -20.0), NORMAL), [(*b"slnt", 20.0)]);
        assert_eq!(slnt(&face(-15.0, 15.0), NORMAL), []);
        // Oblique requests clamp the same way.
        assert_eq!(slnt(&face(45.0, 67.5), oblique(-90.0)), [(*b"slnt", -45.0)]);
        assert_eq!(slnt(&face(45.0, 67.5), oblique(90.0)), [(*b"slnt", -67.5)]);
    }

    #[test]
    fn ital_is_used_for_italic_and_not_for_oblique() {
        let font = variable(5, NORMAL, 400, &[(*b"ital", 0.0, 0.0, 1.0)]);
        let synthesis = font.synthesis(request(100.0, ITALIC, 400.0));
        assert_eq!(settings(&synthesis), [(*b"ital", 1.0)]);
        assert_eq!(synthesis.skew(), None);
        let synthesis = font.synthesis(request(100.0, oblique(10.0), 400.0));
        assert!(synthesis.variation_settings().is_empty());
        assert_eq!(synthesis.skew(), Some(10.0));
    }

    #[test]
    fn a_static_upright_font_leans_for_italic_and_oblique() {
        let upright = font(5, NORMAL, 400);
        let skew = |style| upright.synthesis(request(100.0, style, 400.0)).skew();
        assert_eq!(skew(ITALIC), Some(14.0));
        assert_eq!(skew(FontStyle::Oblique(None)), Some(14.0));
        assert_eq!(skew(oblique(-8.0)), Some(-8.0));
        assert_eq!(skew(NORMAL), None);
        // Nothing to do for a font that already leans.
        let italic = font(5, ITALIC, 400);
        assert!(!italic.synthesis(request(100.0, ITALIC, 400.0)).any());
    }

    #[test]
    fn faux_bold_is_for_bold_the_font_cannot_reach() {
        let regular = font(5, NORMAL, 400);
        let embolden = |weight| regular.synthesis(request(100.0, NORMAL, weight)).embolden();
        assert!(embolden(700.0));
        assert!(embolden(600.0));
        // fontique emboldens here: 500 is heavier than 400, but not bold.
        assert!(!embolden(500.0));
        let semibold = font(5, NORMAL, 600);
        assert!(!semibold.synthesis(request(100.0, NORMAL, 900.0)).embolden());
    }

    #[test]
    fn weight_and_width_axes_are_set_and_clamped() {
        let font = variable(
            5,
            NORMAL,
            400,
            &[
                (*b"wght", 100.0, 400.0, 500.0),
                (*b"wdth", 75.0, 100.0, 100.0),
            ],
        );
        let synthesis = font.synthesis(request(62.5, NORMAL, 700.0));
        assert_eq!(settings(&synthesis), [(*b"wdth", 75.0), (*b"wght", 500.0)]);
        // Values equal to the default are left out.
        assert!(!font.synthesis(request(100.0, NORMAL, 400.0)).any());
    }

    #[test]
    fn a_weight_axis_reaching_past_normal_is_not_emboldened() {
        // `synthetic-bold-out-of-capabilities-range`: the axis stops at 500,
        // and Chrome draws 900 at 500 without faking bold.
        let font = variable(5, NORMAL, 400, &[(*b"wght", 400.0, 400.0, 500.0)]);
        let synthesis = font.synthesis(request(100.0, NORMAL, 900.0));
        assert_eq!(settings(&synthesis), [(*b"wght", 500.0)]);
        assert!(!synthesis.embolden());

        // An axis that goes no further than normal is emboldened.
        let light = variable(5, NORMAL, 400, &[(*b"wght", 100.0, 400.0, 400.0)]);
        assert!(light.synthesis(request(100.0, NORMAL, 900.0)).embolden());

        // So is a face that declares weights short of bold.
        let descriptors = super::super::FaceDescriptors {
            weight: Some((FontWeight::new(400.0), FontWeight::new(500.0))),
            ..Default::default()
        };
        let face = font.with_face(super::super::face::Face::new(
            super::super::FaceId(0),
            descriptors,
        ));
        assert!(face.synthesis(request(100.0, NORMAL, 900.0)).embolden());
    }

    #[test]
    fn a_variable_font_leaning_by_default_is_stood_up_for_normal() {
        let font = variable(5, ITALIC, 400, &[(*b"ital", 0.0, 1.0, 1.0)]);
        let synthesis = font.synthesis(request(100.0, NORMAL, 400.0));
        assert_eq!(settings(&synthesis), [(*b"ital", 0.0)]);
    }

    #[test]
    fn a_gx_weight_axis_is_asked_for_in_its_own_units() {
        // Apple's pre-OpenType fonts state `wght` as a multiplier of normal,
        // not as a weight class: Skia's runs 0.48 to 3.2 about a default of
        // 1. Read as CSS weights, every request from 400 up clamps to the
        // top of that range and the font draws at its heaviest.
        let font = variable(
            5,
            FontStyle::Normal,
            400,
            &[(*b"wght", 0.48, 1.0, 3.2), (*b"wdth", 0.62, 1.0, 1.3)],
        );
        let at = |weight: f32| {
            let synthesis = synthesis(&font, request(100.0, FontStyle::Normal, weight));
            settings(&synthesis)
                .iter()
                .find(|(tag, _)| tag == b"wght")
                .map(|(_, value)| *value)
        };
        // The axis values a font states are 16.16 fixed point, so they come
        // back as near as that gets.
        let close =
            |left: Option<f32>, right: f32| left.is_some_and(|left| (left - right).abs() < 0.001);
        // Normal asks for the default, which is nothing to apply.
        assert_eq!(at(400.0), None, "400");
        // Bold is 1.75 of normal, not 3.2.
        assert!(close(at(700.0), 1.75), "700: {:?}", at(700.0));
        assert!(close(at(900.0), 2.25), "900: {:?}", at(900.0));
        assert!(close(at(300.0), 0.75), "300: {:?}", at(300.0));
        // Past what the axis reaches, it stops there.
        assert!(close(at(2000.0), 3.2), "2000: {:?}", at(2000.0));
        // A width request lands in the axis's units too: CSS's 75% of a
        // GX `wdth` is 0.75, which this font reaches.
        let synthesis = synthesis(
            &font,
            Attributes {
                width: FontWidth::from_percentage(75.0),
                ..request(100.0, FontStyle::Normal, 400.0)
            },
        );
        let wdth = settings(&synthesis)
            .iter()
            .find(|(tag, _)| tag == b"wdth")
            .map(|(_, value)| *value);
        assert!(close(wdth, 0.75), "width: {wdth:?}");
    }
}
