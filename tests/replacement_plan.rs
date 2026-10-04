//! Tests for `ReplacementPlan` (string and bytes versions).

use regex::{Regex, ReplacementPlanErrorKind};

#[test]
fn named_and_numbered_groups() {
    // The acceptance case: a group that doesn't participate in the match
    // expands to the empty string.
    let re = Regex::new(r"(?P<name>a)(b)?").unwrap();
    let plan = re.replacement_plan("$name-$2").unwrap();
    assert_eq!(plan.replace_all(&re, "a").unwrap(), "a-");
    assert_eq!(plan.replace_all(&re, "ab").unwrap(), "a-b");
    assert_eq!(plan.replace_all(&re, "xaxab").unwrap(), "xa-xa-b");
}

#[test]
fn unknown_group_fails_at_prepare_time() {
    let re = Regex::new(r"(?P<name>a)(b)?").unwrap();
    let err = re.replacement_plan("$missing").unwrap_err();
    assert_eq!(
        err.kind(),
        &ReplacementPlanErrorKind::UnknownGroup {
            reference: "missing".to_string(),
        }
    );
    // Out-of-range indices fail at prepare time too.
    let err = re.replacement_plan("$3").unwrap_err();
    assert_eq!(
        err.kind(),
        &ReplacementPlanErrorKind::UnknownGroup { reference: "3".to_string() }
    );
    // Braced unknown references as well.
    assert!(re.replacement_plan("${missing}").is_err());
}

#[test]
fn plan_bound_to_group_layout() {
    let re1 = Regex::new(r"(?P<name>a)(b)?").unwrap();
    // Same number of groups, but different group names: not interchangeable.
    let re2 = Regex::new(r"(?P<other>a)(b)?").unwrap();
    // Different number of groups: not interchangeable either.
    let re3 = Regex::new(r"(?P<name>a)").unwrap();
    // Same layout: interchangeable.
    let re4 = Regex::new(r"(?P<name>a)(b)?").unwrap();

    let plan = re1.replacement_plan("$name-$2").unwrap();
    let err = plan.replace_all(&re2, "ab").unwrap_err();
    assert_eq!(err.kind(), &ReplacementPlanErrorKind::LayoutMismatch);
    let err = plan.replace_all(&re3, "ab").unwrap_err();
    assert_eq!(err.kind(), &ReplacementPlanErrorKind::LayoutMismatch);
    assert_eq!(plan.replace_all(&re4, "ab").unwrap(), "a-b");
}

#[test]
fn dollar_escape() {
    let re = Regex::new(r"(?P<name>a)").unwrap();
    let plan = re.replacement_plan("$$$name").unwrap();
    assert_eq!(plan.replace_all(&re, "a").unwrap(), "$a");
    let plan = re.replacement_plan("$$$$").unwrap();
    assert_eq!(plan.replace_all(&re, "a").unwrap(), "$$");
    // A '$' that doesn't start a valid reference is a literal '$'.
    let plan = re.replacement_plan("a$-b").unwrap();
    assert_eq!(plan.replace_all(&re, "a").unwrap(), "a$-b");
    let plan = re.replacement_plan("cost: $").unwrap();
    assert_eq!(plan.replace_all(&re, "a").unwrap(), "cost: $");
}

#[test]
fn zero_width_matches() {
    // Empty matches must advance the search position, exactly like
    // Regex::replace_all does.
    let re = Regex::new(r"x*").unwrap();
    let plan = re.replacement_plan("<$0>").unwrap();
    assert_eq!(
        plan.replace_all(&re, "abx").unwrap(),
        re.replace_all("abx", "<$0>"),
    );
    assert_eq!(plan.replace_all(&re, "abx").unwrap(), "<>a<>b<x>");

    let re = Regex::new(r"(?m)^").unwrap();
    let plan = re.replacement_plan("> ").unwrap();
    let hay = "one\ntwo";
    assert_eq!(plan.replace_all(&re, hay).unwrap(), re.replace_all(hay, "> "));
    assert_eq!(plan.replace_all(&re, hay).unwrap(), "> one\n> two");
}

#[test]
fn replace_and_replacen_limits() {
    let re = Regex::new(r"(?P<w>\w+)").unwrap();
    let plan = re.replacement_plan("<$w>").unwrap();
    assert_eq!(plan.replace(&re, "one two").unwrap(), "<one> two");
    assert_eq!(
        plan.replacen(&re, "one two three", 2).unwrap(),
        "<one> <two> three"
    );
    // No match: the haystack is returned unchanged (and borrowed).
    let no_match = plan.replace_all(&re, "!!!").unwrap();
    assert!(matches!(no_match, std::borrow::Cow::Borrowed("!!!")));
}

#[test]
fn literal_only_plan_uses_fast_path() {
    let re = Regex::new(r"a+").unwrap();
    let plan = re.replacement_plan("$$").unwrap();
    assert_eq!(plan.replace_all(&re, "aa aaa").unwrap(), "$ $");
    let plan = re.replacement_plan("").unwrap();
    assert_eq!(plan.replace_all(&re, "aa").unwrap(), "");
}

#[test]
fn whole_match_and_braced_refs() {
    let re = Regex::new(r"(?P<year>\d{4})-(?P<month>\d{2})").unwrap();
    let plan = re.replacement_plan("${month}/${year} [$0]").unwrap();
    assert_eq!(plan.replace_all(&re, "2026-10").unwrap(), "10/2026 [2026-10]");
}

#[test]
fn bytes_named_and_numbered_groups() {
    let re = regex::bytes::Regex::new(r"(?P<name>a)(b)?").unwrap();
    let plan = re.replacement_plan(b"$name-$2").unwrap();
    assert_eq!(&*plan.replace_all(&re, b"a").unwrap(), &b"a-"[..]);
    assert_eq!(&*plan.replace_all(&re, b"xaxab").unwrap(), &b"xa-xa-b"[..]);
}

#[test]
fn bytes_unknown_group_fails_at_prepare_time() {
    let re = regex::bytes::Regex::new(r"(?P<name>a)(b)?").unwrap();
    let err = re.replacement_plan(b"$missing").unwrap_err();
    assert_eq!(
        err.kind(),
        &ReplacementPlanErrorKind::UnknownGroup {
            reference: "missing".to_string(),
        }
    );
    assert!(re.replacement_plan(b"$3").is_err());
}

#[test]
fn bytes_plan_bound_to_group_layout() {
    let re1 = regex::bytes::Regex::new(r"(?P<name>a)(b)?").unwrap();
    let re2 = regex::bytes::Regex::new(r"(?P<other>a)(b)?").unwrap();
    let plan = re1.replacement_plan(b"$name-$2").unwrap();
    let err = plan.replace_all(&re2, b"ab").unwrap_err();
    assert_eq!(err.kind(), &ReplacementPlanErrorKind::LayoutMismatch);
}

#[test]
fn bytes_dollar_escape_and_zero_width() {
    let re = regex::bytes::Regex::new(r"(?P<name>a)").unwrap();
    let plan = re.replacement_plan(b"$$$name").unwrap();
    assert_eq!(&*plan.replace_all(&re, b"a").unwrap(), &b"$a"[..]);

    // Zero-width matches advance just like Regex::replace_all.
    let re = regex::bytes::Regex::new(r"x*").unwrap();
    let plan = re.replacement_plan(b"<$0>").unwrap();
    assert_eq!(
        &*plan.replace_all(&re, b"abx").unwrap(),
        &*re.replace_all(b"abx", &b"<$0>"[..]),
    );
    assert_eq!(&*plan.replace_all(&re, b"abx").unwrap(), &b"<>a<>b<x>"[..]);
}

#[test]
fn bytes_non_utf8_haystack() {
    let re = regex::bytes::Regex::new(r"(?-u)(?P<x>[\x00-\xFF]+)").unwrap();
    let plan = re.replacement_plan(b"<$x>").unwrap();
    let hay = b"\xFF\xFE";
    assert_eq!(&*plan.replace_all(&re, hay).unwrap(), &b"<\xFF\xFE>"[..]);
}
