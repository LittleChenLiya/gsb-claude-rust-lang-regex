/*!
The `&[u8]` variant of the log-line [`Selector`].

This mirrors [`super::Selector`] but searches `&[u8]` haystacks using
[`crate::bytes::Regex`]. Match offsets are byte offsets and, because
haystacks need not be valid UTF-8, rules can match through arbitrary bytes
when compiled in ASCII-compatible mode (e.g. `(?-u)\xFF+`). Nothing in this
variant ever re-decodes the haystack as a `str`; non-UTF-8 bytes are
preserved in the returned match and captures.
*/

use alloc::string::String;
use alloc::vec::Vec;

use crate::bytes::{Captures, Match, Regex};
use crate::Error;

/// Select one regex rule to classify a `&[u8]` haystack.
///
/// This is the byte-oriented twin of [`super::Selector`]. Each rule is an
/// independently compiled [`crate::bytes::Regex`], so rules keep their own
/// leftmost-first semantics and may match invalid UTF-8 bytes. Classification
/// chooses the rule whose leftmost match starts at the earliest byte offset;
/// ties go to the rule declared first; zero-width matches participate; and a
/// haystack with no matches yields `None`.
///
/// # Example
///
/// ```
/// use regex::selector::bytes::Selector;
///
/// let sel = Selector::new(["z", "ab"]).unwrap();
/// let picked = sel.classify(b"xxabz").unwrap();
/// assert_eq!(picked.rule(), 1);
/// assert_eq!(picked.matched(), b"ab");
/// ```
#[derive(Clone, Debug)]
pub struct Selector {
    rules: Vec<Regex>,
    patterns: Vec<String>,
}

impl Selector {
    /// Compiles a byte selector from the given ordered rules.
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

    /// Returns an empty byte selector that never matches.
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

    /// Classify a byte haystack and return the winning rule and its captures.
    ///
    /// The selection algorithm is identical to
    /// [`super::Selector::classify`]: compare each rule's leftmost match
    /// start (strictly earlier wins), then break ties by declaration order,
    /// then re-run the winning rule alone to obtain its original captures.
    /// The haystack is never required to be valid UTF-8, and matched bytes
    /// are returned unchanged.
    pub fn classify<'h>(&self, haystack: &'h [u8]) -> Option<Selection<'h>> {
        let mut winner: Option<(usize, usize)> = None;
        for (i, re) in self.rules.iter().enumerate() {
            if let Some(m) = re.find(haystack) {
                match winner {
                    Some((_, best)) if m.start() >= best => {}
                    _ => winner = Some((i, m.start())),
                }
            }
        }
        let (index, start) = winner?;
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
        write!(f, "bytes::Selector(")?;
        for (i, pat) in self.patterns.iter().enumerate() {
            if i > 0 {
                write!(f, ", ")?;
            }
            write!(f, "{:?}", pat)?;
        }
        write!(f, ")")
    }
}

/// The result of classifying a `&[u8]` haystack: the winning rule and the
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
    /// winning rule on the haystack. Group 0 is the overall match.
    pub fn captures(&self) -> &Captures<'h> {
        &self.caps
    }

    /// Returns the overall matched bytes (capture group 0). Non-UTF-8 bytes
    /// are preserved exactly as they occurred in the haystack.
    pub fn matched(&self) -> &'h [u8] {
        self.caps.get(0).map(|m| m.as_bytes()).unwrap_or(b"")
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
    use alloc::format;

    #[test]
    fn earliest_start_wins_over_declaration_order() {
        let sel = Selector::new(["z", "ab"]).unwrap();
        let pick = sel.classify(b"xxabz").unwrap();
        assert_eq!(pick.rule(), 1);
        assert_eq!(pick.start(), 2);
        assert_eq!(pick.matched(), b"ab");
    }

    #[test]
    fn equal_start_keeps_earlier_declaration() {
        let sel = Selector::new(["z", "zz"]).unwrap();
        let pick = sel.classify(b"xxzz").unwrap();
        assert_eq!(pick.rule(), 0);
        assert_eq!(pick.start(), 2);
        assert_eq!(pick.matched(), b"z");
    }

    #[test]
    fn rule_keeps_its_own_leftmost_first_alternation() {
        let sel = Selector::new([r"a|ab"]).unwrap();
        let pick = sel.classify(b"ab").unwrap();
        assert_eq!(pick.matched(), b"a");
    }

    #[test]
    fn non_utf8_bytes_are_preserved_in_the_match() {
        // The winning rule matches a run that includes an invalid UTF-8 byte.
        // This cannot be expressed with the `&str` selector at all.
        let hay: &[u8] = b"xxab\xFF\xFFz";
        let sel = Selector::new([r"z", r"(?-u)\xFF+"]).unwrap();
        let pick = sel.classify(hay).unwrap();
        // The non-UTF-8 run starts at 4, earlier than "z" at 6.
        assert_eq!(pick.rule(), 1);
        assert_eq!(pick.start(), 4);
        assert_eq!(pick.matched(), b"\xFF\xFF");
        assert_eq!(pick.range(), 4..6);
        assert!(std::str::from_utf8(pick.matched()).is_err());
    }

    #[test]
    fn non_utf8_zero_width_tie() {
        // Haystack begins with an invalid UTF-8 byte. The ASCII-compatible
        // word boundary matches zero-width right before 'a' at position 1,
        // which is also where the rule "a" starts; the earlier declared
        // zero-width rule wins. The invalid byte is never touched.
        let hay: &[u8] = b"\xFFa";
        let sel = Selector::new([r"(?-u)\b", "a"]).unwrap();
        let pick = sel.classify(hay).unwrap();
        assert_eq!(pick.rule(), 0);
        assert_eq!(pick.start(), 1);
        assert_eq!(pick.end(), 1);
        assert!(pick.is_empty());
        // The non-UTF-8 leading byte is still exactly where it was.
        assert_eq!(&hay[..1], b"\xFF");
    }

    #[test]
    fn no_match_and_metadata() {
        let sel = Selector::new(["z", "ab"]).unwrap();
        assert!(sel.classify(b"xx").is_none());
        assert_eq!(sel.len(), 2);
        assert_eq!(sel.pattern(1), "ab");
        assert_eq!(format!("{}", sel), r#"bytes::Selector("z", "ab")"#);
    }
}
