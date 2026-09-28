/*!
Find matches while skipping over excluded byte ranges.

This module provides a SkipRanges iterator that finds matches of a Regex
in a haystack while skipping over a caller-provided list of excluded byte
ranges (for example, comments or otherwise hidden content).

The regex is always executed against the *entire* original haystack. The
haystack is never split into pieces. This means that look-around
assertions such as start-of-text, end-of-text and word-boundary assertions
(including the multi-line variants) keep their usual meaning relative to
the whole haystack, even for matches that are adjacent to an excluded
range.

Excluded ranges are given as UTF-8 byte offsets in the half-open form
start..end. When a SkipRanges iterator is created, the ranges are validated
(every bound must be in bounds and on a UTF-8 codepoint boundary) and then
sorted and merged, with overlapping *and* directly adjacent ranges
coalesced into one.

A match is yielded only when its byte range does not intersect any excluded
range. Matches that overlap an excluded range are skipped, and the search
then continues at the end of the overlapping excluded range. As a special
case, a zero-width match is considered excluded only when its position lies
strictly inside an excluded range, so that a zero-width match sitting
exactly on the boundary of an excluded range is still reported.

# Example

```
use regex::skip_ranges;

let hay = "你好，世界！hello 世界";
let re = regex::Regex::new(r"\w+").unwrap();
let matches = skip_ranges::find_all(&re, hay, &[6..18]).unwrap();
assert_eq!(matches, vec![0..6, 18..23, 24..30]);
```
*/

use alloc::vec::Vec;
use core::ops::Range;

use crate::{Match, Regex};

/// An error that occurred while validating a list of excluded byte ranges.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ExcludedRangesError {
    /// A range whose start is greater than its end was provided. The range
    /// is the offending start..end pair.
    Reversed(Range<usize>),
    /// A range bound is beyond the end of the haystack.
    OutOfBounds {
        /// The offending start..end range.
        range: Range<usize>,
        /// The length of the haystack in bytes.
        haystack_len: usize,
    },
    /// A range bound does not fall on a UTF-8 codepoint boundary of the
    /// haystack.
    NotCharBoundary {
        /// The offending byte offset.
        offset: usize,
    },
}

impl core::fmt::Display for ExcludedRangesError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match *self {
            ExcludedRangesError::Reversed(ref range) => write!(
                f,
                "excluded range {}..{} is reversed (start > end)",
                range.start, range.end,
            ),
            ExcludedRangesError::OutOfBounds { ref range, haystack_len } => {
                write!(
                    f,
                    "excluded range {}..{} is out of bounds for haystack \
                     of length {}",
                    range.start, range.end, haystack_len,
                )
            }
            ExcludedRangesError::NotCharBoundary { offset } => write!(
                f,
                "excluded range bound at byte offset {} does not fall on \
                 a UTF-8 codepoint boundary",
                offset,
            ),
        }
    }
}

#[cfg(feature = "std")]
impl std::error::Error for ExcludedRangesError {}

/// Returns the byte ranges of all non-overlapping matches of a regex in a
/// haystack, skipping any match that intersects one of the given excluded
/// byte ranges.
///
/// This is a convenience routine built on top of SkipRanges. The regex is
/// run against the whole haystack (so start-of-text, end-of-text and
/// word-boundary assertions keep their meaning relative to the entire
/// haystack), the excluded ranges are validated and merged, and only
/// matches that do not intersect any excluded range are returned.
///
/// # Errors
///
/// Returns an ExcludedRangesError if any excluded range is reversed, out of
/// bounds, or has a bound that is not on a UTF-8 codepoint boundary.
pub fn find_all(
    re: &Regex,
    haystack: &str,
    excluded: &[Range<usize>],
) -> Result<Vec<Range<usize>>, ExcludedRangesError> {
    Ok(SkipRanges::new(re, haystack, excluded)?
        .map(|m| m.start()..m.end())
        .collect())
}

/// An iterator over all non-overlapping matches of a regex in a haystack
/// that do not intersect any of a set of excluded byte ranges.
///
/// The iterator yields values of type Match. It is created by
/// SkipRanges::new, which validates and merges the excluded ranges.
///
/// The regex is always executed against the entire original haystack, so
/// look-around assertions like start-of-text, end-of-text and word
/// boundaries are evaluated with respect to the whole haystack, exactly as
/// if Regex::find_iter had been used. A match is skipped when its byte
/// range intersects an excluded range. A zero-width match is skipped only
/// when its position lies strictly inside an excluded range.
///
/// # Time complexity
///
/// Iteration has worst case O(m * n^2) time complexity, just like
/// Regex::find_iter. Validating and merging the excluded ranges takes
/// O(k log k) time for k ranges.
#[derive(Clone, Debug)]
pub struct SkipRanges<'r, 'h> {
    re: &'r Regex,
    haystack: &'h str,
    /// Sorted, non-overlapping, non-adjacent excluded ranges.
    excluded: Vec<Range<usize>>,
    /// The offset at which the next search starts.
    at: usize,
    /// Whether iteration has terminated.
    done: bool,
}

impl<'r, 'h> SkipRanges<'r, 'h> {
    /// Creates a new iterator that yields the matches of a regex in a
    /// haystack that do not intersect any of the excluded byte ranges.
    ///
    /// The excluded ranges are interpreted as UTF-8 byte offsets in the
    /// half-open form start..end. They are validated and then sorted and
    /// merged, with overlapping and directly adjacent ranges coalesced.
    ///
    /// # Errors
    ///
    /// Returns an ExcludedRangesError if any excluded range is reversed
    /// (start greater than end), extends beyond the end of the haystack, or
    /// has a bound that does not fall on a UTF-8 codepoint boundary of the
    /// haystack.
    pub fn new(
        re: &'r Regex,
        haystack: &'h str,
        excluded: &[Range<usize>],
    ) -> Result<SkipRanges<'r, 'h>, ExcludedRangesError> {
        let merged = validate_and_merge(haystack, excluded)?;
        Ok(SkipRanges { re, haystack, excluded: merged, at: 0, done: false })
    }

    /// Returns the merged excluded ranges used by this iterator.
    ///
    /// The ranges are sorted by start offset, and all overlapping and
    /// directly adjacent ranges have been coalesced.
    pub fn excluded_ranges(&self) -> &[Range<usize>] {
        &self.excluded
    }
}

impl<'r, 'h> Iterator for SkipRanges<'r, 'h> {
    type Item = Match<'h>;

    fn next(&mut self) -> Option<Match<'h>> {
        if self.done {
            return None;
        }
        loop {
            if self.at > self.haystack.len() {
                self.done = true;
                return None;
            }
            let m = match self.re.find_at(self.haystack, self.at) {
                None => {
                    self.done = true;
                    return None;
                }
                Some(m) => m,
            };
            if m.start() == m.end() {
                // A zero-width match is excluded only when its position is
                // strictly inside an excluded range. A zero-width match on
                // the boundary of an excluded range is still reported.
                let pos = m.start();
                let inside =
                    self.excluded.iter().any(|r| r.start < pos && pos < r.end);
                // Advance to the next UTF-8 codepoint boundary. Adding a
                // single byte would land in the middle of a multi-byte
                // codepoint and make the next find_at call invalid. At the
                // end of the haystack we move one past it to terminate.
                self.at = match self.haystack[pos..].chars().next() {
                    Some(c) => pos + c.len_utf8(),
                    None => pos + 1,
                };
                if !inside {
                    return Some(m);
                }
                continue;
            }
            if let Some(r) = self.excluded.iter().find(|r| intersects(&m, r)) {
                // The match overlaps an excluded range. Skip it and resume
                // the search at the end of that excluded range. Because the
                // excluded ranges are merged, no match can start strictly
                // inside an excluded range without also overlapping it, so
                // resuming at its end never re-finds the same match.
                self.at = r.end;
                continue;
            }
            self.at = m.end();
            return Some(m);
        }
    }
}

impl<'r, 'h> core::iter::FusedIterator for SkipRanges<'r, 'h> {}

/// Returns true when the byte range of the match intersects the excluded
/// range. Only used for non-empty matches.
fn intersects(m: &Match<'_>, r: &Range<usize>) -> bool {
    m.start() < r.end && r.start < m.end()
}

/// Validates the excluded ranges against the haystack and returns them
/// sorted and merged, with overlapping and directly adjacent ranges
/// coalesced.
fn validate_and_merge(
    haystack: &str,
    excluded: &[Range<usize>],
) -> Result<Vec<Range<usize>>, ExcludedRangesError> {
    for r in excluded {
        if r.start > r.end {
            return Err(ExcludedRangesError::Reversed(r.clone()));
        }
        if r.end > haystack.len() {
            return Err(ExcludedRangesError::OutOfBounds {
                range: r.clone(),
                haystack_len: haystack.len(),
            });
        }
        // str::get returns Some for a byte offset only when it is in bounds
        // and on a UTF-8 codepoint boundary.
        if haystack.get(r.start..r.start).is_none()
            || haystack.get(r.end..r.end).is_none()
        {
            let offset = if haystack.get(r.start..r.start).is_none() {
                r.start
            } else {
                r.end
            };
            return Err(ExcludedRangesError::NotCharBoundary { offset });
        }
    }
    let mut sorted: Vec<Range<usize>> = excluded.to_vec();
    sorted.sort_by_key(|r| (r.start, r.end));
    let mut merged: Vec<Range<usize>> = Vec::with_capacity(sorted.len());
    for r in sorted {
        if r.start == r.end {
            // Empty ranges exclude nothing.
            continue;
        }
        match merged.last_mut() {
            Some(last) if r.start <= last.end => {
                last.end = last.end.max(r.end);
            }
            _ => merged.push(r),
        }
    }
    Ok(merged)
}
