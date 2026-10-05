use std::borrow::Cow;

// Three matches, each expanded to the two-byte UTF-8 encoding of 'é'.
// A budget of exactly 6 bytes succeeds; one byte less fails and no
// partial string is returned. The replacement callback runs exactly
// once per match.
#[test]
fn bounded_three_matches_two_bytes_each() {
    let re = regex::Regex::new(r"[a-z]").unwrap();

    let mut calls = 0;
    let result = re.try_replacen("abc", 0, 6, |_: &regex::Captures<'_>| {
        calls += 1;
        "é"
    });
    assert_eq!(result.unwrap(), "ééé");
    assert_eq!(calls, 3, "callback must run exactly once per match");

    // Budget one byte short: fails, no half string.
    let mut calls = 0;
    let result = re.try_replacen("abc", 0, 5, |_: &regex::Captures<'_>| {
        calls += 1;
        "é"
    });
    let err = result.unwrap_err();
    assert_eq!(err.budget(), 5);
    assert!(calls <= 3, "callback must never be re-executed per match");

    // The original input is untouched.
    let hay = String::from("abc");
    let _ = re.try_replacen(&hay, 0, 6, "é");
    assert_eq!(hay, "abc");
}

// UTF-8 output is counted by bytes, not by code points: 'é' is 2 bytes.
#[test]
fn bounded_utf8_counts_output_bytes() {
    let re = regex::Regex::new(r"a").unwrap();
    assert!(re.try_replacen("a", 0, 1, "é").is_err());
    assert_eq!(re.try_replacen("a", 0, 2, "é").unwrap(), "é");
}

// Unmatched portions of the haystack count toward the budget too.
#[test]
fn bounded_unmatched_parts_count() {
    let re = regex::Regex::new(r"b").unwrap();
    // "abc" with b -> "XY" gives "aXYc" (4 bytes).
    assert_eq!(re.try_replacen("abc", 0, 4, "XY").unwrap(), "aXYc");
    assert!(re.try_replacen("abc", 0, 3, "XY").is_err());
    // The unmatched tail alone can blow the budget.
    let re = regex::Regex::new(r"a").unwrap();
    assert!(re.try_replacen("abcdef", 0, 3, "Z").is_err());
}

// Zero-width matches advance along the haystack and terminate.
#[test]
fn bounded_zero_width_terminates() {
    let re = regex::Regex::new(r"").unwrap();
    assert_eq!(re.try_replacen("ab", 0, 5, "-").unwrap(), "-a-b-");
    // Same, but with a budget that the zero-width expansions exceed.
    assert!(re.try_replacen("ab", 0, 4, "-").is_err());

    let re = regex::Regex::new(r"\b").unwrap();
    assert_eq!(re.try_replacen("ab cd", 0, 9, "|").unwrap(), "|ab| |cd|");
}

// A limit on the number of replacements still applies.
#[test]
fn bounded_limit() {
    let re = regex::Regex::new(r"[0-9]").unwrap();
    assert_eq!(re.try_replacen("123", 2, 5, "X").unwrap(), "XX3");
    assert!(re.try_replacen("123", 2, 1, "X").is_err());
}

// No matches: the haystack is returned borrowed and unchanged.
#[test]
fn bounded_no_match_borrows() {
    let re = regex::Regex::new(r"z").unwrap();
    match re.try_replacen("abc", 0, 3, "Q").unwrap() {
        Cow::Borrowed(s) => assert_eq!(s, "abc"),
        Cow::Owned(_) => panic!("expected borrowed haystack"),
    }
    // Even the borrowed output must fit the budget.
    assert!(re.try_replacen("abc", 0, 2, "Q").is_err());
}

// The byte-oriented API gets the same treatment.
#[test]
fn bounded_bytes_api() {
    let re = regex::bytes::Regex::new(r"[a-z]").unwrap();

    let mut calls = 0;
    let result =
        re.try_replacen(b"abc", 0, 6, |_: &regex::bytes::Captures<'_>| {
            calls += 1;
            "é".as_bytes().to_vec()
        });
    assert_eq!(result.unwrap().as_ref(), "ééé".as_bytes());
    assert_eq!(calls, 3);

    let err: regex::bytes::BudgetExceeded =
        re.try_replacen(b"abc", 0, 5, &b"xx"[..]).unwrap_err();
    assert_eq!(err.budget(), 5);

    // Zero-width matches terminate for bytes too.
    let re = regex::bytes::Regex::new(r"").unwrap();
    assert_eq!(re.try_replacen(b"ab", 0, 5, b"-").unwrap().as_ref(), b"-a-b-");
}

// The error type is a plain value: display, debug, equality.
#[test]
fn bounded_error_type() {
    let re = regex::Regex::new(r"a").unwrap();
    let err1 = re.try_replacen("a", 0, 0, "bb").unwrap_err();
    let err2 = re.try_replacen("a", 0, 0, "bb").unwrap_err();
    assert_eq!(err1, err2);
    assert_eq!(
        err1.to_string(),
        "replacement output exceeds byte budget of 0"
    );
}

// The pre-existing replace_all behavior is unchanged.
#[test]
fn replace_all_unchanged() {
    let re = regex::Regex::new(r"[a-z]").unwrap();
    assert_eq!(re.replace_all("abc", "é"), "ééé");
    assert_eq!(re.replace_all("abc", ""), "");
}
