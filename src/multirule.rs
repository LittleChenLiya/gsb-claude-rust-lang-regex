/*!
An iterator that reports the hits of multiple rules over one haystack.

This module provides [MultiRule], a collection of regex rules where each
rule carries a caller supplied unique ID. Unlike [RegexSet](crate::RegexSet),
which only reports which patterns matched somewhere in a haystack, a
[MultiRule] enumerates every hit of every rule, including hits that overlap
with the hits of other rules (or with other hits of the same rule).

For each rule, the hits are computed by conceptually restarting a
leftmost-first search at every position at which a match can begin. That is,
for every legal start position, the hit reported is the same match that
[Regex::find_at](crate::Regex::find_at) would select at that position. The
hits of all rules are then merged and yielded in order of increasing start
offset, then increasing end offset, and finally by the declaration order of
the rules.
*/

use alloc::{collections::BinaryHeap, string::String, vec, vec::Vec};
use core::{cmp::Reverse, ops::Range};

use crate::{Captures, Error, Regex};

/// A set of regex rules that are all matched against the same haystack.
///
/// Each rule pairs a caller supplied ID with a regex pattern. IDs are
/// expected to be unique so that every hit can be attributed to exactly one
/// rule, but this is not enforced: rules may share IDs or even patterns, in
/// which case each rule still reports its own hits.
///
/// # Example
///
/// This shows the motivating example: the rules 'aba' and 'ba' are matched
/// against 'ababa'. Every hit is reported, including the ones that overlap
/// with hits of the other rule:
///
/// ```
/// use regex::MultiRule;
///
/// let rules = MultiRule::new([("R1", "aba"), ("R2", "ba")]).unwrap();
/// let hits: Vec<(usize, usize, &str)> = rules
///     .find_iter("ababa")
///     .map(|m| (m.start(), m.end(), *m.id()))
///     .collect();
/// assert_eq!(hits, vec![
///     (0, 3, "R1"), // aba
///     (1, 3, "R2"), //  ba
///     (2, 5, "R1"), //   aba
///     (3, 5, "R2"), //    ba
/// ]);
/// ```
#[derive(Clone, Debug)]
pub struct MultiRule<I> {
    rules: Vec<Rule<I>>,
}

#[derive(Clone, Debug)]
struct Rule<I> {
    id: I,
    re: Regex,
}

impl<I> MultiRule<I> {
    /// Compiles a set of rules from (id, pattern) pairs.
    ///
    /// Every pattern is compiled with the default configuration, exactly as
    /// if [Regex::new] had been called on it. If any pattern fails to
    /// compile, an error is returned.
    ///
    /// The order in which rules are given is their declaration order. It is
    /// used as the final tie-breaker when two hits share the same start and
    /// end offsets.
    pub fn new<R, P>(rules: R) -> Result<MultiRule<I>, Error>
    where
        R: IntoIterator<Item = (I, P)>,
        P: AsRef<str>,
    {
        let mut compiled = vec![];
        for (id, pattern) in rules {
            let re = Regex::new(pattern.as_ref())?;
            compiled.push(Rule { id, re });
        }
        Ok(MultiRule { rules: compiled })
    }

    /// Returns the number of rules in this set.
    pub fn len(&self) -> usize {
        self.rules.len()
    }

    /// Returns true if and only if this set has no rules.
    pub fn is_empty(&self) -> bool {
        self.rules.is_empty()
    }

    /// Returns an iterator over the hits of every rule in the haystack.
    ///
    /// For each rule, a hit is reported for every position at which a match
    /// of that rule can begin. When several matches of the same rule could
    /// begin at the same position, only the one selected by the usual
    /// leftmost-first semantics (the same one [Regex::find_at] would
    /// return) is reported. Hits may overlap arbitrarily, both within one
    /// rule and across different rules.
    ///
    /// The hits of all rules are interleaved into a single sequence ordered
    /// by start offset, then end offset, then rule declaration order.
    ///
    /// Note that finding every hit of every rule requires restarting a
    /// search at (roughly) every position of the haystack for every rule,
    /// so this is necessarily slower than [Regex::find_iter].
    ///
    /// [Regex::find_at]: crate::Regex::find_at
    /// [Regex::find_iter]: crate::Regex::find_iter
    pub fn find_iter<'m, 'h>(
        &'m self,
        haystack: &'h str,
    ) -> MultiRuleMatches<'m, 'h, I> {
        let mut it = MultiRuleMatches {
            set: self,
            haystack,
            next_at: vec![0; self.rules.len()],
            heap: BinaryHeap::new(),
        };
        for rule in 0..self.rules.len() {
            it.advance(rule);
        }
        it
    }

    /// Returns the captures of the rule that produced the given hit.
    ///
    /// The haystack must be the same one that the hit was found in. This
    /// re-runs the search of the corresponding rule at the start position
    /// of the hit, which is guaranteed to select the same match again.
    ///
    /// # Example
    ///
    /// ```
    /// use regex::MultiRule;
    ///
    /// let rules = MultiRule::new([("pair", r"(\d+)-(\d+)")]).unwrap();
    /// let hits: Vec<_> = rules.find_iter("1-22 333-4").collect();
    /// let caps = rules.captures(&hits[1], "1-22 333-4").unwrap();
    /// assert_eq!(&caps[1], "333");
    /// assert_eq!(&caps[2], "4");
    /// ```
    pub fn captures<'h>(
        &self,
        hit: &MultiRuleMatch<I>,
        haystack: &'h str,
    ) -> Option<Captures<'h>> {
        let rule = self.rules.get(hit.rule)?;
        rule.re.captures_at(haystack, hit.start)
    }

    /// Creates a reader that accepts the haystack in segments.
    ///
    /// Segments are concatenated in the order they are pushed, and the hits
    /// reported by [MultiRuleReader::finish] are exactly the ones that
    /// [MultiRule::find_iter] would report for the concatenation of all
    /// segments read at once.
    ///
    /// # Example
    ///
    /// ```
    /// use regex::MultiRule;
    ///
    /// let rules = MultiRule::new([("R1", "aba"), ("R2", "ba")]).unwrap();
    /// let mut reader = rules.reader();
    /// reader.push("ab");
    /// reader.push("ab");
    /// reader.push("a");
    /// let segmented = reader.finish();
    /// let oneshot: Vec<_> = rules.find_iter("ababa").collect();
    /// assert_eq!(segmented, oneshot);
    /// ```
    pub fn reader(&self) -> MultiRuleReader<'_, I> {
        MultiRuleReader { set: self, buf: String::new() }
    }
}

/// A single hit of one rule of a [MultiRule] in a haystack.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MultiRuleMatch<I> {
    id: I,
    rule: usize,
    start: usize,
    end: usize,
}

impl<I> MultiRuleMatch<I> {
    /// Returns the ID of the rule that produced this hit.
    pub fn id(&self) -> &I {
        &self.id
    }

    /// Returns the declaration order index of the rule that produced this
    /// hit. That is, the position of the rule in the iterator given to
    /// [MultiRule::new].
    pub fn rule_index(&self) -> usize {
        self.rule
    }

    /// Returns the start byte offset of this hit in the haystack.
    pub fn start(&self) -> usize {
        self.start
    }

    /// Returns the end byte offset of this hit in the haystack.
    pub fn end(&self) -> usize {
        self.end
    }

    /// Returns the byte range of this hit in the haystack.
    pub fn range(&self) -> Range<usize> {
        self.start..self.end
    }

    /// Returns the matched text by slicing the haystack with this hit.
    ///
    /// The haystack must be the same one that this hit was found in.
    ///
    /// # Panics
    ///
    /// Panics if the range of this hit is out of bounds for the haystack.
    pub fn as_str<'h>(&self, haystack: &'h str) -> &'h str {
        &haystack[self.range()]
    }
}

/// An iterator over the hits of every rule of a [MultiRule] in a haystack.
///
/// This iterator is created by [MultiRule::find_iter]. See its
/// documentation for the precise semantics and the order in which hits are
/// yielded.
#[derive(Debug)]
pub struct MultiRuleMatches<'m, 'h, I> {
    set: &'m MultiRule<I>,
    haystack: &'h str,
    // The position at which the search of each rule resumes. Always a
    // char boundary, or one past the end of the haystack once a rule is
    // exhausted.
    next_at: Vec<usize>,
    // Pending hits as (start, end, rule) triples. The Reverse turns the
    // binary heap into a min-heap ordered by start offset, then end
    // offset, then rule declaration order.
    heap: BinaryHeap<Reverse<(usize, usize, usize)>>,
}

impl<'m, 'h, I> MultiRuleMatches<'m, 'h, I> {
    // Finds the next hit of the given rule and pushes it onto the heap.
    fn advance(&mut self, rule: usize) {
        let at = self.next_at[rule];
        if at > self.haystack.len() {
            return;
        }
        let m = match self.set.rules[rule].re.find_at(self.haystack, at) {
            None => {
                self.next_at[rule] = self.haystack.len() + 1;
                return;
            }
            Some(m) => m,
        };
        // The next hit of this rule may begin at any position after the
        // start of this one. Skip ahead to the next char boundary so that
        // the resume position is always a valid boundary, even when the
        // current hit starts at a multi-byte UTF-8 encoded character.
        let mut next = m.start() + 1;
        while next < self.haystack.len()
            && !self.haystack.is_char_boundary(next)
        {
            next += 1;
        }
        self.next_at[rule] = next;
        self.heap.push(Reverse((m.start(), m.end(), rule)));
    }
}

impl<'m, 'h, I: Clone> Iterator for MultiRuleMatches<'m, 'h, I> {
    type Item = MultiRuleMatch<I>;

    fn next(&mut self) -> Option<MultiRuleMatch<I>> {
        let Reverse((start, end, rule)) = self.heap.pop()?;
        self.advance(rule);
        let id = self.set.rules[rule].id.clone();
        Some(MultiRuleMatch { id, rule, start, end })
    }
}

impl<'m, 'h, I: Clone> core::iter::FusedIterator
    for MultiRuleMatches<'m, 'h, I>
{
}

/// A reader that feeds a haystack to a [MultiRule] in segments.
///
/// This is created by [MultiRule::reader]. Segments are buffered and
/// concatenated, and [MultiRuleReader::finish] reports the hits of the
/// concatenation. Since hits may span arbitrary distances, they cannot in
/// general be determined before the final segment has been seen.
#[derive(Clone, Debug)]
pub struct MultiRuleReader<'m, I> {
    set: &'m MultiRule<I>,
    buf: String,
}

impl<'m, I> MultiRuleReader<'m, I> {
    /// Appends a segment of the haystack.
    ///
    /// Segments are concatenated in the order they are pushed. Segments
    /// may be split at any char boundary.
    pub fn push(&mut self, segment: &str) {
        self.buf.push_str(segment);
    }

    /// Returns the haystack that has been accumulated so far.
    pub fn haystack(&self) -> &str {
        &self.buf
    }

    /// Returns the hits of every rule over the concatenation of all
    /// segments pushed so far.
    ///
    /// The result is identical to collecting [MultiRule::find_iter] over
    /// the concatenated haystack read at once.
    pub fn finish(self) -> Vec<MultiRuleMatch<I>>
    where
        I: Clone,
    {
        self.set.find_iter(&self.buf).collect()
    }
}
