//! Streams of characters with their offsets, which the segmenters read.
//!
//! A segmenter reads any cloneable iterator of `(offset, char)`, in the
//! units the text is stored in: bytes for UTF-8, code units for UTF-16, or
//! anything else that grows from one character to the next. Lookahead is a
//! clone of the iterator. [`Utf8Scalars`] and [`Utf16Scalars`] are the
//! streams of the two common encodings.

use core::str::CharIndices;

/// The characters of a UTF-8 string, with their byte offsets.
///
/// # Examples
///
/// ```
/// use cleft::Utf8Scalars;
///
/// let chars: Vec<_> = Utf8Scalars::with_start("añb", 1).collect();
/// assert_eq!(chars, [(1, 'ñ'), (3, 'b')]);
/// ```
#[derive(Clone, Debug)]
pub struct Utf8Scalars<'a> {
    chars: CharIndices<'a>,
    /// The offset of `chars` in the whole string.
    base: usize,
}

impl<'a> Utf8Scalars<'a> {
    /// Creates a stream of the characters of `text`, from its start.
    #[inline]
    pub fn new(text: &'a str) -> Self {
        Self::with_start(text, 0)
    }

    /// Creates a stream of the characters of `text` from byte `start`, with
    /// offsets from the start of `text`.
    ///
    /// The stream is empty if `start` is past the end of `text` or not on a
    /// character boundary.
    #[inline]
    pub fn with_start(text: &'a str, start: usize) -> Self {
        let (rest, base) = match text.get(start..) {
            Some(rest) => (rest, start),
            None => ("", text.len()),
        };
        Self {
            chars: rest.char_indices(),
            base,
        }
    }
}

impl Iterator for Utf8Scalars<'_> {
    type Item = (usize, char);

    #[inline]
    fn next(&mut self) -> Option<(usize, char)> {
        let (offset, ch) = self.chars.next()?;
        Some((self.base + offset, ch))
    }

    #[inline]
    fn size_hint(&self) -> (usize, Option<usize>) {
        self.chars.size_hint()
    }
}

/// The characters of a UTF-16 string, with their code unit offsets.
///
/// An unpaired surrogate reads as U+FFFD REPLACEMENT CHARACTER, one
/// character a code unit.
///
/// # Examples
///
/// ```
/// use cleft::Utf16Scalars;
///
/// let units = [0x61, 0xD83D, 0xDE00, 0xDC00];
/// let chars: Vec<_> = Utf16Scalars::new(&units).collect();
/// assert_eq!(chars, [(0, 'a'), (1, '😀'), (3, '\u{FFFD}')]);
/// ```
#[derive(Clone, Debug)]
pub struct Utf16Scalars<'a> {
    units: &'a [u16],
    /// The offset of the next code unit.
    at: usize,
}

impl<'a> Utf16Scalars<'a> {
    /// Creates a stream of the characters of `units`, from their start.
    #[inline]
    pub fn new(units: &'a [u16]) -> Self {
        Self::with_start(units, 0)
    }

    /// Creates a stream of the characters of `units` from code unit `start`,
    /// with offsets from the start of `units`.
    ///
    /// The stream is empty if `start` is past the end. A `start` between the
    /// two halves of a surrogate pair reads the second half as U+FFFD.
    #[inline]
    pub fn with_start(units: &'a [u16], start: usize) -> Self {
        Self {
            units,
            at: start.min(units.len()),
        }
    }
}

impl Iterator for Utf16Scalars<'_> {
    type Item = (usize, char);

    #[inline]
    fn next(&mut self) -> Option<(usize, char)> {
        let at = self.at;
        let unit = u32::from(*self.units.get(at)?);
        self.at = at + 1;
        if !(0xD800..0xE000).contains(&unit) {
            return Some((
                at,
                char::from_u32(unit).unwrap_or(char::REPLACEMENT_CHARACTER),
            ));
        }
        let low = self.units.get(at + 1).map_or(0, |&low| u32::from(low));
        if unit < 0xDC00 && (0xDC00..0xE000).contains(&low) {
            self.at = at + 2;
            let scalar = 0x1_0000 + ((unit - 0xD800) << 10) + (low - 0xDC00);
            return Some((
                at,
                char::from_u32(scalar).unwrap_or(char::REPLACEMENT_CHARACTER),
            ));
        }
        Some((at, char::REPLACEMENT_CHARACTER))
    }

    #[inline]
    fn size_hint(&self) -> (usize, Option<usize>) {
        let left = self.units.len().saturating_sub(self.at);
        (left.div_ceil(2), Some(left))
    }
}
