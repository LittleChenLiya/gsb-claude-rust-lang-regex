/*!
Select one regex rule to classify a haystack, such as a log line.

This module solves a different problem from [`RegexSet`](crate::RegexSet). A
`RegexSet` tells you *all* of the patterns that match anywhere in a haystack.
When classifying logs, however, one usually wants exactly *one* rule to win,
namely the rule whose match appears first in the haystack. Moreover, joining
the patterns into one big `a|b|...` alternate changes the answer: the combined
regex becomes a single leftmost-first alternation, so the *declaration order*
of the branches can beat a branch that matches at an earlier position within
the same rule, and captures from the individual rules are hard to recover.

The [`Selector`] type in this module instead keeps every rule as its own,
independently compiled [`Regex`](crate::Regex). To classify a haystack it:

1. Runs every rule independently and records the byte offset where each
   rule's leftmost match starts.
2. Chooses the rule whose match starts at the smallest offset. If two rules
   start at the same offset, the rule that was declared first wins.
3. Re-runs the winning rule to return its *original*
   [`Captures`](crate::Captures), exactly as if that regex alone had been
   used to search the haystack.

Because each pattern is compiled separately, a rule that is itself an
alternation keeps its ordinary leftmost-first semantics. For example, the rule
`a|ab` still prefers `a`; the selector never rewrites or reorders the
alternatives *inside* a rule. A match may be zero-width (an empty match at
some position participates in selection like any other match), and when no
rule matches, classification returns `None`.

There are two variants:

* [`Selector`] operates on `&str` haystacks. As with ordinary `&str` regex
  searches, match endpoints always fall on UTF-8 boundaries.
* [`bytes::Selector`] operates on `&[u8]` haystacks and can match through
  non-UTF-8 bytes when a rule is built with Unicode mode disabled (e.g.
  `(?-u)\xFF+`).

# Example

```rust
use regex::selector::Selector;

// Rule 0 matches "z"; rule 1 matches "ab". They are *not* joined into one
// big alternation.
let sel = Selector::new(["z", "ab"]).unwrap();

// In "xxabz", "ab" begins at byte 2 while "z" begins at byte 4, so rule 1
// wins even though rule 0 was declared first.
let picked = sel.classify("xxabz").unwrap();
assert_eq!(picked.rule(), 1);
assert_eq!(picked.matched(), "ab");

// "z" also starts at byte 2 of "xxzz", and so does the longer rule "zz".
// When two rules match at the same position, the rule declared first wins.
let sel = Selector::new(["z", "zz"]).unwrap();
let picked = sel.classify("xxzz").unwrap();
assert_eq!(picked.rule(), 0);
assert_eq!(picked.start(), 2);

// A rule that is itself an alternation keeps its own leftmost-first order:
// "a|ab" matches "a" in "ab", it is never rewritten to prefer "ab".
let sel = Selector::new([r"a|ab"]).unwrap();
let picked = sel.classify("ab").unwrap();
assert_eq!(picked.matched(), "a");

// No rule matches:
assert!(Selector::new(["z", "ab"]).unwrap().classify("xx").is_none());
```

# Relationship to `RegexSet`

A `RegexSet` is a union search that reports every matching pattern; a
`Selector` is a small classifier that reports exactly one pattern together
with its captures. If you only need to know whether any pattern matches,
prefer a `RegexSet` (or the cheaper `is_match` queries); use a `Selector` when
the relative *positions* of the matches decide the answer and you need the
winning pattern's capture groups.
*/

pub mod bytes;
pub mod string;

#[doc(inline)]
pub use self::string::{Selection, Selector};
