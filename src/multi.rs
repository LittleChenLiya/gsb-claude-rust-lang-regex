/*!
Multi-rule matching with overlapping results.

This module provides a "multi rule" matcher built on top of ordinary
[`Regex`] values. Unlike [`crate::RegexSet`], which only reports *which*
patterns match, the matcher in this module enumerates, for every rule, the
match selected by the regex's normal leftmost priority at every legal start
position in the same haystack. Matches from different rules may overlap, and
are emitted together in a single order: by start offset, then end offset,
then the order in which the rules were declared.

The typical entry point is [`MultiMatcher`]. For example, given the rules
`aba` and `ba` and the haystack `ababa`, the matcher reports all of the
overlapping occurrences at once:

```
use regex::multi::{MultiMatcher, Rule};

let matcher = MultiMatcher::from_rules([
    Rule::new("a", "aba").unwrap(),
    Rule::new("b", "ba").unwrap(),
]).unwrap();
let ranges: Vec<std::ops::Range<usize>> =
    matcher.find_iter("ababa").map(|m| m.range()).collect();
assert_eq!(ranges, vec![0..3, 1..3, 2..5, 3..5]);
```

The first and third ranges correspond to the two `aba` matches, while the
second and last ranges correspond to the two `ba` matches; they overlap.
*/

use alloc::{
    string::{String, ToString},
    vec::Vec,
};
use core::str::Utf8Error;

use crate::{CaptureLocations, Captures, Error, Match, Regex};

/// A single rule participating in a [`MultiMatcher`].
///
/// A rule pairs a unique identifier with a compiled [`Regex`]. Every rule in
/// a matcher must have a distinct identifier; attempting to build a matcher
/// with duplicate identifiers returns [`Error::DuplicateRuleId`].
///
/// The identifier is reported on every [`MultiMatch`] produced by the rule.
#[derive(Clone, Debug)]
pub struct Rule<'a> {
    id: &'a str,
    regex: Regex,
}

impl<'a> Rule<'a> {
    /// Compiles a new rule with the given identifier and pattern.
    ///
    /// # Errors
    ///
    /// This returns an error if the pattern is invalid, exactly like
    /// [`Regex::new`].
    pub fn new(id: &'a str, pattern: &str) -> Result<Rule<'a>, Error> {
        Ok(Rule { id, regex: Regex::new(pattern)? })
    }

    /// Wraps an already compiled [`Regex`] as a rule.
    pub fn from_regex(id: &'a str, regex: Regex) -> Rule<'a> {
        Rule { id, regex }
    }

    /// Returns the unique identifier of this rule.
    pub fn id(&self) -> &'a str {
        self.id
    }

    /// Returns the regular expression associated with this rule.
    pub fn regex(&self) -> &Regex {
        &self.regex
    }
}

/// A matcher that runs multiple rules against the same haystack.
///
/// A `MultiMatcher` is built incrementally with [`MultiMatcher::add`] or in
/// one shot with [`MultiMatcher::from_rules`], and searched with
/// [`MultiMatcher::find_iter`]. Each rule is independent: every rule is
/// searched against the full haystack, and matches that overlap with matches
/// from other rules (or from the same rule at later start positions) are all
/// reported.
#[derive(Clone, Debug)]
pub struct MultiMatcher<'a> {
    rules: Vec<Rule<'a>>,
}

impl<'a> MultiMatcher<'a> {
    /// Creates a new empty matcher.
    pub fn new() -> MultiMatcher<'a> {
        MultiMatcher { rules: Vec::new() }
    }

    /// Adds a rule to the matcher.
    ///
    /// The rule's identifier must be distinct from every identifier already
    /// added.
    ///
    /// # Errors
    ///
    /// This returns [`Error::DuplicateRuleId`] if a rule with the same
    /// identifier has already been added.
    pub fn add(&mut self, rule: Rule<'a>) -> Result<(), Error> {
        if self.rules.iter().any(|existing| existing.id == rule.id) {
            return Err(Error::DuplicateRuleId(rule.id.to_string()));
        }
        self.rules.push(rule);
        Ok(())
    }

    /// Compiles a rule directly and adds it to the matcher.
    ///
    /// This is a convenience for calling [`Rule::new`] followed by
    /// [`MultiMatcher::add`].
    ///
    /// # Errors
    ///
    /// This returns an error if the pattern is invalid or if the identifier
    /// is already in use.
    pub fn add_pattern(
        &mut self,
        id: &'a str,
        pattern: &str,
    ) -> Result<(), Error> {
        self.add(Rule::new(id, pattern)?)
    }

    /// Builds a matcher from an iterator of rules.
    ///
    /// # Errors
    ///
    /// This returns [`Error::DuplicateRuleId`] if two rules share an
    /// identifier.
    pub fn from_rules<I>(rules: I) -> Result<MultiMatcher<'a>, Error>
    where
        I: IntoIterator<Item = Rule<'a>>,
    {
        let mut matcher = MultiMatcher::new();
        for rule in rules {
            matcher.add(rule)?;
        }
        Ok(matcher)
    }

    /// Returns the rules in this matcher, in declaration order.
    pub fn rules(&self) -> &[Rule<'a>] {
        &self.rules
    }

    /// Returns the rule with the given identifier, if one exists.
    pub fn rule(&self, id: &str) -> Option<&Rule<'a>> {
        self.rules.iter().find(|rule| rule.id == id)
    }

    /// Returns an iterator over every match of every rule in `haystack`.
    ///
    /// For each rule, the iterator yields the match chosen by the regex's
    /// normal leftmost priority at every legal start position where the rule
    /// can match. In particular, matches are *not* skipped because they
    /// overlap with a previously yielded match. The combined results are
    /// ordered by start offset, then by end offset, and finally by the
    /// declaration order of the rules.
    pub fn find_iter<'r, 'h>(
        &'r self,
        haystack: &'h str,
    ) -> MultiMatches<'r, 'h> {
        let mut matches = Vec::new();
        for (order, rule) in self.rules.iter().enumerate() {
            let mut probe = 0;
            loop {
                if let Some(mat) = rule.regex().find_at(haystack, probe) {
                    let mat_start = mat.start();
                    // The match belongs to the legal start position where
                    // it actually begins. It is recorded only when the
                    // enumeration reaches that start; a match found from a
                    // smaller probe is the same search result and must not
                    // be repeated at intermediate starts.
                    if mat_start == probe {
                        matches.push(StoredMatch {
                            rule,
                            order,
                            start: mat_start,
                            end: mat.end(),
                        });
                    }
                }
                probe = next_char_boundary(haystack, probe);
                if probe > haystack.len() {
                    break;
                }
            }
        }
        // Stable sort by start, then end. Because enumeration appends rules
        // in declaration order, ties on both offsets already occur in that
        // order, and stability preserves it. The order field makes that
        // third key explicit and robust to future changes in enumeration.
        matches.sort_by_key(|mat| (mat.start, mat.end, mat.order));
        MultiMatches { offsets: OffsetMatches { matches, next: 0 }, haystack }
    }

    /// Returns a new incremental search that accumulates its haystack from
    /// pieces.
    ///
    /// See [`ChunkedSearch`] for how to feed chunks and retrieve the same
    /// results that [`MultiMatcher::find_iter`] would produce over the
    /// concatenation.
    pub fn chunked<'r>(&'r self) -> ChunkedSearch<'r> {
        ChunkedSearch { matcher: self, haystack: ChunkedHaystack::new() }
    }
}

impl<'a> Default for MultiMatcher<'a> {
    fn default() -> MultiMatcher<'a> {
        MultiMatcher::new()
    }
}

/// A single match reported by a [`MultiMatcher`].
///
/// In addition to the matched byte range (accessible through methods shared
/// with [`Match`], such as [`MultiMatch::range`] and
/// [`MultiMatch::as_str`]), a `MultiMatch` exposes the [`MultiMatch::id`] of
/// the rule that produced it.
#[derive(Copy, Clone, Debug)]
pub struct MultiMatch<'r, 'h> {
    rule: &'r Rule<'r>,
    haystack: &'h str,
    start: usize,
    end: usize,
}

impl<'r, 'h> MultiMatch<'r, 'h> {
    /// Returns the identifier of the rule that produced this match.
    pub fn id(&self) -> &'r str {
        self.rule.id
    }

    /// Returns the byte offset of the start of the match, inclusive.
    pub fn start(&self) -> usize {
        self.start
    }

    /// Returns the byte offset of the end of the match, exclusive.
    pub fn end(&self) -> usize {
        self.end
    }

    /// Returns the byte range of the match, as a half-open interval.
    pub fn range(&self) -> core::ops::Range<usize> {
        self.start..self.end
    }

    /// Returns the length of the match, in bytes.
    pub fn len(&self) -> usize {
        self.end - self.start
    }

    /// Returns true when the match has length zero.
    pub fn is_empty(&self) -> bool {
        self.start == self.end
    }

    /// Returns the substring that matched.
    pub fn as_str(&self) -> &'h str {
        &self.haystack[self.start..self.end]
    }

    /// Returns the [`Rule`] that produced this match.
    pub fn rule(&self) -> &'r Rule<'r> {
        self.rule
    }

    /// Returns this result as an ordinary [`Match`].
    pub fn as_match(&self) -> Match<'h> {
        Match::new(self.haystack, self.start, self.end)
    }

    /// Returns the capture groups for this match.
    ///
    /// This re-runs the rule that produced the match, anchored at the match's
    /// start offset, and returns the capture groups of that search. A regex
    /// search at a fixed start position is deterministic, so the overall
    /// match returned by [`Captures::get_match`] always has the same range as
    /// this `MultiMatch`.
    pub fn captures(&self) -> Captures<'h> {
        self.rule
            .regex
            .captures_at(self.haystack, self.start)
            .expect("a MultiMatch corresponds to an existing match")
    }

    /// Writes the capture groups for this match into `locations`.
    ///
    /// This is the zero-allocation version of [`MultiMatch::captures`]. The
    /// locations must have been created by this match's rule, for example
    /// with `multi_match.rule().regex().capture_locations()`.
    pub fn read_captures(
        &self,
        locations: &mut CaptureLocations,
    ) -> Match<'h> {
        self.rule
            .regex
            .captures_read_at(locations, self.haystack, self.start)
            .expect("a MultiMatch corresponds to an existing match")
    }
}

/// Returns the smallest UTF-8 boundary strictly after `start`, or
/// `haystack.len() + 1` when `start == haystack.len()`.
fn next_char_boundary(haystack: &str, start: usize) -> usize {
    if start == haystack.len() {
        return start + 1;
    }
    let mut next = start + 1;
    while !haystack.is_char_boundary(next) {
        next += 1;
    }
    next
}

/// An iterator over the combined, overlapping matches of all rules in a
/// [`MultiMatcher`].
///
/// This value is created by [`MultiMatcher::find_iter`].
#[derive(Debug)]
pub struct MultiMatches<'r, 'h> {
    offsets: OffsetMatches<'r>,
    haystack: &'h str,
}

impl<'r, 'h> MultiMatches<'r, 'h> {
    /// Discards the haystack borrow and returns the underlying offset
    /// iterator.
    pub(crate) fn into_offsets(self) -> OffsetMatches<'r> {
        self.offsets
    }
}

/// A match without a haystack reference: the rule plus its byte offsets.
#[derive(Copy, Clone, Debug)]
struct StoredMatch<'r> {
    rule: &'r Rule<'r>,
    order: usize,
    start: usize,
    end: usize,
}

impl<'r, 'h> Iterator for MultiMatches<'r, 'h> {
    type Item = MultiMatch<'r, 'h>;

    fn next(&mut self) -> Option<MultiMatch<'r, 'h>> {
        let stored = self.offsets.next()?;
        Some(MultiMatch {
            rule: stored.0,
            haystack: self.haystack,
            start: stored.1,
            end: stored.2,
        })
    }
}

/// An iterator over match offsets that does not borrow the haystack.
///
/// This lets a caller own both the haystack and the match results (as in
/// [`ChunkedMatches`]) without creating a self-referential type.
#[derive(Clone, Debug)]
pub(crate) struct OffsetMatches<'r> {
    matches: Vec<StoredMatch<'r>>,
    next: usize,
}

impl<'r> Iterator for OffsetMatches<'r> {
    // Rule reference, start offset, end offset.
    type Item = (&'r Rule<'r>, usize, usize);

    fn next(&mut self) -> Option<Self::Item> {
        let stored = *self.matches.get(self.next)?;
        self.next += 1;
        Some((stored.rule, stored.start, stored.end))
    }
}

/// Accumulates a haystack delivered in arbitrary pieces.
///
/// This is a convenience for callers that read their input incrementally
/// (for example, from a socket or a log file) but want to run the same
/// whole-haystack search that [`MultiMatcher::find_iter`] provides. Pieces
/// may be split anywhere, including in the middle of a UTF-8 encoded scalar
/// value: the incomplete trailing bytes are retained and joined with the
/// next piece.
///
/// Most callers should use [`MultiMatcher::chunked`] (which wraps this type
/// in a [`ChunkedSearch`]) rather than using it directly.
#[derive(Clone, Debug, Default)]
pub struct ChunkedHaystack {
    haystack: String,
    tail: alloc::vec::Vec<u8>,
}

impl ChunkedHaystack {
    /// Creates a new empty accumulator.
    pub fn new() -> ChunkedHaystack {
        ChunkedHaystack {
            haystack: String::new(),
            tail: alloc::vec::Vec::new(),
        }
    }

    /// Appends a piece of the haystack.
    ///
    /// The piece may end in the middle of a UTF-8 scalar value.
    ///
    /// # Errors
    ///
    /// This returns an error if the piece, combined with any trailing bytes
    /// retained from a previous piece, is not valid UTF-8 for any reason
    /// other than ending in the middle of a scalar value.
    pub fn extend(&mut self, chunk: &[u8]) -> Result<(), Utf8Error> {
        self.tail.extend_from_slice(chunk);
        let valid = match core::str::from_utf8(&self.tail) {
            Ok(text) => text.len(),
            Err(err) => {
                let valid_up_to = err.valid_up_to();
                if err.error_len().is_some() {
                    return Err(err);
                }
                // `error_len` is only `None` when the bytes end in the
                // middle of a UTF-8 scalar value; retain those trailing
                // bytes for the next piece and consume the valid prefix.
                valid_up_to
            }
        };
        self.haystack
            .push_str(core::str::from_utf8(&self.tail[..valid]).unwrap());
        // When everything was valid this clears the tail; otherwise it
        // retains just the incomplete trailing bytes.
        self.tail.drain(..valid);
        Ok(())
    }

    /// Appends a piece that is already valid UTF-8.
    pub fn extend_str(&mut self, chunk: &str) {
        // Valid text cannot complete a retained tail into anything but a
        // valid prefix, so this cannot fail.
        self.extend(chunk.as_bytes())
            .expect("extending with valid UTF-8 cannot fail")
    }

    /// Finalizes the haystack and returns it.
    ///
    /// After this call, no incomplete scalar may remain; every piece must
    /// have formed valid UTF-8 in aggregate.
    ///
    /// # Errors
    ///
    /// This returns an error if the accumulated input is not valid UTF-8.
    pub fn finish(mut self) -> Result<String, Utf8Error> {
        if !self.tail.is_empty() {
            core::str::from_utf8(&self.tail)?;
            self.haystack.push_str(core::str::from_utf8(&self.tail).unwrap());
        }
        Ok(self.haystack)
    }

    /// Returns the valid UTF-8 prefix accumulated so far.
    ///
    /// Bytes belonging to an incomplete trailing scalar are not included.
    pub fn as_str(&self) -> &str {
        &self.haystack
    }
}

/// An incremental multi-rule search over a haystack fed in pieces.
///
/// A `ChunkedSearch` pairs a [`MultiMatcher`] with a [`ChunkedHaystack`].
/// Chunks are accumulated with [`ChunkedSearch::extend`] (for bytes) or
/// [`ChunkedSearch::extend_str`] (for `&str`). When all pieces have been
/// fed, [`ChunkedSearch::finish`] returns a [`ChunkedMatches`] over exactly
/// the same matches that [`MultiMatcher::find_iter`] would return over the
/// concatenation of all pieces.
#[derive(Clone, Debug)]
pub struct ChunkedSearch<'r> {
    matcher: &'r MultiMatcher<'r>,
    haystack: ChunkedHaystack,
}

impl<'r> ChunkedSearch<'r> {
    /// Creates a new incremental search for the given matcher.
    pub fn new(matcher: &'r MultiMatcher<'r>) -> ChunkedSearch<'r> {
        matcher.chunked()
    }

    /// Appends a byte chunk, which may be split in the middle of a UTF-8
    /// scalar value.
    ///
    /// # Errors
    ///
    /// This returns the same error as [`ChunkedHaystack::extend`].
    pub fn extend(&mut self, chunk: &[u8]) -> Result<(), Utf8Error> {
        self.haystack.extend(chunk)
    }

    /// Appends a chunk that is already valid UTF-8.
    pub fn extend_str(&mut self, chunk: &str) {
        self.haystack.extend_str(chunk)
    }

    /// Returns the valid UTF-8 prefix accumulated so far.
    pub fn haystack(&self) -> &str {
        self.haystack.as_str()
    }

    /// Finalizes the input and returns the combined matches.
    ///
    /// # Errors
    ///
    /// This returns an error if the accumulated chunks are not valid UTF-8.
    pub fn finish(self) -> Result<ChunkedMatches<'r>, Utf8Error> {
        let haystack = self.haystack.finish()?;
        // Compute offsets first (they do not borrow the haystack), then move
        // the owned haystack in alongside them. This avoids a self
        // referential struct.
        let offsets = self.matcher.find_iter(&haystack).into_offsets();
        Ok(ChunkedMatches { haystack, offsets })
    }
}

/// The results of a [`ChunkedSearch`].
///
/// This owns the concatenated haystack, so it cannot implement the standard
/// [`Iterator`] trait (its items borrow from `self`). Use
/// [`ChunkedMatches::next_match`] instead, or
/// [`ChunkedMatches::collect_remaining`] to drain every result.
#[derive(Debug)]
pub struct ChunkedMatches<'r> {
    haystack: String,
    offsets: OffsetMatches<'r>,
}

impl<'r> ChunkedMatches<'r> {
    /// Advances to the next match, or returns `None` when exhausted.
    pub fn next_match(&mut self) -> Option<MultiMatch<'r, '_>> {
        let (rule, start, end) = self.offsets.next()?;
        Some(MultiMatch { rule, haystack: &self.haystack, start, end })
    }

    /// Returns the concatenated haystack that was searched.
    pub fn haystack(&self) -> &str {
        &self.haystack
    }

    /// Returns all remaining matches as `(rule id, byte range)` pairs.
    pub fn collect_remaining(
        &mut self,
    ) -> Vec<(&'r str, core::ops::Range<usize>)> {
        let mut collected = Vec::new();
        while let Some(mat) = self.next_match() {
            collected.push((mat.id(), mat.range()));
        }
        collected
    }
}
