/*!
Tests for [`ReplacementPlan`].

These tests cover the acceptance criteria for the precompiled replacement
template feature:

* The basic `$name-$2` example with a non-participating group.
* References to non-existent groups erroring at plan time.
* Two regexes with the same group count but different names cannot share a
  plan.
* `$$` escaping.
* Zero-width match handling.
* The byte version of the API.
*/

use regex::{Regex, ReplacementPlan};

/// The acceptance example: `(?P<name>a)(b)?` with template `$name-$2` and
/// input `a` yields `a-` (the non-participating group `b` is replaced with
/// the empty string).
#[test]
fn acceptance_example() {
    let re = Regex::new(r"(?P<name>a)(b)?").unwrap();
    let plan = re.replacement_plan("$name-$2").unwrap();
    assert_eq!(re.replace_with_plan("a", &plan).unwrap(), "a-");
}

/// A template referencing a non-existent group name errors at plan time.
#[test]
fn missing_name_errors_at_plan_time() {
    let re = Regex::new(r"(?P<name>a)(b)?").unwrap();
    let err = re.replacement_plan("$missing").unwrap_err();
    assert!(
        err.to_string().contains("missing"),
        "error should mention the missing group name, got: {err}"
    );
}

/// A template referencing a non-existent group index errors at plan time.
#[test]
fn missing_index_errors_at_plan_time() {
    let re = Regex::new(r"(?P<name>a)(b)?").unwrap();
    // Group 5 does not exist (there are only 3 groups: 0, 1, 2).
    let err = re.replacement_plan("$5").unwrap_err();
    assert!(
        err.to_string().contains("5"),
        "error should mention the bad index, got: {err}"
    );
}

/// A template referencing a non-existent group via braced syntax errors.
#[test]
fn missing_braced_name_errors_at_plan_time() {
    let re = Regex::new(r"(?P<name>a)(b)?").unwrap();
    assert!(re.replacement_plan("${missing}").is_err());
}

/// Two regexes with the same group count but different group names cannot
/// share a plan.
#[test]
fn same_count_different_names_cannot_share_plan() {
    let re_a = Regex::new(r"(?P<x>a)(?P<y>b)").unwrap();
    let re_b = Regex::new(r"(?P<y>a)(?P<x>b)").unwrap();
    assert_eq!(re_a.captures_len(), re_b.captures_len());

    let plan = re_a.replacement_plan("$x-$y").unwrap();

    // Using the plan with the regex it was compiled for works.
    assert!(re_a.replace_with_plan("ab", &plan).is_ok());

    // Using it with a different regex (same count, different names) is
    // rejected.
    let err = re_b.replace_with_plan("ab", &plan).unwrap_err();
    assert!(
        err.to_string().to_lowercase().contains("layout"),
        "error should mention group layout, got: {err}"
    );
}

/// Two regexes with the same group count and the same names in the same
/// order can share a plan.
#[test]
fn same_layout_can_share_plan() {
    let re_a = Regex::new(r"(?P<x>a)(?P<y>b)").unwrap();
    let re_b = Regex::new(r"(?P<x>a)(?P<y>b)").unwrap();

    let plan = re_a.replacement_plan("$x-$y").unwrap();
    assert!(re_b.replace_with_plan("ab", &plan).is_ok());
}

/// Two regexes with different group counts cannot share a plan.
#[test]
fn different_count_cannot_share_plan() {
    let re_a = Regex::new(r"(?P<x>a)").unwrap();
    let re_b = Regex::new(r"(?P<x>a)(?P<y>b)").unwrap();

    let plan = re_a.replacement_plan("$x").unwrap();
    assert!(re_b.replace_with_plan("ab", &plan).is_err());
}

/// `$$` writes a literal `$`.
#[test]
fn dollar_escape() {
    let re = Regex::new(r"(?P<name>a)").unwrap();
    let plan = re.replacement_plan("$$name").unwrap();
    assert_eq!(re.replace_with_plan("a", &plan).unwrap(), "$name");

    // A lone `$` (not followed by a valid reference) is also literal.
    let plan = re.replacement_plan("$").unwrap();
    assert_eq!(re.replace_with_plan("a", &plan).unwrap(), "$");

    // `$$$` -> `$$` (escaped dollar + lone dollar).
    let plan = re.replacement_plan("$$$").unwrap();
    assert_eq!(re.replace_with_plan("a", &plan).unwrap(), "$$");
}

/// Braced references work.
#[test]
fn braced_references() {
    let re = Regex::new(r"(?P<name>a)(?P<last>b)").unwrap();
    let plan = re.replacement_plan("${name}-${last}").unwrap();
    assert_eq!(re.replace_with_plan("ab", &plan).unwrap(), "a-b");

    // Braced reference adjacent to a letter.
    let plan = re.replacement_plan("${name}x").unwrap();
    assert_eq!(re.replace_with_plan("ab", &plan).unwrap(), "ax");
}

/// Numbered references work.
#[test]
fn numbered_references() {
    let re = Regex::new(r"(a)(b)?").unwrap();
    let plan = re.replacement_plan("$1-$2").unwrap();
    assert_eq!(re.replace_with_plan("a", &plan).unwrap(), "a-");
    assert_eq!(re.replace_with_plan("ab", &plan).unwrap(), "a-b");
}

/// A non-participating group in the middle of a match is replaced with the
/// empty string.
#[test]
fn non_participating_group_is_empty() {
    let re = Regex::new(r"(a)(b)?(c)").unwrap();
    let plan = re.replacement_plan("$1$2$3").unwrap();
    // "ac" matches with group 2 not participating.
    assert_eq!(re.replace_with_plan("ac", &plan).unwrap(), "ac");
    // "abc" matches with all groups participating.
    assert_eq!(re.replace_with_plan("abc", &plan).unwrap(), "abc");
}

/// Zero-width matches are handled correctly: the zero-width advance rule is
/// preserved and text between matches is kept.
#[test]
fn zero_width_matches() {
    // Empty regex matches at every position.
    let re = Regex::new(r"").unwrap();
    let plan = re.replacement_plan("X").unwrap();
    let result = re.replace_all_with_plan("abc", &plan).unwrap();
    // The existing replace semantics for an empty regex: matches at 0, 1, 2,
    // and the text between matches is preserved.
    assert_eq!(result, "XaXbXcX");

    // A regex that can match empty (e.g., `a*`) with a non-empty template.
    let re = Regex::new(r"a*").unwrap();
    let plan = re.replacement_plan("X").unwrap();
    let result = re.replace_all_with_plan("baab", &plan).unwrap();
    // The existing replace semantics yield matches at 0..0, 1..3 ("aa"),
    // and 4..4. The empty match at position 3 is skipped because it is
    // adjacent to the non-empty match that ended there. Text between
    // matches is preserved.
    assert_eq!(result, "XbXbX");
}

/// `replace_with_plan` (single match) only replaces the first match.
#[test]
fn replace_single() {
    let re = Regex::new(r"a").unwrap();
    let plan = re.replacement_plan("X").unwrap();
    assert_eq!(re.replace_with_plan("aaa", &plan).unwrap(), "Xaa");
}

/// `replacen_with_plan` with a limit replaces at most `limit` matches.
#[test]
fn replace_n_with_limit() {
    let re = Regex::new(r"a").unwrap();
    let plan = re.replacement_plan("X").unwrap();
    assert_eq!(re.replacen_with_plan("aaa", 2, &plan).unwrap(), "XXa");
    assert_eq!(re.replacen_with_plan("aaa", 0, &plan).unwrap(), "XXX");
}

/// When there is no match, the haystack is returned borrowed and unchanged.
#[test]
fn no_match_returns_borrowed() {
    let re = Regex::new(r"z").unwrap();
    let plan = re.replacement_plan("X").unwrap();
    let result = re.replace_with_plan("abc", &plan).unwrap();
    assert_eq!(result, "abc");
    assert!(matches!(result, std::borrow::Cow::Borrowed(_)));
}

/// The plan is `Clone` and `Debug`.
#[test]
fn plan_is_clone_and_debug() {
    let re = Regex::new(r"(?P<name>a)").unwrap();
    let plan = re.replacement_plan("$name").unwrap();
    let plan2 = plan.clone();
    assert_eq!(plan.captures_len(), plan2.captures_len());
    let _ = format!("{plan:?}");
}

/// The plan's `captures_len` matches the regex's `captures_len`.
#[test]
fn plan_captures_len() {
    let re = Regex::new(r"(?P<name>a)(b)").unwrap();
    let plan = re.replacement_plan("$name").unwrap();
    assert_eq!(plan.captures_len(), re.captures_len());
}

/// A short demo of compile success and failure.
#[test]
fn compile_success_and_failure_demo() {
    let re = Regex::new(r"(?P<name>a)(b)?").unwrap();

    // Compile success: the template references valid groups.
    let plan = re.replacement_plan("$name-$2");
    assert!(plan.is_ok());
    let plan = plan.unwrap();
    assert_eq!(re.replace_with_plan("a", &plan).unwrap(), "a-");

    // Compile failure: the template references a group that does not exist.
    let bad = re.replacement_plan("$missing");
    assert!(bad.is_err());
    let err = bad.unwrap_err();
    eprintln!("expected compile failure: {err}");
}

/// The byte version of the API works.
#[test]
fn byte_version() {
    use regex::bytes::Regex as BytesRegex;

    let re = BytesRegex::new(r"(?P<name>a)(b)?").unwrap();
    let plan = re.replacement_plan(b"$name-$2").unwrap();
    assert_eq!(re.replace_with_plan(b"a", &plan).unwrap(), &b"a-"[..]);

    // Non-participating group is empty.
    assert_eq!(re.replace_with_plan(b"ab", &plan).unwrap(), &b"a-b"[..]);

    // `$$` escaping.
    let plan = re.replacement_plan(b"$$name").unwrap();
    assert_eq!(re.replace_with_plan(b"a", &plan).unwrap(), &b"$name"[..]);

    // Missing group errors at plan time.
    assert!(re.replacement_plan(b"$missing").is_err());

    // Zero-width matches.
    let re = BytesRegex::new(r"").unwrap();
    let plan = re.replacement_plan(b"X").unwrap();
    assert_eq!(
        re.replace_all_with_plan(b"abc", &plan).unwrap(),
        &b"XaXbXcX"[..]
    );
}

/// The byte version rejects cross-use with a different regex.
#[test]
fn byte_version_rejects_cross_use() {
    use regex::bytes::Regex as BytesRegex;

    let re_a = BytesRegex::new(r"(?P<x>a)(?P<y>b)").unwrap();
    let re_b = BytesRegex::new(r"(?P<y>a)(?P<x>b)").unwrap();

    let plan = re_a.replacement_plan(b"$x-$y").unwrap();
    assert!(re_a.replace_with_plan(b"ab", &plan).is_ok());
    assert!(re_b.replace_with_plan(b"ab", &plan).is_err());
}

/// The plan is publicly exported from the crate root.
#[test]
fn public_export() {
    // This test just checks that the type is accessible at the crate root.
    let _: fn(&Regex, &str) -> Result<ReplacementPlan, regex::Error> =
        ReplacementPlan::new;
}
