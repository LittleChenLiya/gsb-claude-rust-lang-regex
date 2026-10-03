//! Select the first matching rule from a collection of regex rules.
//!
//! This module provides [`RuleSelector`], a small helper for *log
//! classification*: given a list of named regex rules and a haystack, it
//! picks the rule whose match starts at the earliest byte offset. When
//! several rules match at the same offset, the rule that was declared first
//! wins.
//!
//! The selector does **not** concatenate the patterns into one big alternation.
//! Each rule keeps its own compiled [`Regex`], and the selector compares the
//! match positions reported by those regexes. This means that:
//!
//! * A rule that was registered first cannot shadow another rule whose match
//!   starts earlier. The earliest match start always wins, regardless of
//!   declaration order.
//! * Ties (two rules matching at the same start offset) are broken by
//!   declaration order: the rule added first wins.
//! * Within a single rule, the original [`Regex`]'s leftmost-first semantics
//!   are preserved. For example, a rule whose pattern is `a|ab` still matches
//!   `a` first, exactly as it would on its own.
//! * Zero-width matches participate in the selection. A rule that matches the
//!   empty string at a given offset is a real match and can win.
//!
//! Two variants are provided:
//!
//! * [`RuleSelector`] searches `&str` haystacks. Matches are only reported at
//!   UTF-8 byte boundaries, just like the top-level [`Regex`].
//! * [`bytes::RuleSelector`] searches `&[u8]` haystacks and preserves
//!   non-UTF-8 bytes. Matches can start at any byte offset, including offsets
//!   that are not UTF-8 boundaries.
//!
//! # Example
//!
//! ```
//! use regex::selector::{Rule, RuleSelector};
//!
//! let mut selector = RuleSelector::new();
//! selector.push(Rule::new("z", regex::Regex::new("z").unwrap()));
//! selector.push(Rule::new("ab", regex::Regex::new("ab").unwrap()));
//!
//! // The rule `ab` starts matching at offset 2, while `z` only starts at
//! // offset 4. The earlier start wins, even though `z` was declared first.
//! let selected = selector.select("xxabz").unwrap();
//! assert_eq!(selected.name(), "ab");
//! assert_eq!(selected.start(), 2);
//! assert_eq!(selected.as_str(), "ab");
//! ```

use alloc::{string::String, vec::Vec};

use crate::{Captures, Regex};

/// A named regex rule for use with a [`RuleSelector`].
///
/// A rule pairs a human-readable name with the [`Regex`] that identifies it.
/// The name is only used for reporting; it does not influence matching.
#[derive(Debug)]
pub struct Rule {
    name: String,
    regex: Regex,
}

impl Rule {
    /// Creates a new rule with the given name and regex.
    pub fn new(name: impl Into<String>, regex: Regex) -> Rule {
        Rule { name: name.into(), regex }
    }

    /// Returns the name of this rule.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Returns the regex that identifies this rule.
    pub fn regex(&self) -> &Regex {
        &self.regex
    }
}

/// Selects the first matching rule among a collection of regex rules.
///
/// Rules are declared with [`RuleSelector::push`]. The selection rule is:
///
/// 1. The rule whose match starts at the earliest byte offset wins.
/// 2. If several rules match at the same offset, the one that was declared
///    first wins.
///
/// Within a rule, the original regex's leftmost-first semantics are preserved.
/// See the [module documentation](self) for details.
#[derive(Debug, Default)]
pub struct RuleSelector {
    rules: Vec<Rule>,
}

impl RuleSelector {
    /// Creates an empty rule selector.
    pub fn new() -> RuleSelector {
        RuleSelector::default()
    }

    /// Creates an empty rule selector with capacity for at least `capacity`
    /// rules without reallocating.
    pub fn with_capacity(capacity: usize) -> RuleSelector {
        RuleSelector { rules: Vec::with_capacity(capacity) }
    }

    /// Appends a rule to the selector. Rules are matched in declaration order
    /// when their match starts tie.
    pub fn push(&mut self, rule: Rule) -> &mut RuleSelector {
        self.rules.push(rule);
        self
    }

    /// Returns the number of rules in the selector.
    pub fn len(&self) -> usize {
        self.rules.len()
    }

    /// Returns `true` if the selector contains no rules.
    pub fn is_empty(&self) -> bool {
        self.rules.is_empty()
    }

    /// Returns the rule at the given index, if any.
    pub fn get(&self, index: usize) -> Option<&Rule> {
        self.rules.get(index)
    }

    /// Returns an iterator over the rules in declaration order.
    pub fn iter(&self) -> impl Iterator<Item = &Rule> {
        self.rules.iter()
    }

    /// Selects the first matching rule for `haystack`.
    ///
    /// Returns `None` if no rule matches. Otherwise returns a [`SelectedRule`]
    /// containing the winning rule and the captures produced by that rule's
    /// regex.
    ///
    /// # Example
    ///
    /// ```
    /// use regex::selector::{Rule, RuleSelector};
    ///
    /// let mut selector = RuleSelector::new();
    /// selector.push(Rule::new("digits", regex::Regex::new(r"\d+").unwrap()));
    /// selector.push(Rule::new("word", regex::Regex::new(r"\w+").unwrap()));
    ///
    /// // Both rules match at offset 0, so the first declared rule wins.
    /// let selected = selector.select("123 abc").unwrap();
    /// assert_eq!(selected.name(), "digits");
    /// assert_eq!(selected.as_str(), "123");
    /// ```
    pub fn select<'h>(&self, haystack: &'h str) -> Option<SelectedRule<'_, 'h>> {
        let mut best: Option<SelectedRule<'_, 'h>> = None;
        for (index, rule) in self.rules.iter().enumerate() {
            if let Some(caps) = rule.regex.captures(haystack) {
                // Group 0 is the overall match and is always present for a
                // successful match.
                let start =
                    caps.get(0).expect("a matching regex has group 0").start();
                if best.as_ref().map_or(true, |b| start < b.start()) {
                    best = Some(SelectedRule { selector: self, index, caps });
                }
            }
        }
        best
    }
}

/// The result of selecting a rule: the winning rule and its captures.
///
/// The captures are the original captures produced by the winning rule's
/// regex; they are not recombined or re-matched.
pub struct SelectedRule<'r, 'h> {
    selector: &'r RuleSelector,
    index: usize,
    caps: Captures<'h>,
}

impl<'r, 'h> SelectedRule<'r, 'h> {
    /// Returns the index of the winning rule in declaration order.
    pub fn index(&self) -> usize {
        self.index
    }

    /// Returns the winning rule.
    pub fn rule(&self) -> &Rule {
        &self.selector.rules[self.index]
    }

    /// Returns the name of the winning rule.
    pub fn name(&self) -> &str {
        self.rule().name()
    }

    /// Returns the captures produced by the winning rule's regex.
    pub fn captures(&self) -> &Captures<'h> {
        &self.caps
    }

    /// Returns the start byte offset of the overall match.
    pub fn start(&self) -> usize {
        self.caps.get(0).expect("a matching regex has group 0").start()
    }

    /// Returns the end byte offset of the overall match.
    pub fn end(&self) -> usize {
        self.caps.get(0).expect("a matching regex has group 0").end()
    }

    /// Returns the overall match as a string slice.
    pub fn as_str(&self) -> &'h str {
        self.caps.get(0).expect("a matching regex has group 0").as_str()
    }
}

impl core::fmt::Debug for SelectedRule<'_, '_> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("SelectedRule")
            .field("index", &self.index)
            .field("name", &self.name())
            .field("start", &self.start())
            .field("end", &self.end())
            .field("match", &self.as_str())
            .finish()
    }
}

/// Select rules on `&[u8]` haystacks.
///
/// This is the byte-oriented counterpart of the top-level selector. It
/// preserves non-UTF-8 bytes: matches can start at any byte offset, including
/// offsets that are not UTF-8 boundaries. See the [module documentation](self)
/// for the full selection rule.
pub mod bytes {
    use alloc::{string::String, vec::Vec};

    use crate::bytes::{Captures, Regex};

    /// A named byte regex rule for use with a [`RuleSelector`].
    ///
    /// See [`super::Rule`] for details.
    #[derive(Debug)]
    pub struct Rule {
        name: String,
        regex: Regex,
    }

    impl Rule {
        /// Creates a new rule with the given name and byte regex.
        pub fn new(name: impl Into<String>, regex: Regex) -> Rule {
            Rule { name: name.into(), regex }
        }

        /// Returns the name of this rule.
        pub fn name(&self) -> &str {
            &self.name
        }

        /// Returns the byte regex that identifies this rule.
        pub fn regex(&self) -> &Regex {
            &self.regex
        }
    }

    /// Selects the first matching rule among a collection of byte regex rules.
    ///
    /// The selection rule is the same as [`super::RuleSelector`]: earliest
    /// match start wins, ties are broken by declaration order. Matches can
    /// start at any byte offset, including non-UTF-8 boundaries.
    #[derive(Debug, Default)]
    pub struct RuleSelector {
        rules: Vec<Rule>,
    }

    impl RuleSelector {
        /// Creates an empty byte rule selector.
        pub fn new() -> RuleSelector {
            RuleSelector::default()
        }

        /// Creates an empty byte rule selector with capacity for at least
        /// `capacity` rules without reallocating.
        pub fn with_capacity(capacity: usize) -> RuleSelector {
            RuleSelector { rules: Vec::with_capacity(capacity) }
        }

        /// Appends a rule to the selector.
        pub fn push(&mut self, rule: Rule) -> &mut RuleSelector {
            self.rules.push(rule);
            self
        }

        /// Returns the number of rules in the selector.
        pub fn len(&self) -> usize {
            self.rules.len()
        }

        /// Returns `true` if the selector contains no rules.
        pub fn is_empty(&self) -> bool {
            self.rules.is_empty()
        }

        /// Returns the rule at the given index, if any.
        pub fn get(&self, index: usize) -> Option<&Rule> {
            self.rules.get(index)
        }

        /// Returns an iterator over the rules in declaration order.
        pub fn iter(&self) -> impl Iterator<Item = &Rule> {
            self.rules.iter()
        }

        /// Selects the first matching rule for `haystack`.
        ///
        /// Returns `None` if no rule matches. Otherwise returns a
        /// [`SelectedRule`] containing the winning rule and the captures
        /// produced by that rule's regex.
        ///
        /// Matches can start at any byte offset, including offsets that are
        /// not UTF-8 boundaries.
        pub fn select<'h>(&self, haystack: &'h [u8]) -> Option<SelectedRule<'_, 'h>> {
            let mut best: Option<SelectedRule<'_, 'h>> = None;
            for (index, rule) in self.rules.iter().enumerate() {
                if let Some(caps) = rule.regex.captures(haystack) {
                    // Group 0 is the overall match and is always present for a
                    // successful match.
                    let start =
                        caps.get(0).expect("a matching regex has group 0").start();
                    if best.as_ref().map_or(true, |b| start < b.start()) {
                        best = Some(SelectedRule { selector: self, index, caps });
                    }
                }
            }
            best
        }
    }

    /// The result of selecting a byte rule: the winning rule and its captures.
    ///
    /// The captures are the original captures produced by the winning rule's
    /// regex; they are not recombined or re-matched.
    pub struct SelectedRule<'r, 'h> {
        selector: &'r RuleSelector,
        index: usize,
        caps: Captures<'h>,
    }

    impl<'r, 'h> SelectedRule<'r, 'h> {
        /// Returns the index of the winning rule in declaration order.
        pub fn index(&self) -> usize {
            self.index
        }

        /// Returns the winning rule.
        pub fn rule(&self) -> &Rule {
            &self.selector.rules[self.index]
        }

        /// Returns the name of the winning rule.
        pub fn name(&self) -> &str {
            self.rule().name()
        }

        /// Returns the captures produced by the winning rule's regex.
        pub fn captures(&self) -> &Captures<'h> {
            &self.caps
        }

        /// Returns the start byte offset of the overall match.
        pub fn start(&self) -> usize {
            self.caps.get(0).expect("a matching regex has group 0").start()
        }

        /// Returns the end byte offset of the overall match.
        pub fn end(&self) -> usize {
            self.caps.get(0).expect("a matching regex has group 0").end()
        }

        /// Returns the overall match as a byte slice.
        pub fn as_bytes(&self) -> &'h [u8] {
            self.caps.get(0).expect("a matching regex has group 0").as_bytes()
        }
    }

    impl core::fmt::Debug for SelectedRule<'_, '_> {
        fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
            f.debug_struct("SelectedRule")
                .field("index", &self.index)
                .field("name", &self.name())
                .field("start", &self.start())
                .field("end", &self.end())
                .field("match", &self.as_bytes())
                .finish()
        }
    }
}
