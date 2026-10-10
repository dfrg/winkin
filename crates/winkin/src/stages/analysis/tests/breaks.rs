//! Line-break opportunity tests. They pin:
//! - the opportunities of UAX #14, and of Blink's ASCII table where Chrome uses
//!   it;
//! - style seams, item boundaries and box edges, which move no break;
//! - `white-space`, `word-break`, `line-break`, `hyphens` and
//!   `overflow-wrap`, each with the answer Chrome or CSS gives;
//! - small kana, the East Asian gate and the Latin-1 fast path;
//! - what hangs.

use super::*;

#[test]
fn opportunities_follow_uax_14() {
    let layout = plain("the quick (brown) fox, 1,000 well-known");
    let text = &layout.content().text;
    let words: Vec<_> = breaks(&layout).iter().map(|&at| &text[..at]).collect();
    assert_eq!(
        words,
        [
            "the ",
            "the quick ",
            "the quick (brown) ",
            "the quick (brown) fox, ",
            "the quick (brown) fox, 1,000 ",
            "the quick (brown) fox, 1,000 well-",
        ]
    );
}

/// Blink's pair table as `LineBreakData::FillAscii` writes it
/// (`character_property_data_generator.cc`), call for call and in its
/// order, over U+0021 to U+007F: whether a line may break between the two,
/// `[before][after]`.
fn blinks_ascii_table() -> [[bool; 95]; 95] {
    const FIRST: u8 = b'!';
    const LAST: u8 = 0x7F;
    let mut table = [[false; 95]; 95];
    let mut set = |before: (u8, u8), after: (u8, u8), value: bool| {
        for x in before.0..=before.1 {
            for y in after.0..=after.1 {
                table[usize::from(x - FIRST)][usize::from(y - FIRST)] = value;
            }
        }
    };
    let all = (FIRST, LAST);
    let one = |ch: u8| (ch, ch);
    set(all, all, false);
    set(all, one(b'('), true);
    set(all, one(b'<'), true);
    set(all, one(b'['), true);
    set(all, one(b'{'), true);
    set(one(b'-'), all, true);
    set(one(b'?'), all, true);
    set(one(b'-'), one(b'$'), false);
    set(all, one(b'!'), false);
    set(one(b'?'), one(b'"'), false);
    set(one(b'?'), one(b'\''), false);
    set(all, one(b')'), false);
    set(all, one(b','), false);
    set(all, one(b'.'), false);
    set(all, one(b'/'), false);
    // Between `-` and a digit is hard-coded in `ShouldBreakFast()`.
    set(one(b'-'), (b'0', b'9'), false);
    set(all, one(b':'), false);
    set(all, one(b';'), false);
    set(all, one(b'?'), false);
    set(all, one(b']'), false);
    set(all, one(b'}'), false);
    set(one(b'$'), all, false);
    set(one(b'\''), all, false);
    set(one(b'('), all, false);
    set(one(b'/'), all, false);
    set((b'0', b'9'), all, false);
    set(one(b'<'), all, false);
    set(one(b'@'), all, false);
    set((b'A', b'Z'), all, false);
    set(one(b'['), all, false);
    set((b'^', b'`'), all, false);
    set((b'a', b'z'), all, false);
    set(one(b'{'), all, false);
    set(one(LAST), all, false);
    table
}

/// Chrome's ASCII rules (`ascii`) are Blink's, pair for pair:
/// - every pair of U+0021 to U+007F breaks as Blink's table has it. Chrome 153
///   breaks all 8,836 printable pairs this way, measured with each pair alone
///   in a column of no width, in 10 px Ahem;
/// - a hyphen before a digit is checked after every ASCII character, and
///   breaks only after an ASCII letter or digit (`ShouldBreakFast`);
/// - a line breaks after U+0020, a tab or a line feed before anything, and
///   never before one (`kAfterSpaceRun`);
/// - ICU decides everything else: a pair with a character past ASCII or a
///   control, and a hyphen before a character past ASCII.
#[test]
fn ascii_pairs_break_as_blinks_table_has_them() {
    let table = blinks_ascii_table();
    let printable = || (b'!'..=0x7F).map(char::from);
    let index = |ch: char| usize::from(u8::try_from(ch).unwrap_or(b'!') - b'!');
    for before in printable() {
        for after in printable() {
            let blink = table[index(before)][index(after)];
            let rule = super::ascii::breaks(|| None, before, after);
            if before == '-' && after.is_ascii_digit() {
                assert_eq!(rule, Some(false), "{before:?} {after:?} at the start");
                for context in (0..=0x7F_u8).map(char::from) {
                    assert_eq!(
                        super::ascii::breaks(|| Some(context), before, after),
                        Some(context.is_ascii_alphanumeric()),
                        "{context:?}{before:?}{after:?}"
                    );
                }
            } else {
                assert_eq!(rule, Some(blink), "{before:?} {after:?}");
            }
        }
    }
    let spaces = [' ', '\t', '\n'];
    let others: Vec<char> = printable()
        .chain([
            '\u{A0}', '\u{E9}', '\u{BB}', '\u{3001}', '\u{2060}', '\u{FFFC}', '\u{1}',
        ])
        .collect();
    for space in spaces {
        for &other in &others {
            assert_eq!(super::ascii::breaks(|| None, space, other), Some(true));
            assert_eq!(super::ascii::breaks(|| None, other, space), Some(false));
        }
        for other in spaces {
            assert_eq!(super::ascii::breaks(|| None, space, other), Some(false));
        }
    }
    let past = [
        '\u{80}',
        '\u{A0}',
        '\u{AB}',
        '\u{E9}',
        '\u{FF}',
        '\u{100}',
        '\u{3001}',
        '\u{1F600}',
    ];
    for past in past {
        for ascii in printable() {
            assert_eq!(
                super::ascii::breaks(|| Some('a'), past, ascii),
                None,
                "{past:?}{ascii:?}"
            );
            assert_eq!(
                super::ascii::breaks(|| Some('a'), ascii, past),
                None,
                "{ascii:?}{past:?}"
            );
        }
    }
    for control in ['\u{0}', '\u{1}', '\u{B}', '\u{D}', '\u{1F}'] {
        assert_eq!(super::ascii::breaks(|| None, control, 'a'), None);
        assert_eq!(super::ascii::breaks(|| None, 'a', control), None);
    }
}

/// Where Chrome 155 breaks each pair of U+0021 to U+007E under `word-break:
/// break-all`, measured with each pair alone in a column of no width, in
/// 10 px Ahem: one row per character before, bit `n` set where it breaks
/// before U+0021 + `n`.
const CHROME_BREAK_ALL: [u128; 94] = [
    0x2fffffffefffffffb9ff96bc,
    0x040000000400000008000080,
    0x2fffffffefffffffb9ff96ac,
    0x000000000000000000000010,
    0x2fffffffefffffffb9ff96bc,
    0x2fffffffefffffffb9ff96ac,
    0x000000000000000000000000,
    0x000000000000000000000000,
    0x2fffffffefffffffb9ff96ac,
    0x2fffffffefffffffb9ff96ac,
    0x2fffffffefffffffb9ff96ac,
    0x2fffffffe7ffffffb9ff96a4,
    0x2fffffffefffffffb9ff96f6,
    0x2fffffffe7ffffffb9ff96a4,
    0x2fffffffefffffffb9ff96ac,
    0x2fffffffefffffffb9ff96ac,
    0x2fffffffefffffffb9ff96ac,
    0x2fffffffefffffffb9ff96ac,
    0x2fffffffefffffffb9ff96ac,
    0x2fffffffefffffffb9ff96ac,
    0x2fffffffefffffffb9ff96ac,
    0x2fffffffefffffffb9ff96ac,
    0x2fffffffefffffffb9ff96ac,
    0x2fffffffefffffffb9ff96ac,
    0x2fffffffefffffffb9ff96ac,
    0x2fffffffe7ffffffb9ff96a4,
    0x2fffffffe7ffffffb9ff96a4,
    0x2fffffffefffffffb9ff96ac,
    0x2fffffffefffffffb9ff96ac,
    0x2fffffffefffffffb9ff96ac,
    0x2fffffffefffffffb9ff96bc,
    0x2fffffffefffffffb9ff96ac,
    0x2fffffffefffffffb9ff96ac,
    0x2fffffffefffffffb9ff96ac,
    0x2fffffffefffffffb9ff96ac,
    0x2fffffffefffffffb9ff96ac,
    0x2fffffffefffffffb9ff96ac,
    0x2fffffffefffffffb9ff96ac,
    0x2fffffffefffffffb9ff96ac,
    0x2fffffffefffffffb9ff96ac,
    0x2fffffffefffffffb9ff96ac,
    0x2fffffffefffffffb9ff96ac,
    0x2fffffffefffffffb9ff96ac,
    0x2fffffffefffffffb9ff96ac,
    0x2fffffffefffffffb9ff96ac,
    0x2fffffffefffffffb9ff96ac,
    0x2fffffffefffffffb9ff96ac,
    0x2fffffffefffffffb9ff96ac,
    0x2fffffffefffffffb9ff96ac,
    0x2fffffffefffffffb9ff96ac,
    0x2fffffffefffffffb9ff96ac,
    0x2fffffffefffffffb9ff96ac,
    0x2fffffffefffffffb9ff96ac,
    0x2fffffffefffffffb9ff96ac,
    0x2fffffffefffffffb9ff96ac,
    0x2fffffffefffffffb9ff96ac,
    0x2fffffffefffffffb9ff96ac,
    0x2fffffffefffffffb9ff96ac,
    0x000000000000000000000000,
    0x040000000400000008000090,
    0x2fffffffefffffffb9ff96ac,
    0x2fffffffefffffffb9ff96ac,
    0x2fffffffefffffffb9ff96ac,
    0x2fffffffefffffffb9ff96ac,
    0x2fffffffefffffffb9ff96ac,
    0x2fffffffefffffffb9ff96ac,
    0x2fffffffefffffffb9ff96ac,
    0x2fffffffefffffffb9ff96ac,
    0x2fffffffefffffffb9ff96ac,
    0x2fffffffefffffffb9ff96ac,
    0x2fffffffefffffffb9ff96ac,
    0x2fffffffefffffffb9ff96ac,
    0x2fffffffefffffffb9ff96ac,
    0x2fffffffefffffffb9ff96ac,
    0x2fffffffefffffffb9ff96ac,
    0x2fffffffefffffffb9ff96ac,
    0x2fffffffefffffffb9ff96ac,
    0x2fffffffefffffffb9ff96ac,
    0x2fffffffefffffffb9ff96ac,
    0x2fffffffefffffffb9ff96ac,
    0x2fffffffefffffffb9ff96ac,
    0x2fffffffefffffffb9ff96ac,
    0x2fffffffefffffffb9ff96ac,
    0x2fffffffefffffffb9ff96ac,
    0x2fffffffefffffffb9ff96ac,
    0x2fffffffefffffffb9ff96ac,
    0x2fffffffefffffffb9ff96ac,
    0x2fffffffefffffffb9ff96ac,
    0x2fffffffefffffffb9ff96ac,
    0x2fffffffefffffffb9ff96ac,
    0x000000000000000000000000,
    0x2fffffffefffffffb9ff96ac,
    0x2fffffffefffffffb9ff96ac,
    0x2fffffffefffffffb9ff96ac,
];

/// Under `break-all`, Chrome's ASCII rules decide every printable pair, as
/// Blink's break-all table adds to its pair table.
///
/// Each of the 8,836 pairs breaks as Chrome 155 breaks it. A hyphen before a
/// digit breaks after anything. Pairs with a space or a character past ASCII
/// are answered as under `normal`.
#[test]
fn ascii_pairs_break_under_break_all_as_chrome_breaks_them() {
    let printable = || (b'!'..=b'~').map(char::from);
    for (row, before) in CHROME_BREAK_ALL.iter().zip(printable()) {
        for (bit, after) in printable().enumerate() {
            let chrome = row >> bit & 1 == 1;
            assert_eq!(
                super::ascii::breaks_all(|| None, before, after),
                Some(chrome),
                "{before:?} {after:?}"
            );
        }
    }
    assert_eq!(super::ascii::breaks_all(|| Some(' '), '-', '1'), Some(true));
    for (before, after) in [(' ', 'a'), ('a', ' '), ('a', '\u{E9}'), ('\u{E9}', 'a')] {
        assert_eq!(
            super::ascii::breaks_all(|| None, before, after),
            super::ascii::breaks(|| None, before, after),
            "{before:?} {after:?}"
        );
    }
    // `XX XX\\\` breaks between its letters, at its space and before
    // its backslashes, and nowhere between them.
    let all = root_text(&word_break(WordBreak::BreakAll), r"XX XX\\\");
    assert_eq!(breaks(&all), [1, 3, 4, 5]);
}

/// The analysis breaks ASCII text as Chrome 153 does, measured in Ahem, where
/// ICU's rules break it elsewhere:
/// - no break after a slash between letters or digits (`a/b`, `a/1`, `1/a`),
///   so none after `com/` in a URL, where ICU breaks after each slash;
/// - a break at a hyphen before a digit after a letter or digit, and none at
///   a minus sign;
/// - a break after a run of spaces before a closing bracket or a full stop,
///   and after an opening bracket and a space;
/// - a break before an opening bracket after `#` or `=`;
/// - no break after `!`, `|` or `}` before a letter.
///
/// `break-all` takes Blink's break-all table over the pair table. `line-break:
/// anywhere` is untouched.
#[test]
fn ascii_text_breaks_as_chrome_breaks_it() {
    let at = |text: &str| -> Vec<String> {
        let layout = plain(text);
        breaks(&layout)
            .into_iter()
            .map(|at| String::from(&layout.content().text[..at]))
            .collect()
    };
    assert_eq!(at("a/b a/1 1/a"), ["a/b ", "a/b a/1 "]);
    assert_eq!(
        at("https://example.com/a/b and"),
        ["https://example.com/a/b "]
    );
    assert_eq!(
        at("x ABCD-1234 -1 (-1)"),
        ["x ", "x ABCD-", "x ABCD-1234 ", "x ABCD-1234 -1 "]
    );
    assert_eq!(at("a ) b . c"), ["a ", "a ) ", "a ) b ", "a ) b . "]);
    assert_eq!(at("( a"), ["( "]);
    assert_eq!(at("#(=<a(b"), ["#", "#(="]);
    assert!(at("a!b|c}d").is_empty());
    assert_eq!(at("well-known?yes"), ["well-", "well-known?"]);
    // `break-all`: Blink's break-all table between the letters, and its
    // pair table between `?` and `-`, which ICU keeps together (UAX #14
    // LB21).
    let all = root_text(&word_break(WordBreak::BreakAll), "ab#(c");
    assert_eq!(breaks(&all), [1, 2, 3]);
    let all = root_text(&word_break(WordBreak::BreakAll), "x?-y");
    assert_eq!(breaks(&all), [2, 3]);
    // `anywhere`: between every grapheme, whatever the rules say.
    let anywhere = root_text(&line_break(LineBreak::Anywhere), "a/b");
    assert_eq!(breaks(&anywhere), [1, 2]);
}

/// Thai breaks between its words where ICU's Southeast Asian dictionaries find
/// them, with the `dictionaries` feature. The expected breaks are where Chrome
/// 153 wraps a Thai paragraph in a column of no width, taking every
/// opportunity. ICU4C's dictionary and ICU4X's agree on all nineteen. The text
/// says "Thailand has a long history and a diverse culture. Most people are
/// Buddhist, and the capital is Bangkok."
///
/// Without dictionaries the LSTM finds those and four more, inside words
/// Chrome keeps whole: `ประเทศ|ไทย`, "Thailand", `ยาว|นาน`, "long", `ผู้|คน`,
/// "people", and before the abbreviation mark of `กรุงเทพ|ฯ`, "Bangkok".
#[test]
fn thai_breaks_where_chrome_wraps_it() {
    let text = "\u{E1B}\u{E23}\u{E30}\u{E40}\u{E17}\u{E28}\u{E44}\u{E17}\u{E22}\u{E21}\u{E35}\u{E1B}\u{E23}\u{E30}\u{E27}\u{E31}\u{E15}\u{E34}\u{E28}\u{E32}\u{E2A}\u{E15}\u{E23}\u{E4C}\u{E22}\u{E32}\u{E27}\u{E19}\u{E32}\u{E19}\u{E41}\u{E25}\u{E30}\u{E27}\u{E31}\u{E12}\u{E19}\u{E18}\u{E23}\u{E23}\u{E21}\u{E17}\u{E35}\u{E48}\u{E2B}\u{E25}\u{E32}\u{E01}\u{E2B}\u{E25}\u{E32}\u{E22} \u{E1C}\u{E39}\u{E49}\u{E04}\u{E19}\u{E2A}\u{E48}\u{E27}\u{E19}\u{E43}\u{E2B}\u{E0D}\u{E48}\u{E19}\u{E31}\u{E1A}\u{E16}\u{E37}\u{E2D}\u{E28}\u{E32}\u{E2A}\u{E19}\u{E32}\u{E1E}\u{E38}\u{E17}\u{E18}\u{E41}\u{E25}\u{E30}\u{E40}\u{E21}\u{E37}\u{E2D}\u{E07}\u{E2B}\u{E25}\u{E27}\u{E07}\u{E04}\u{E37}\u{E2D}\u{E01}\u{E23}\u{E38}\u{E07}\u{E40}\u{E17}\u{E1E}\u{E2F}";
    // Where each line after the first starts, in UTF-16 units, as Chrome
    // reports them: one a character here.
    let chrome = [
        9, 11, 24, 30, 33, 41, 44, 48, 53, 58, 62, 66, 72, 77, 81, 84, 89, 93, 96,
    ];
    let layout = root_text(&language("th"), text);
    let found: Vec<usize> = breaks(&layout)
        .into_iter()
        .map(|at| text[..at].encode_utf16().count())
        .collect();
    #[cfg(feature = "dictionaries")]
    assert_eq!(found, chrome);
    #[cfg(not(feature = "dictionaries"))]
    {
        let lstm = [
            6, 9, 11, 24, 27, 30, 33, 41, 44, 48, 53, 56, 58, 62, 66, 72, 77, 81, 84, 89, 93, 96,
            103,
        ];
        assert_eq!(found, lstm);
        assert!(chrome.iter().all(|at| lstm.contains(at)));
    }
}

/// Breaks with `text` cut into text nodes at `cuts`, each in its style.
fn breaks_cut(text: &str, cuts: &[(usize, &ComputedStyle<'_>)]) -> Vec<usize> {
    let mut parts = Vec::new();
    let mut last = 0;
    for &(cut, style) in cuts {
        parts.push((&text[last..cut], style));
        last = cut;
    }
    let initial = ComputedStyle::initial();
    let style = cuts.last().map_or(&initial, |&(_, style)| style);
    parts.push((&text[last..], style));
    breaks(&pieces(&parts))
}

/// A seam where the options change is no opportunity, and resuming there
/// must not invent one. LB13 forbids a break before `)` under every option.
#[test]
fn a_property_seam_invents_no_break() {
    let text = "ab)cd";
    let values = [WordBreak::Normal, WordBreak::BreakAll, WordBreak::KeepAll].map(word_break);
    for before in &values {
        for after in &values {
            for split in 1..text.len() {
                let found = breaks(&pieces(&[
                    (&text[..split], before),
                    (&text[split..], after),
                ]));
                assert!(
                    !found.contains(&2),
                    "{before:?} | {after:?} at {split}: {found:?}"
                );
            }
        }
    }
}

/// The options after a seam decide the pair at it.
#[test]
fn the_options_after_a_seam_decide_it() {
    let all = word_break(WordBreak::BreakAll);
    let normal = ComputedStyle::initial();
    // `break-all` after the seam allows the pair across it; `normal` does not.
    assert_eq!(breaks_cut("abcd", &[(2, &normal), (4, &all)]), [2, 3]);
    assert_eq!(breaks_cut("abcd", &[(2, &all), (4, &normal)]), [1]);
}

#[test]
fn identical_options_across_a_seam_match_the_whole_text() {
    let text = "hello there world";
    for value in [WordBreak::Normal, WordBreak::BreakAll, WordBreak::KeepAll] {
        let style = word_break(value);
        let whole = breaks(&root_text(&style, text));
        for split in 1..text.len() {
            assert_eq!(
                breaks(&pieces(&[
                    (&text[..split], &style),
                    (&text[split..], &style)
                ])),
                whole,
                "{value:?} split at {split}"
            );
        }
    }
}

/// An item boundary is not an opportunity, so putting one anywhere moves no
/// break. The texts are chosen for the UAX #14 rules that reach across more
/// than one character, which an implementation restarting at the boundary,
/// or carrying one character of context over it, gets wrong.
#[test]
fn an_item_boundary_never_moves_a_break() {
    let texts = [
        "the quick (brown) fox 1,000 well known jumps over the lazy dog",
        // LB21a: no break after a hyphen following a Hebrew letter.
        "\u{5D0}-b \u{5D1}-c \u{5D2}-d",
        // LB30a: regional indicators break between pairs.
        "\u{1F1FA}\u{1F1F8}\u{1F1EC}\u{1F1E7}\u{1F1EB}\u{1F1F7}",
        "cost 1,234,567.89 total 9,876.54 end",
    ];
    let style = ComputedStyle::initial();
    for text in texts {
        let whole = breaks(&plain(text));
        for cut in 1..text.len() {
            if text.is_char_boundary(cut) {
                assert_eq!(
                    breaks_cut(text, &[(cut, &style)]),
                    whole,
                    "{text:?} cut at {cut}"
                );
            }
        }
    }
}

#[test]
fn several_item_boundaries_never_move_a_break() {
    let text = "\u{1F1FA}\u{1F1F8}\u{1F1EC}\u{1F1E7}\u{1F1EB}\u{1F1F7}";
    let whole = breaks(&plain(text));
    assert_eq!(whole, [8, 16], "between pairs; the end is the paragraph's");
    let style = ComputedStyle::initial();
    for first in (4..text.len()).step_by(4) {
        for second in ((first + 4)..text.len()).step_by(4) {
            assert_eq!(
                breaks_cut(text, &[(first, &style), (second, &style)]),
                whole,
                "cut at {first} and {second}"
            );
        }
    }
}

/// A box edge moves no break, whether it paints or takes room. The
/// opportunity after the space before it belongs to the space.
#[test]
fn a_box_edge_moves_no_break() {
    let painted = styled(|style| style.paints = true);
    let padded = styled(|style| {
        style.edges = EdgesGroup {
            padding: Sides::from_px(4.0),
            ..EdgesGroup::INITIAL
        };
    });
    let whole = breaks(&plain("wrap a colored word"));
    for style in [&painted, &padded] {
        let layout = build(|b| {
            b.text(key(1), "wrap a ");
            b.open_box(key(2), style, None);
            b.text(key(3), "colored");
            b.close_box();
            b.text(key(4), " word");
        });
        assert_eq!(breaks(&layout), whole);
    }
}

/// A float is invisible to analysis but for shaping, which Blink ends at a
/// float. An atomic inline is content.
#[test]
fn a_float_is_invisible_and_an_object_is_content() {
    let whole = plain("office waffle");
    let floated = build(|b| {
        b.text(key(1), "of");
        b.float(
            key(2),
            &ComputedStyle::initial(),
            FloatSide::Left,
            BoxSize::default(),
        );
        b.text(key(3), "fice waffle");
    });
    assert_eq!(clusters(&floated), clusters(&whole));
    assert_eq!(breaks(&floated), breaks(&whole));
    assert_eq!(runs(&floated), runs(&whole));
    assert_eq!(ends_with(&floated, ClusterAttrs::SHAPE_BREAK_AFTER), [2]);
    assert_eq!(
        ends_with(&whole, ClusterAttrs::SHAPE_BREAK_AFTER),
        [] as [usize; 0]
    );

    let object = build(|b| {
        b.text(key(1), "of");
        b.atomic(key(2), &ComputedStyle::initial(), None, BoxSize::default());
        b.text(key(3), "fice waffle");
    });
    assert_eq!(clusters(&object).len(), clusters(&whole).len() + 1);
    assert_eq!(classes(&object)[2], ClusterClass::Object);
    assert_eq!(
        scripts(&object),
        [("of\u{FFFC}fice waffle", String::from("Latn"))]
    );
}

/// The `text-wrap-mode` of the box holding a space or a tab decides whether
/// text may wrap after it. At a boundary between two other characters in two
/// nodes, their nearest common ancestor's decides (CSS Text 3, section 5.1).
///
/// Chrome 153 gives these rows:
/// - `<nowrap>XXX </nowrap>XXX` is one line;
/// - `XXX` then `XXX` where the space is in a span that wraps, inside a block
///   that does not;
/// - `<nowrap>XXX</nowrap>` ending in U+3000 breaks before wrapping text.
#[test]
fn nowrap_is_decided_by_the_space_or_the_nearest_common_ancestor() {
    let layout = build(|b| {
        b.text(key(1), "a ");
        b.open_box(key(2), &nowrap(), None);
        b.text(key(3), "b c");
        b.close_box();
        b.text(key(4), " d");
    });
    // After `a `, the block's space: a break. After `b `, inside the span:
    // none. After ` `, the block's again.
    assert_eq!(breaks(&layout), [2, 6]);

    // Two wrapping boxes inside one that does not: the space ending the
    // first is its own, and so is the break after it.
    let wrap = ComputedStyle::initial();
    let layout = build(|b| {
        b.open_box(key(1), &nowrap(), None);
        b.open_box(key(2), &wrap, None);
        b.text(key(3), "a b ");
        b.close_box();
        b.open_box(key(4), &wrap, None);
        b.text(key(5), "c");
        b.close_box();
        b.close_box();
    });
    assert_eq!(breaks(&layout), [2, 4]);

    // A space ending a `nowrap` span, before text that wraps: no break, the
    // space being the span's; and so for a tab ending a `pre` span in a
    // `pre-wrap` block.
    let layout = build(|b| {
        b.open_box(key(1), &nowrap(), None);
        b.text(key(2), "a ");
        b.close_box();
        b.text(key(3), "b");
    });
    assert_eq!(breaks(&layout), [] as [usize; 0]);
    let pre_wrap = white_space(WhiteSpaceCollapse::Preserve);
    let pre = styled(|s| {
        s.text.white_space_collapse = WhiteSpaceCollapse::Preserve;
        s.text.wrap_mode = TextWrapMode::NoWrap;
    });
    let layout = build_with(&ComputedBlockStyle::new(&pre_wrap), |b| {
        b.open_box(key(1), &pre, None);
        b.text(key(2), "a\t");
        b.close_box();
        b.text(key(3), "b");
    });
    assert_eq!(breaks(&layout), [] as [usize; 0]);
    // An ideographic space is no space that disappears at a break: the
    // boundary after it is the nearest common ancestor's, as in Chrome.
    let layout = build(|b| {
        b.open_box(key(1), &nowrap(), None);
        b.text(key(2), "a\u{3000}");
        b.close_box();
        b.text(key(3), "b");
    });
    assert_eq!(breaks(&layout), [4]);

    // A forced break stays one.
    let layout = root_text(
        &styled(|s| {
            s.text.wrap_mode = TextWrapMode::NoWrap;
            s.text.white_space_collapse = WhiteSpaceCollapse::Preserve;
        }),
        "a b\nc d",
    );
    assert_eq!(breaks(&layout), [] as [usize; 0]);
    assert_eq!(paragraphs(&layout), ["a b\n", "c d"]);
}

/// `break-spaces` breaks after every preserved space and tab, and after every
/// other space separator that breaks, between two of them too. Nothing hangs.
#[test]
fn break_spaces_breaks_after_every_space_and_nothing_hangs() {
    let layout = root_text(&white_space(WhiteSpaceCollapse::BreakSpaces), "a  b\t\tc");
    assert_eq!(breaks(&layout), [2, 3, 5, 6]);
    assert_eq!(hanging(&layout), [] as [&str; 0]);
    let layout = root_text(&white_space(WhiteSpaceCollapse::Preserve), "a  b\t\tc");
    assert_eq!(breaks(&layout), [3, 6]);
    assert_eq!(hanging(&layout), [" ", " ", "\t", "\t"]);
    // And after every other space separator that breaks, between two
    // ideographic spaces too, where UAX #14 alone would not break, as Chrome
    // does (WPT `break-spaces-with-ideographic-space-001`); never after a
    // no-break space.
    let ideographic = "a\u{3000}\u{3000}b";
    let layout = root_text(&white_space(WhiteSpaceCollapse::BreakSpaces), ideographic);
    assert_eq!(breaks(&layout), [4, 7]);
    assert_eq!(hanging(&layout), [] as [&str; 0]);
    let layout = root_text(&white_space(WhiteSpaceCollapse::Preserve), ideographic);
    assert_eq!(breaks(&layout), [7]);
    let layout = root_text(
        &white_space(WhiteSpaceCollapse::BreakSpaces),
        "a\u{A0}\u{A0}b",
    );
    assert_eq!(breaks(&layout), [] as [usize; 0]);
    // Not before a forced break, which ends the line there anyway: the
    // separator is never a line of its own.
    let layout = root_text(&white_space(WhiteSpaceCollapse::BreakSpaces), "a  \nb");
    assert_eq!(breaks(&layout), [2]);
    let layout = build(|b| {
        b.atomic(key(1), &ComputedStyle::initial(), None, BoxSize::default());
        b.line_break(key(2));
    });
    assert_eq!(breaks(&layout), [] as [usize; 0]);
}

#[test]
fn hyphens_none_declines_a_soft_hyphen() {
    let manual = plain("soft\u{AD}ware");
    assert_eq!(breaks(&manual), [6]);
    let none = root_text(
        &styled(|s| s.text.hyphens = Hyphens::None),
        "soft\u{AD}ware",
    );
    assert_eq!(breaks(&none), [] as [usize; 0]);
}

/// An atomic inline has an opportunity on either side, even beside a
/// character that would suppress one. It has none beside GL other than
/// U+00A0, WJ or ZWJ (CSS Text 3, section 5.1). UAX #14 alone, running through
/// the U+FFFC, would say no break before `)` or after `(`. Chrome is not yet
/// probed here.
#[test]
fn an_atomic_inline_has_an_opportunity_on_either_side_but_beside_glue() {
    let around = |before: &str, after: &str| {
        let layout = build(|b| {
            b.text(key(1), before);
            b.atomic(key(2), &ComputedStyle::initial(), None, BoxSize::default());
            b.text(key(3), after);
        });
        let object = before.len();
        let found = breaks(&layout);
        (found.contains(&object), found.contains(&(object + 3)))
    };
    assert_eq!(around("a", "b"), (true, true));
    assert_eq!(around("(", ")"), (true, true));
    assert_eq!(around("\u{A0}", "\u{A0}"), (true, true), "U+00A0 is exempt");
    assert_eq!(
        around("\u{202F}", "\u{202F}"),
        (false, false),
        "other GL is not"
    );
    assert_eq!(around("\u{2060}", "\u{2060}"), (false, false), "WJ");
    assert_eq!(around("a\u{200D}", "\u{200D}b"), (false, false), "ZWJ");
    // And nowrap around it still holds.
    let layout = build_with(&ComputedBlockStyle::new(&nowrap()), |b| {
        b.text(key(1), "a");
        b.atomic(key(2), &nowrap(), None, BoxSize::default());
        b.text(key(3), "b");
    });
    assert_eq!(breaks(&layout), [] as [usize; 0]);
}

#[test]
fn word_break_and_line_break_choose_the_segmenter() {
    let text = "日本語 abc";
    let with = |style: &ComputedStyle<'_>| breaks(&root_text(style, text));
    assert_eq!(with(&ComputedStyle::initial()), [3, 6, 10]);
    assert_eq!(with(&word_break(WordBreak::KeepAll)), [10]);
    assert_eq!(with(&word_break(WordBreak::BreakAll)), [3, 6, 10, 11, 12]);
    let anywhere = line_break(LineBreak::Anywhere);
    assert_eq!(with(&anywhere), [3, 6, 9, 10, 11, 12]);
    // `keep-all` with `anywhere` is anywhere, as CSS says.
    let both = styled(|s| {
        s.text.word_break = WordBreak::KeepAll;
        s.text.line_break = LineBreak::Anywhere;
    });
    assert_eq!(with(&both), with(&anywhere));
}

/// Under `line-break: normal` a small kana may start a line, as in Chrome,
/// whose line breaker is ICU's. `strict` keeps it with what precedes it.
/// CSS now holds it back under `normal` too, and Chrome does not;
/// `Config::small_kana` chooses.
#[test]
fn a_small_kana_may_start_a_line_under_normal_as_in_chrome() {
    let text = "すぁ";
    assert_eq!(breaks(&plain(text)), [3]);
    assert_eq!(
        breaks(&root_text(&line_break(LineBreak::Strict), text)),
        [] as [usize; 0]
    );
}

/// A layout of one text in `root`, analyzed with small kana `small_kana`.
fn text_with_kana(small_kana: SmallKana, root: &ComputedStyle<'_>, text: &str) -> Layout {
    let mut cx = no_fonts();
    let mut config = *cx.config();
    config.small_kana = small_kana;
    cx.set_config(config);
    let mut layout = Layout::new();
    let mut builder = layout.builder(
        NodeKey(0),
        &ComputedBlockStyle::new(root),
        BuildOptions::default(),
    );
    builder.text(key(1), text);
    builder.finish(&mut cx);
    check(&layout);
    layout
}

/// `SmallKana::Held` keeps a small kana and the prolonged sound mark with
/// what precedes them under `normal`, as CSS Text 3 now says
/// (csswg-drafts#10363). `loose` still lets one start a line. A space before
/// one still breaks, as UAX #14 breaks before NS after a space (LB18).
/// Chrome's `MayStartLine` is the default.
#[test]
fn small_kana_are_held_under_normal_where_the_config_says() {
    use crate::config::SmallKana::{Held, MayStartLine};
    let normal = ComputedStyle::initial();
    // す ぁ ニ ャ ー: a break after each under Chrome's rule.
    let text = "すぁニャー";
    assert_eq!(
        breaks(&text_with_kana(MayStartLine, &normal, text)),
        [3, 6, 9, 12]
    );
    assert_eq!(breaks(&text_with_kana(Held, &normal, text)), [6]);
    let loose = line_break(LineBreak::Loose);
    assert_eq!(breaks(&text_with_kana(Held, &loose, text)), [3, 6, 9, 12]);
    // After a space the small kana may start a line either way.
    assert_eq!(breaks(&text_with_kana(Held, &normal, "す ぁ")), [4]);
    // Strict holds it whatever the config.
    let strict = line_break(LineBreak::Strict);
    assert_eq!(breaks(&text_with_kana(MayStartLine, &strict, text)), [6]);
    // Held is strict's rule for it: after an ideographic space, which is
    // BA, it holds as strict does, and under `break-spaces` it follows the
    // break after the space.
    let spaced = "す\u{3000}ぁ";
    assert_eq!(
        breaks(&text_with_kana(MayStartLine, &strict, spaced)),
        [] as [usize; 0]
    );
    assert_eq!(
        breaks(&text_with_kana(Held, &normal, spaced)),
        [] as [usize; 0]
    );
    assert_eq!(breaks(&text_with_kana(MayStartLine, &normal, spaced)), [6]);
    let kept = white_space(WhiteSpaceCollapse::BreakSpaces);
    assert_eq!(breaks(&text_with_kana(Held, &kept, spaced)), [6]);
}

/// The CJK spacing passes skip a paragraph whose gate is closed. The gate
/// opens for every character `text-autospace` takes for an ideograph, so no
/// paragraph it leaves closed can hold a seam.
#[test]
fn the_east_asian_gate_covers_every_ideograph_autospace_sees() {
    use crate::unicode::{EastAsianSpacing, rare_props};
    let missed: Vec<String> = (0..=0x10FFFF_u32)
        .filter_map(char::from_u32)
        .filter(|&ch| rare_props(ch).east_asian_spacing() == EastAsianSpacing::Wide)
        .filter(|&ch| !super::classify::is_east_asian(ch))
        .map(|ch| format!("U+{:04X}", u32::from(ch)))
        .collect();
    assert!(missed.is_empty(), "{}", missed.join(" "));
}

/// The Latin-1 fast path takes each of its characters as the general walk
/// does: a cluster of the fast path's class, standing alone, no separator, no
/// small kana, at its paragraph's level and not East Asian. No other
/// character is a fast-path character.
#[test]
fn latin_1_fast_path_characters_analyse_as_the_general_walk_does() {
    let layout = plain("a");
    let content = layout.content();
    let fast: Vec<u32> = (0..=0xFF_u32)
        .filter_map(char::from_u32)
        .filter(|&ch| super::classify::latin1_fast_agrees(ch, content))
        .map(u32::from)
        .collect();
    let printable = (0x20..=0x7E).chain(0xA0..=0xFF).filter(|&cp| cp != 0xAD);
    assert_eq!(fast, printable.collect::<Vec<u32>>());
}

/// Japanese and Chinese tailor `normal`: a break before U+301C `〜`,
/// which the stream resumes for at a seam into Japanese.
#[test]
fn japanese_and_chinese_tailor_normal() {
    let text = "あ\u{301C}";
    assert_eq!(breaks(&root_text(&language("ja"), text)), [3]);
    assert_eq!(breaks(&root_text(&language("zh-Hant"), text)), [3]);
    assert_eq!(breaks(&root_text(&language("en"), text)), [] as [usize; 0]);
    let strict_ja = styled(|s| {
        s.text.language = Language::parse("ja").ok();
        s.text.line_break = LineBreak::Strict;
    });
    assert_eq!(breaks(&root_text(&strict_ja, text)), [] as [usize; 0]);
    let en = language("en");
    let ja = language("ja");
    assert_eq!(breaks_cut(text, &[(3, &en), (6, &ja)]), [3]);
    assert_eq!(breaks_cut(text, &[(3, &ja), (6, &en)]), [] as [usize; 0]);
}

/// `overflow-wrap: anywhere` and `break-word` allow an emergency break after
/// every cluster of text that wraps. They allow none after a paragraph's last
/// cluster or inside a divided grapheme.
#[test]
fn emergencies_follow_overflow_wrap() {
    for wrap in [OverflowWrap::Anywhere, OverflowWrap::BreakWord] {
        let style = styled(|s| s.text.overflow_wrap = wrap);
        let layout = root_text(&style, "abc de");
        assert_eq!(
            ends_with(&layout, ClusterAttrs::EMERGENCY_AFTER),
            [1, 2, 3, 4, 5]
        );
        let layout = build_with(&ComputedBlockStyle::new(&style), |b| {
            b.text(key(1), "ae");
            b.text(key(2), "\u{301}b");
        });
        assert_eq!(ends_with(&layout, ClusterAttrs::EMERGENCY_AFTER), [1, 4]);
    }
    let layout = plain("abc de");
    assert_eq!(
        ends_with(&layout, ClusterAttrs::EMERGENCY_AFTER),
        [] as [usize; 0]
    );
    let unwrapped = styled(|s| {
        s.text.overflow_wrap = OverflowWrap::Anywhere;
        s.text.wrap_mode = TextWrapMode::NoWrap;
    });
    let layout = root_text(&unwrapped, "abc de");
    assert_eq!(
        ends_with(&layout, ClusterAttrs::EMERGENCY_AFTER),
        [] as [usize; 0]
    );
}

/// Spaces, tabs, the other space separators and separators hang. A no-break
/// space never does.
///
/// A `<wbr>`'s U+200B and the one collapsing keeps hang and take no room. The
/// white space before them hangs through them at a line's end, as Blink
/// passes over them. A U+200B the caller writes is text.
#[test]
fn spaces_and_separators_hang_and_a_no_break_space_does_not() {
    let layout = plain("a b\u{A0}c\u{3000}d\u{2003}e");
    assert_eq!(hanging(&layout), [" ", "\u{3000}", "\u{2003}"]);
    let layout = pre("a \t\u{A0}\n");
    assert_eq!(hanging(&layout), [" ", "\t", "\n"]);
    let layout = build(|b| {
        b.text(key(1), "a \u{200B}b ");
        b.break_opportunity();
        b.text(key(1), "c");
        b.open_box(key(2), &nowrap(), None);
        b.text(key(3), "d ");
        b.close_box();
        b.text(key(4), " e");
    });
    assert_eq!(layout.content().text, "a \u{200B}b \u{200B}cd \u{200B}e");
    assert_eq!(hanging(&layout), [" ", " ", "\u{200B}", " ", "\u{200B}"]);
}
