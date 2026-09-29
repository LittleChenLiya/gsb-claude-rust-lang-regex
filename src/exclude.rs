/*!
Search for regex matches while skipping excluded byte ranges.

This module provides the shared implementation backing
[`Regex::find_iter_excluding`](crate::Regex::find_iter_excluding) and
[`regex::bytes::Regex::find_iter_excluding`](crate::bytes::Regex::find_iter_excluding).

The key idea is that the regex is always executed over the _whole_ original
haystack. Excluded ranges only affect which of the resulting matches are
reported. Consequently, anchors such as `^` and `$` and assertions such as
`\b` see the same surrounding context they would see in a normal search;
the haystack is never split into pieces.
*/

use alloc::vec::Vec;
use core::ops::Range;

use crate::{bytes, Regex};

/// An error that can occur when building an exclusion-aware match iterator.
///
/// Excluded ranges are given as half-open UTF-8 byte ranges. They must be
/// fully inside the haystack, their start offset must not be greater than
/// their end offset, and, when searching a `&str`, both offsets must lie on
/// a UTF-8 character boundary.
#[non_exhaustive]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ExclusionError {
    /// A range whose start offset is greater than its end offset.
    InvalidRange {
        /// The index of the offending range as given by the caller.
        index: usize,
        /// The start offset of the offending range.
        start: usize,
        /// The end offset of the offending range.
        end: usize,
    },
    /// A range whose end offset is beyond the end of the haystack.
    OutOfBounds {
        /// The index of the offending range as given by the caller.
        index: usize,
        /// The end offset of the offending range.
        end: usize,
        /// The length of the haystack in bytes.
        haystack_len: usize,
    },
    /// A range endpoint that is not a UTF-8 character boundary.
    ///
    /// This can only occur when searching a `&str`. The byte-oriented
    /// [`bytes::Regex`] API permits ranges that split UTF-8 sequences.
    NotCharBoundary {
        /// The index of the offending range as given by the caller.
        index: usize,
        /// The offset that is not a UTF-8 character boundary.
        offset: usize,
    },
}

#[cfg(feature = "std")]
impl std::error::Error for ExclusionError {}

impl core::fmt::Display for ExclusionError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match *self {
            ExclusionError::InvalidRange { index, start, end } => write!(
                f,
                "excluded range at index {index} has start {start} greater \
                 than end {end}",
            ),
            ExclusionError::OutOfBounds { index, end, haystack_len } => {
                write!(
                    f,
                    "excluded range at index {index} ends at {end}, but the \
                 haystack is only {haystack_len} bytes long",
                )
            }
            ExclusionError::NotCharBoundary { index, offset } => write!(
                f,
                "excluded range at index {index} has offset {offset} that is \
                 not a UTF-8 character boundary",
            ),
        }
    }
}

/// The haystack operations needed by the exclusion-aware iterator.
pub(crate) trait Hay {
    /// The length of the haystack in bytes.
    fn len(&self) -> usize;

    /// Whether the given offset is a valid UTF-8 character boundary.
    fn is_char_boundary(&self, at: usize) -> bool;

    /// The smallest valid search start strictly after `at`.
    ///
    /// For `&str`, this advances to the next UTF-8 character boundary so that
    /// a search never starts in the middle of a codepoint. For `&[u8]`, this
    /// is simply `at + 1`.
    fn next_search_start(&self, at: usize) -> usize;
}

impl Hay for str {
    #[inline]
    fn len(&self) -> usize {
        str::len(self)
    }

    #[inline]
    fn is_char_boundary(&self, at: usize) -> bool {
        str::is_char_boundary(self, at)
    }

    #[inline]
    fn next_search_start(&self, at: usize) -> usize {
        if at >= str::len(self) {
            return at + 1;
        }
        let mut next = at + 1;
        while !str::is_char_boundary(self, next) {
            next += 1;
        }
        next
    }
}

impl Hay for [u8] {
    #[inline]
    fn len(&self) -> usize {
        <[u8]>::len(self)
    }

    #[inline]
    fn is_char_boundary(&self, _at: usize) -> bool {
        true
    }

    #[inline]
    fn next_search_start(&self, at: usize) -> usize {
        at + 1
    }
}

/// Execution of a leftmost search at a particular byte offset.
///
/// The search is always performed over the whole haystack, so anchors and
/// look-around take the entire surrounding context into account.
pub(crate) trait FindIn<H: Hay + ?Sized> {
    /// Returns the byte range of the leftmost match starting at or after
    /// `at`, or `None` if there is no such match.
    fn find_in_at(&self, haystack: &H, at: usize) -> Option<(usize, usize)>;
}

impl FindIn<str> for Regex {
    #[inline]
    fn find_in_at(&self, haystack: &str, at: usize) -> Option<(usize, usize)> {
        self.find_at(haystack, at).map(|m| (m.start(), m.end()))
    }
}

impl FindIn<[u8]> for bytes::Regex {
    #[inline]
    fn find_in_at(
        &self,
        haystack: &[u8],
        at: usize,
    ) -> Option<(usize, usize)> {
        self.find_at(haystack, at).map(|m| (m.start(), m.end()))
    }
}

/// Validates the excluded ranges and merges any that overlap.
///
/// Adjacent (but non-overlapping) ranges are kept separate: a zero-width
/// match at their shared endpoint is visible either way.
fn normalize<H: Hay + ?Sized>(
    haystack: &H,
    ranges: &[Range<usize>],
) -> Result<Vec<Range<usize>>, ExclusionError> {
    for (index, range) in ranges.iter().enumerate() {
        if range.start > range.end {
            return Err(ExclusionError::InvalidRange {
                index,
                start: range.start,
                end: range.end,
            });
        }
        if range.end > haystack.len() {
            return Err(ExclusionError::OutOfBounds {
                index,
                end: range.end,
                haystack_len: haystack.len(),
            });
        }
        if !haystack.is_char_boundary(range.start) {
            return Err(ExclusionError::NotCharBoundary {
                index,
                offset: range.start,
            });
        }
        if !haystack.is_char_boundary(range.end) {
            return Err(ExclusionError::NotCharBoundary {
                index,
                offset: range.end,
            });
        }
    }
    let mut merged: Vec<Range<usize>> = ranges.to_vec();
    merged.sort_unstable_by_key(|range| range.start);
    let mut normalized: Vec<Range<usize>> = Vec::with_capacity(merged.len());
    for range in merged {
        match normalized.last_mut() {
            Some(last) if range.start < last.end => {
                if range.end > last.end {
                    last.end = range.end;
                }
            }
            _ => normalized.push(range),
        }
    }
    Ok(normalized)
}

/// An iterator over the non-overlapping matches that do not overlap any
/// excluded byte range.
#[derive(Debug)]
pub(crate) struct Excluding<'r, 'h, R, H: ?Sized> {
    regex: &'r R,
    haystack: &'h H,
    skipped: Vec<Range<usize>>,
    start: usize,
    last_end: Option<usize>,
}

impl<'r, 'h, R, H: Hay + ?Sized> Clone for Excluding<'r, 'h, R, H> {
    fn clone(&self) -> Self {
        Excluding {
            regex: self.regex,
            haystack: self.haystack,
            skipped: self.skipped.clone(),
            start: self.start,
            last_end: self.last_end,
        }
    }
}

impl<'r, 'h, R, H: Hay + ?Sized> Excluding<'r, 'h, R, H> {
    /// Creates a new exclusion-aware iterator.
    pub(crate) fn new(
        regex: &'r R,
        haystack: &'h H,
        ranges: &[Range<usize>],
    ) -> Result<Excluding<'r, 'h, R, H>, ExclusionError> {
        let skipped = normalize(haystack, ranges)?;
        Ok(Excluding { regex, haystack, skipped, start: 0, last_end: None })
    }

    /// Returns the haystack being searched.
    pub(crate) fn haystack(&self) -> &'h H {
        self.haystack
    }

    /// Returns the excluded range overlapping the match at `s..e`, if any.
    ///
    /// A match overlaps when it consumes an excluded byte, or when it is a
    /// zero-width match whose position belongs to a hidden region. The only
    /// positions inside ranges that remain visible are the shared boundary
    /// between two non-overlapping (merely adjacent) non-empty ranges.
    #[inline]
    fn overlapping(&self, s: usize, e: usize) -> Option<&Range<usize>> {
        if s == e {
            // The ranges are sorted and overlapping ones are merged, so a
            // position belongs to at most one range. A position at the
            // shared boundary of two merely adjacent ranges is visible.
            let containing =
                self.skipped.iter().find(|r| s >= r.start && s < r.end);
            let shared_visible_boundary =
                self.skipped.windows(2).any(|pair| {
                    pair[0].start < s
                        && pair[0].end == s
                        && pair[1].start == s
                        && pair[1].end > s
                });
            containing.filter(|_| !shared_visible_boundary)
        } else {
            self.overlapping_nonempty(s, e)
        }
    }

    /// Overlap test for a non-empty match.
    #[inline]
    fn overlapping_nonempty(
        &self,
        s: usize,
        e: usize,
    ) -> Option<&Range<usize>> {
        let i = self.skipped.partition_point(|range| range.start <= s);
        if i > 0 {
            let range = &self.skipped[i - 1];
            if s < range.end && e > range.start {
                return Some(range);
            }
        }
        if i < self.skipped.len() {
            let range = &self.skipped[i];
            if s < range.start && e > range.start {
                return Some(range);
            }
        }
        None
    }
}

impl<'r, 'h, R, H> Iterator for Excluding<'r, 'h, R, H>
where
    R: FindIn<H>,
    H: Hay + ?Sized,
{
    type Item = (usize, usize);

    fn next(&mut self) -> Option<(usize, usize)> {
        loop {
            // `start == haystack.len()` is a legal final search position
            // (it can match trailing empty assertions such as `$`).
            if self.start > self.haystack.len() {
                return None;
            }
            // Reproduce the standard non-overlapping iteration first, so
            // that hidden matches affect empty-match progression exactly
            // as they would in a normal search: an empty match beginning
            // where the previous (possibly hidden) match ended is dropped
            // and the search restarts at the next position.
            let mut found =
                self.regex.find_in_at(self.haystack, self.start)?;
            if found.0 == found.1 && Some(found.0) == self.last_end {
                self.start = self.haystack.next_search_start(found.0);
                found = self.regex.find_in_at(self.haystack, self.start)?;
            }
            let (s, e) = found;
            // As in the standard iterator, the end of the latest found
            // match (even if it is hidden) suppresses an overlapping empty
            // match on the next step.
            self.last_end = Some(e);
            self.start = e;

            if self.overlapping(s, e).is_some() {
                // Retry from after the discarded match. A non-empty match
                // resumes at its end (otherwise the same leftmost match
                // would be found again); a hidden empty match moves to the
                // next search position, preserving the standard empty-match
                // progression even when the forced position lands inside a
                // hidden range.
                self.start = if s == e {
                    self.haystack.next_search_start(s)
                } else {
                    e
                };
                continue;
            }
            return Some((s, e));
        }
    }
}
