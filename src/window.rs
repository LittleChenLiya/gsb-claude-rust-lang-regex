/*!
Support for searching a byte window of a haystack without re-basing
coordinates.

The window query entry points (such as [`crate::Regex::find_in`] and
[`crate::Regex::captures_in`]) let a caller restrict *where* in a haystack a
complete match may appear while still searching the original haystack. That
is, the haystack is not sliced: byte offsets reported by the engine remain
absolute offsets into the original haystack, and the bytes outside of the
window remain visible to anchors, look-around assertions and word boundary
assertions.

This module contains the shared window validation logic and the
[`WindowError`] returned when a caller supplies an invalid window.
*/

use core::ops::{Bound, RangeBounds};

use regex_automata::Input;

/// An error returned when a search window is invalid for a haystack.
///
/// A window is the half-open byte range `start..end` in which the complete
/// match must be contained. Two kinds of invalid windows are possible:
///
/// * The window bounds are out of range for the haystack (or the range is
///   inverted). This is [`WindowError::OutOfBounds`].
/// * For haystacks given as `&str`, both bounds must fall on a [UTF-8
///   code point boundary]. A bound that would split the bytes of a single
///   encoded character is rejected with [`WindowError::NotCharBoundary`].
///
/// Note that for `&[u8]` haystacks (as used by the [`crate::bytes`] API),
/// only the out-of-bounds error can occur since arbitrary bytes need not be
/// valid UTF-8.
///
/// [UTF-8 code point boundary]: https://docs.rs/regex/latest/regex/struct.Regex.html#byte-offsets
///
/// # Example
///
/// ```
/// use regex::Regex;
///
/// let hay = "a猫b"; // bytes: a=0, 猫=1..4, b=4
/// let re = Regex::new(".").unwrap();
/// // Offset 2 is in the middle of the UTF-8 encoding of `猫`.
/// assert!(re.find_in(hay, 2..4).is_err());
/// // The same bounds on a complete code point are fine.
/// assert!(re.find_in(hay, 1..4).is_ok());
/// ```
#[non_exhaustive]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum WindowError {
    /// The given window was not a valid half-open range for the haystack.
    ///
    /// This occurs when a bound exceeds the haystack length, when the range
    /// is inverted (`start > end`), or when a bound cannot be represented as
    /// a `usize` (for example, `0..=usize::MAX`).
    ///
    /// The values are the haystack length, and the (inclusive) lower and
    /// (exclusive) upper bounds as resolved from the range provided.
    OutOfBounds {
        /// The length of the haystack that was searched.
        haystack_len: usize,
        /// The inclusive lower bound of the window.
        start: usize,
        /// The exclusive upper bound of the window.
        end: usize,
    },
    /// A window bound for a `&str` haystack did not fall on a UTF-8 code
    /// point boundary.
    ///
    /// `offset` is the offending byte offset.
    NotCharBoundary {
        /// The byte offset that split the encoding of a code point.
        offset: usize,
    },
}

impl core::fmt::Display for WindowError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match *self {
            WindowError::OutOfBounds { haystack_len, start, end } => write!(
                f,
                "invalid search window {start}..{end} for haystack of \
                 length {haystack_len}",
            ),
            WindowError::NotCharBoundary { offset } => write!(
                f,
                "search window bound at byte offset {offset} is not on a \
                 valid UTF-8 code point boundary",
            ),
        }
    }
}

#[cfg(feature = "std")]
impl std::error::Error for WindowError {}

/// Resolve arbitrary `RangeBounds` into a concrete `start..end`, rejecting
/// windows that do not fit within a haystack of the given length.
#[inline]
pub(crate) fn resolve<R: RangeBounds<usize>>(
    haystack_len: usize,
    range: R,
) -> Result<core::ops::Range<usize>, WindowError> {
    let start = match range.start_bound() {
        Bound::Included(&i) => i,
        Bound::Excluded(&i) => match i.checked_add(1) {
            Some(i) => i,
            None => {
                return Err(WindowError::OutOfBounds {
                    haystack_len,
                    start: usize::MAX,
                    end: usize::MAX,
                })
            }
        },
        Bound::Unbounded => 0,
    };
    let end = match range.end_bound() {
        Bound::Included(&i) => match i.checked_add(1) {
            Some(i) => i,
            None => {
                return Err(WindowError::OutOfBounds {
                    haystack_len,
                    start,
                    end: usize::MAX,
                })
            }
        },
        Bound::Excluded(&i) => i,
        Bound::Unbounded => haystack_len,
    };
    if start > end || end > haystack_len {
        return Err(WindowError::OutOfBounds { haystack_len, start, end });
    }
    Ok(start..end)
}

/// Build an `Input` for a `&str` haystack restricted to the given window,
/// requiring both window bounds to fall on UTF-8 code point boundaries.
#[inline]
pub(crate) fn input_str<'h, R: RangeBounds<usize>>(
    haystack: &'h str,
    range: R,
) -> Result<Input<'h>, WindowError> {
    let window = resolve(haystack.len(), range)?;
    // `str::is_char_boundary` lets us reject windows that would split the
    // UTF-8 encoding of a code point *before* the engine ever sees them.
    if !haystack.is_char_boundary(window.start) {
        return Err(WindowError::NotCharBoundary { offset: window.start });
    }
    if !haystack.is_char_boundary(window.end) {
        return Err(WindowError::NotCharBoundary { offset: window.end });
    }
    Ok(Input::new(haystack).span(window))
}

/// Build an `Input` for a `&[u8]` haystack restricted to the given window.
///
/// Arbitrary byte haystacks need not be valid UTF-8, so only in-bounds
/// windows are required.
#[inline]
pub(crate) fn input_bytes<'h, R: RangeBounds<usize>>(
    haystack: &'h [u8],
    range: R,
) -> Result<Input<'h>, WindowError> {
    let window = resolve(haystack.len(), range)?;
    Ok(Input::new(haystack).span(window))
}
