/*!
The `&str` variant of the log-line [`Selector`].

All match offsets are byte offsets, exactly like searches with the top-level
[`Regex`], and therefore always fall on UTF-8 boundaries.
*/

use alloc::string::String;
use alloc::vec::Vec;

use crate::{Captures, Error, Match, Regex};

/// Select one regex rule to classify a `&str` haystack.
///
/// A `Selector` holds several independently compiled regexes (its *rules*).
/// Unlike a [`RegexSet`](crate::RegexSet), which reports *all* matching
/// patterns, [`Selector::classify`] reports exactly one rule: the rule whose
/// leftmost match starts at the earliest byte offset in the haystack. Ties
/// (two rules whose leftmost matches start at the same offset) are broken in
/// favor of the rule that was declared first.
///
/// Rules are never joined into one big alternation. Consequently, each rule
/// keeps its own leftmost-first semantics. For example, a rule written as
/// `a|ab` still prefers `a`, and the returned [`Captures`] are exactly those
/// produced by running the winning rule alone.
///
/// An empty match (zero-width hit) is treated exactly like any other match
/// and participates in selection. If no rule matches, classification returns
/// `None`.
///
/// # Example
///
/// ```
/// use regex::selector::Selector;
///
/// let sel = Selector::new(["z", "ab"]).unwrap();
/// let picked = sel.classify("xxabz").unwrap();
/// assert_eq!(picked.rule(), 1);
/// assert_eq!(picked.matched(), "ab");
/// ```
#[derive(Clone, Debug)]
pub struct Selector {
    rules: Vec<Regex>,
    patterns: Vec<String>,
}

impl Selector {
    /// Compiles a selector from the given ordered rules.
    ///
    /// Rules are kept in declaration order: when two rules match at the same
    /// earliest byte offset, the smaller index wins.
    ///
    /// An error is returned if any of the patterns is not a valid regex.
    pub fn new<I, S>(rules: I) -> Result<Selector, Error>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let mut patterns = Vec::new();
        let mut compiled = Vec::new();
        for pat in rules {
            let re = Regex::new(pat.as_ref())?;
            patterns.push(String::from(pat.as_ref()));
            compiled.push(re);
        }
        Ok(Selector { rules: compiled, patterns })
    }

    /// Returns an empty selector that never matches.
    pub fn empty() -> Selector {
        Selector { rules: Vec::new(), patterns: Vec::new() }
    }

    /// Returns the number of rules in this selector.
    pub fn len(&self) -> usize {
        self.rules.len()
    }

    /// Returns true if this selector has no rules.
    pub fn is_empty(&self) -> bool {
        self.rules.is_empty()
    }

    /// Returns the original pattern source of the rule at `index`.
    ///
    /// # Panics
    ///
    /// Panics if `index >= self.len()`.
    pub fn pattern(&self, index: usize) -> &str {
        self.patterns[index].as_str()
    }

    /// Returns a shared reference to the independently compiled rule at
    /// `index`.
    ///
    /// # Panics
    ///
    /// Panics if `index >= self.len()`.
    pub fn rule(&self, index: usize) -> &Regex {
        &self.rules[index]
    }

    /// Classify a haystack and return the winning rule and its captures.
    ///
    /// Selection proceeds in two phases:
    ///
    /// 1. Every rule is searched independently. The rule whose leftmost match
    ///    starts at the smallest byte offset wins; ties go to the rule
    ///    declared first. Because rules are iterated in declaration order and
    ///    a strictly smaller offset is required to replace the current winner,
    ///    a rule that registered first can never hide a match at an earlier
    ///    position, while equal-position matches stay with the earlier
    ///    declaration.
    /// 2. The winning rule alone is run again for its captures. The captures
    ///    are therefore identical to calling
    ///    `self.rule(i).captures(haystack)`, preserving the rule's native
    ///    (e.g. leftmost-first) semantics.
    ///
    /// Zero-width (empty) matches take part in phase 1 just like non-empty
    /// matches. If nothing matches, `None` is returned.
    pub fn classify<'h>(&self, haystack: &'h str) -> Option<Selection<'h>> {
        let mut winner: Option<(usize, usize)> = None;
        for (i, re) in self.rules.iter().enumerate() {
            if let Some(m) = re.find(haystack) {
                match winner {
                    // A strictly earlier starting point wins, no matter which
                    // rule produced it. Equal positions keep the rule that
                    // was declared earlier.
                    Some((_, best)) if m.start() >= best => {}
                    _ => winner = Some((i, m.start())),
                }
            }
        }
        let (index, start) = winner?;
        // The rule was already known to match at `start`, so these captures
        // always exist and describe that same match.
        let caps = self.rules[index].captures(haystack)?;
        Some(Selection { index, start, caps })
    }
}

impl Default for Selector {
    fn default() -> Selector {
        Selector::empty()
    }
}

impl core::fmt::Display for Selector {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "Selector(")?;
        for (i, pat) in self.patterns.iter().enumerate() {
            if i > 0 {
                write!(f, ", ")?;
            }
            write!(f, "{:?}", pat)?;
        }
        write!(f, ")")
    }
}

/// The result of classifying a `&str` haystack: the winning rule and the
/// captures produced by that exact rule.
#[derive(Debug)]
pub struct Selection<'h> {
    index: usize,
    start: usize,
    caps: Captures<'h>,
}

impl<'h> Selection<'h> {
    /// Returns the index of the winning rule, in declaration order.
    pub fn rule(&self) -> usize {
        self.index
    }

    /// Returns the byte offset at which the winning rule's match starts.
    pub fn start(&self) -> usize {
        self.start
    }

    /// Returns the byte offset at which the winning rule's match ends.
    pub fn end(&self) -> usize {
        self.caps.get(0).map(|m| m.end()).unwrap_or(self.start)
    }

    /// Returns the full byte range spanned by the winning match.
    pub fn range(&self) -> core::ops::Range<usize> {
        self.start()..self.end()
    }

    /// Returns the winning [`Captures`] exactly as produced by running the
    /// winning rule on the haystack. Group 0 is the overall match; other
    /// groups correspond to that rule's capture groups.
    pub fn captures(&self) -> &Captures<'h> {
        &self.caps
    }

    /// Returns the overall matched substring (capture group 0).
    pub fn matched(&self) -> &'h str {
        self.caps.get(0).map(|m| m.as_str()).unwrap_or("")
    }

    /// Returns true when the winning match is zero-width, i.e. empty.
    pub fn is_empty(&self) -> bool {
        self.start() == self.end()
    }

    /// Convenience accessor for capture group `i` (where `0` is the overall
    /// match) of the winning rule.
    pub fn get(&self, i: usize) -> Option<Match<'h>> {
        self.caps.get(i)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::{format, vec};

    #[test]
    fn earliest_start_wins_over_declaration_order() {
        // Rules "z" and "ab": in "xxabz", "ab" starts at 2 and "z" at 4,
        // so the later-declared rule must win.
        let sel = Selector::new(["z", "ab"]).unwrap();
        let pick = sel.classify("xxabz").unwrap();
        assert_eq!(pick.rule(), 1);
        assert_eq!(pick.start(), 2);
        assert_eq!(pick.matched(), "ab");
        assert_eq!(pick.range(), 2..4);
    }

    #[test]
    fn equal_start_keeps_earlier_declaration() {
        // Both rules can match at position 2; the first declaration wins.
        let sel = Selector::new(["z", "zz"]).unwrap();
        let pick = sel.classify("xxzz").unwrap();
        assert_eq!(pick.rule(), 0);
        assert_eq!(pick.start(), 2);
        assert_eq!(pick.matched(), "z");
    }

    #[test]
    fn rule_keeps_its_own_leftmost_first_alternation() {
        // "a|ab" alone prefers "a"; the selector must not change that.
        let sel = Selector::new([r"a|ab"]).unwrap();
        let pick = sel.classify("ab").unwrap();
        assert_eq!(pick.rule(), 0);
        assert_eq!(pick.matched(), "a");
    }

    #[test]
    fn earlier_registered_rule_cannot_hide_earlier_start() {
        // Register a rule that matches late first; another rule matches much
        // earlier and must still win.
        let sel = Selector::new([r"zzz$", r"ab"]).unwrap();
        let pick = sel.classify("xxabzzz").unwrap();
        assert_eq!(pick.rule(), 1);
        assert_eq!(pick.start(), 2);
    }

    #[test]
    fn no_match_returns_none() {
        let sel = Selector::new(["z", "ab"]).unwrap();
        assert!(sel.classify("xx").is_none());
        // An empty selector never matches.
        assert!(Selector::empty().classify("anything").is_none());
    }

    #[test]
    fn zero_width_match_participates_in_selection() {
        // Rule 0 can match zero-width at the start ("^"); rule 1 matches "ab"
        // at position 2. The empty match at position 0 is earlier and wins.
        let sel = Selector::new([r"^", "ab"]).unwrap();
        let pick = sel.classify("xxab").unwrap();
        assert_eq!(pick.rule(), 0);
        assert_eq!(pick.start(), 0);
        assert_eq!(pick.end(), 0);
        assert!(pick.is_empty());
        assert_eq!(pick.matched(), "");
    }

    #[test]
    fn zero_width_tie_keeps_earlier_rule() {
        // In " x", the zero-width word boundary "\b" matches at position 1
        // (right before 'x'), which is exactly where "x" starts. The earlier
        // declared zero-width rule wins the tie.
        let sel = Selector::new([r"\b", "x"]).unwrap();
        let pick = sel.classify(" x").unwrap();
        assert_eq!(pick.rule(), 0);
        assert_eq!(pick.start(), 1);
        assert!(pick.is_empty());

        // Reversing declaration order lets "x" win the same tie.
        let sel = Selector::new(["x", r"\b"]).unwrap();
        let pick = sel.classify(" x").unwrap();
        assert_eq!(pick.rule(), 0);
        assert_eq!(pick.matched(), "x");
    }

    #[test]
    fn captures_are_the_winning_rules_original_captures() {
        let sel =
            Selector::new([r"z(?P<after>.*)", r"(?P<letters>a)(b)"]).unwrap();
        let pick = sel.classify("xxabz").unwrap();
        assert_eq!(pick.rule(), 1);
        assert_eq!(pick.matched(), "ab");
        assert_eq!(pick.get(1).unwrap().as_str(), "a");
        assert_eq!(pick.get(2).unwrap().as_str(), "b");
        assert_eq!(pick.captures().name("letters").unwrap().as_str(), "a");
    }

    #[test]
    fn utf8_boundaries_are_respected() {
        // "e-acute" is two UTF-8 bytes; offsets reported must be char
        // boundaries.
        let e_acute = "\u{e9}";
        let hay = format!("{}{}abz", e_acute, e_acute);
        let sel = Selector::new(["z", "ab"]).unwrap();
        let pick = sel.classify(&hay).unwrap();
        assert_eq!(pick.rule(), 1);
        assert_eq!(pick.start(), 4);
        assert_eq!(pick.matched(), "ab");
        // The reported offset splits the haystack along a UTF-8 boundary.
        assert!(hay.is_char_boundary(pick.start()));
        assert_eq!(&hay[..pick.start()], "\u{e9}\u{e9}");
        // Display keeps the original pattern sources.
        assert_eq!(format!("{}", sel), r#"Selector("z", "ab")"#);
    }

    #[test]
    fn metadata_and_patterns() {
        let sel = Selector::new(["z", "ab"]).unwrap();
        assert_eq!(sel.len(), 2);
        assert!(!sel.is_empty());
        assert_eq!(sel.pattern(0), "z");
        assert_eq!(sel.pattern(1), "ab");
        assert!(Selector::default().is_empty());
    }

    #[test]
    fn invalid_pattern_errors() {
        assert!(Selector::new(["("]).is_err());
    }

    #[test]
    fn accepts_owned_pattern_sources() {
        let pats: Vec<String> = vec![String::from("z"), String::from("ab")];
        let sel = Selector::new(pats).unwrap();
        assert_eq!(sel.classify("xxabz").unwrap().rule(), 1);
    }
}
