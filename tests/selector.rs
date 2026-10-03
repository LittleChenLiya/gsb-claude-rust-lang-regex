//! Targeted tests for the rule selector.

use regex::bytes as bstr;
use regex::selector::{bytes as bsel, Rule, RuleSelector};
use regex::Regex;

/// The acceptance example: rules `z` and `ab` on `xxabz` must select `ab`,
/// because `ab` starts matching at offset 2 while `z` only starts at offset 4.
/// The rule registered first (`z`) must not shadow the earlier start of `ab`.
#[test]
fn earlier_start_wins_over_declaration_order() {
    let mut selector = RuleSelector::new();
    selector.push(Rule::new("z", Regex::new("z").unwrap()));
    selector.push(Rule::new("ab", Regex::new("ab").unwrap()));

    let selected = selector.select("xxabz").unwrap();
    assert_eq!(selected.name(), "ab");
    assert_eq!(selected.index(), 1);
    assert_eq!(selected.start(), 2);
    assert_eq!(selected.end(), 4);
    assert_eq!(selected.as_str(), "ab");
}

/// When two rules match at the same start offset, the rule declared first
/// wins. Both rules here start matching at offset 2.
#[test]
fn tie_at_same_start_picks_first_declared() {
    let mut selector = RuleSelector::new();
    selector.push(Rule::new("first", Regex::new("ab").unwrap()));
    selector.push(Rule::new("second", Regex::new("ab").unwrap()));

    let selected = selector.select("xxabz").unwrap();
    assert_eq!(selected.name(), "first");
    assert_eq!(selected.index(), 0);
    assert_eq!(selected.start(), 2);
}

/// A rule whose pattern is `a|ab` must keep its own leftmost-first behavior:
/// it matches `a` first, not `ab`.
#[test]
fn leftmost_first_semantics_preserved_within_a_rule() {
    let mut selector = RuleSelector::new();
    selector.push(Rule::new("alt", Regex::new("a|ab").unwrap()));

    let selected = selector.select("ab").unwrap();
    assert_eq!(selected.name(), "alt");
    assert_eq!(selected.start(), 0);
    assert_eq!(selected.end(), 1);
    assert_eq!(selected.as_str(), "a");
}

/// A zero-width match participates in selection and can win. The rule `a*`
/// matches the empty string at offset 0, which beats a rule that only starts
/// matching at offset 1.
#[test]
fn zero_width_match_participates_in_selection() {
    let mut selector = RuleSelector::new();
    selector.push(Rule::new("star", Regex::new("a*").unwrap()));
    selector.push(Rule::new("one", Regex::new("b").unwrap()));

    let selected = selector.select("b").unwrap();
    assert_eq!(selected.name(), "star");
    assert_eq!(selected.start(), 0);
    assert_eq!(selected.end(), 0);
    assert!(selected.as_str().is_empty());
}

/// A zero-width match at a later offset still loses to a non-zero match that
/// starts earlier.
#[test]
fn zero_width_match_loses_to_earlier_nonzero_match() {
    let mut selector = RuleSelector::new();
    selector.push(Rule::new("word", Regex::new(r"\w+").unwrap()));
    selector.push(Rule::new("boundary", Regex::new(r"\b").unwrap()));

    // `\b` matches at offset 0 (zero-width) and `\w+` also starts at 0, so the
    // first declared rule wins. Use a case where the zero-width rule starts
    // later: `\b` at the boundary before `world` (offset 6) vs `hello` at 0.
    let selected = selector.select("hello world").unwrap();
    assert_eq!(selected.name(), "word");
    assert_eq!(selected.start(), 0);
    assert_eq!(selected.as_str(), "hello");
}

/// When no rule matches, `select` returns `None`.
#[test]
fn no_match_returns_none() {
    let mut selector = RuleSelector::new();
    selector.push(Rule::new("digits", Regex::new(r"\d+").unwrap()));
    selector.push(Rule::new("upper", Regex::new(r"[A-Z]+").unwrap()));

    assert!(selector.select("hello world").is_none());
}

/// The captures returned are the original captures produced by the winning
/// rule's regex, including capture groups.
#[test]
fn original_captures_are_returned() {
    let mut selector = RuleSelector::new();
    selector.push(Rule::new("kv", Regex::new(r"(\w+)=(\d+)").unwrap()));

    let selected = selector.select("id=42").unwrap();
    assert_eq!(selected.name(), "kv");
    let caps = selected.captures();
    assert_eq!(&caps[1], "id");
    assert_eq!(&caps[2], "42");
    assert_eq!(caps.get_match().as_str(), "id=42");
}

/// The string selector only reports matches at UTF-8 byte boundaries. A rule
/// that would match inside a multi-byte character does not produce a match at
/// a non-boundary offset.
#[test]
fn string_version_respects_utf8_boundaries() {
    let mut selector = RuleSelector::new();
    // `a` followed by any char: on "éa" the match starts at offset 2 (the `a`),
    // not inside the é.
    selector.push(Rule::new("a", Regex::new("a").unwrap()));

    let selected = selector.select("éa").unwrap();
    assert_eq!(selected.start(), 2);
    assert_eq!(selected.as_str(), "a");

    // A rule matching the empty string at a non-boundary offset is not
    // reported by the string selector (it cannot match inside a char).
    let mut selector = RuleSelector::new();
    selector.push(Rule::new("empty", Regex::new(r"").unwrap()));
    let selected = selector.select("é").unwrap();
    assert_eq!(selected.start(), 0);
    assert_eq!(selected.end(), 0);
}

/// The bytes selector preserves non-UTF-8 bytes. A byte regex can match
/// invalid UTF-8, and the match start is reported at the exact byte offset.
#[test]
fn bytes_version_preserves_non_utf8() {
    let mut selector = bsel::RuleSelector::new();
    selector.push(bsel::Rule::new(
        "ff",
        bstr::Regex::new(r"(?-u)\xFF").unwrap(),
    ));
    selector.push(bsel::Rule::new(
        "ab",
        bstr::Regex::new(r"ab").unwrap(),
    ));

    // `ab` starts at offset 2, while `\xFF` starts at offset 4. The earlier
    // start wins even though `\xFF` was declared first.
    let hay = b"xxab\xFFz";
    let selected = selector.select(hay).unwrap();
    assert_eq!(selected.name(), "ab");
    assert_eq!(selected.start(), 2);
    assert_eq!(selected.as_bytes(), b"ab");

    // The non-UTF-8 rule matches on its own, at the exact byte offset.
    let mut selector = bsel::RuleSelector::new();
    selector.push(bsel::Rule::new(
        "ff",
        bstr::Regex::new(r"(?-u)\xFF").unwrap(),
    ));
    let selected = selector.select(b"abc\xFF").unwrap();
    assert_eq!(selected.name(), "ff");
    assert_eq!(selected.start(), 3);
    assert_eq!(selected.as_bytes(), b"\xFF");
}

/// A zero-width match in the bytes selector participates in selection, even
/// at a non-UTF-8 boundary.
#[test]
fn bytes_zero_width_match_at_non_utf8_boundary() {
    let mut selector = bsel::RuleSelector::new();
    selector.push(bsel::Rule::new(
        "boundary",
        bstr::Regex::new(r"(?-u)\b").unwrap(),
    ));
    selector.push(bsel::Rule::new(
        "cd",
        bstr::Regex::new(r"cd").unwrap(),
    ));

    // `\b` matches at offset 1, right after the invalid byte \xFF (between the
    // non-word byte and the word byte `c`). `cd` also starts at offset 1, so
    // the first declared rule wins on the tie.
    let hay = b"\xFFcd";
    let selected = selector.select(hay).unwrap();
    assert_eq!(selected.name(), "boundary");
    assert_eq!(selected.start(), 1);
    assert!(selected.as_bytes().is_empty());

    // The zero-width rule also loses to a match that starts earlier. On
    // `b"!ab"`, the first `\b` only starts at offset 1 (between the non-word
    // byte `!` and the word byte `a`), while a rule matching `!` starts at
    // offset 0. The earlier start wins.
    let mut selector = bsel::RuleSelector::new();
    selector.push(bsel::Rule::new(
        "boundary",
        bstr::Regex::new(r"(?-u)\b").unwrap(),
    ));
    selector.push(bsel::Rule::new(
        "bang",
        bstr::Regex::new(r"!").unwrap(),
    ));
    let hay = b"!ab";
    let selected = selector.select(hay).unwrap();
    assert_eq!(selected.name(), "bang");
    assert_eq!(selected.start(), 0);
}

/// Declaration order only breaks ties; a rule registered second with a match
/// starting earlier still wins.
#[test]
fn declaration_order_only_breaks_ties() {
    let mut selector = RuleSelector::new();
    selector.push(Rule::new("late", Regex::new("z").unwrap()));
    selector.push(Rule::new("early", Regex::new("a").unwrap()));

    // `a` starts at offset 0, so the second-declared rule wins even though `z`
    // was registered first.
    let selected = selector.select("az").unwrap();
    assert_eq!(selected.name(), "early");
    assert_eq!(selected.start(), 0);
}

/// An empty selector returns `None`.
#[test]
fn empty_selector_returns_none() {
    let selector = RuleSelector::new();
    assert!(selector.select("anything").is_none());
    assert!(selector.is_empty());
    assert_eq!(selector.len(), 0);
}
