macro_rules! replace(
    ($name:ident, $which:ident, $re:expr,
     $search:expr, $replace:expr, $result:expr) => (
        #[test]
        fn $name() {
            let re = regex::Regex::new($re).unwrap();
            assert_eq!(re.$which($search, $replace), $result);
        }
    );
);

replace!(first, replace, r"[0-9]", "age: 26", "Z", "age: Z6");
replace!(plus, replace, r"[0-9]+", "age: 26", "Z", "age: Z");
replace!(all, replace_all, r"[0-9]", "age: 26", "Z", "age: ZZ");
replace!(groups, replace, r"([^ ]+)[ ]+([^ ]+)", "w1 w2", "$2 $1", "w2 w1");
replace!(
    double_dollar,
    replace,
    r"([^ ]+)[ ]+([^ ]+)",
    "w1 w2",
    "$2 $$1",
    "w2 $1"
);
// replace!(adjacent_index, replace,
// r"([^aeiouy])ies$", "skies", "$1y", "sky");
replace!(
    named,
    replace_all,
    r"(?P<first>[^ ]+)[ ]+(?P<last>[^ ]+)(?P<space>[ ]*)",
    "w1 w2 w3 w4",
    "$last $first$space",
    "w2 w1 w4 w3"
);
replace!(
    trim,
    replace_all,
    "^[ \t]+|[ \t]+$",
    " \t  trim me\t   \t",
    "",
    "trim me"
);
replace!(number_hyphen, replace, r"(.)(.)", "ab", "$1-$2", "a-b");
// replace!(number_underscore, replace, r"(.)(.)", "ab", "$1_$2", "a_b");
replace!(
    simple_expand,
    replace_all,
    r"([a-z]) ([a-z])",
    "a b",
    "$2 $1",
    "b a"
);
replace!(
    literal_dollar1,
    replace_all,
    r"([a-z]+) ([a-z]+)",
    "a b",
    "$$1",
    "$1"
);
replace!(
    literal_dollar2,
    replace_all,
    r"([a-z]+) ([a-z]+)",
    "a b",
    "$2 $$c $1",
    "b $c a"
);
replace!(
    no_expand1,
    replace,
    r"([^ ]+)[ ]+([^ ]+)",
    "w1 w2",
    regex::NoExpand("$2 $1"),
    "$2 $1"
);
replace!(
    no_expand2,
    replace,
    r"([^ ]+)[ ]+([^ ]+)",
    "w1 w2",
    regex::NoExpand("$$1"),
    "$$1"
);
replace!(
    closure_returning_reference,
    replace,
    r"([0-9]+)",
    "age: 26",
    |captures: &regex::Captures<'_>| { captures[1][0..1].to_owned() },
    "age: 2"
);
replace!(
    closure_returning_value,
    replace,
    r"[0-9]+",
    "age: 26",
    |_captures: &regex::Captures<'_>| "Z".to_owned(),
    "age: Z"
);

// See https://github.com/rust-lang/regex/issues/314
replace!(
    match_at_start_replace_with_empty,
    replace_all,
    r"foo",
    "foobar",
    "",
    "bar"
);

// See https://github.com/rust-lang/regex/issues/393
replace!(single_empty_match, replace, r"^", "bar", "foo", "foobar");

// See https://github.com/rust-lang/regex/issues/399
replace!(
    capture_longest_possible_name,
    replace_all,
    r"(.)",
    "b",
    "${1}a $1a",
    "ba "
);

replace!(
    impl_string,
    replace,
    r"[0-9]",
    "age: 26",
    "Z".to_string(),
    "age: Z6"
);
replace!(
    impl_string_ref,
    replace,
    r"[0-9]",
    "age: 26",
    &"Z".to_string(),
    "age: Z6"
);
replace!(
    impl_cow_str_borrowed,
    replace,
    r"[0-9]",
    "age: 26",
    std::borrow::Cow::<'_, str>::Borrowed("Z"),
    "age: Z6"
);
replace!(
    impl_cow_str_borrowed_ref,
    replace,
    r"[0-9]",
    "age: 26",
    &std::borrow::Cow::<'_, str>::Borrowed("Z"),
    "age: Z6"
);
replace!(
    impl_cow_str_owned,
    replace,
    r"[0-9]",
    "age: 26",
    std::borrow::Cow::<'_, str>::Owned("Z".to_string()),
    "age: Z6"
);
replace!(
    impl_cow_str_owned_ref,
    replace,
    r"[0-9]",
    "age: 26",
    &std::borrow::Cow::<'_, str>::Owned("Z".to_string()),
    "age: Z6"
);

#[test]
fn replacen_no_captures() {
    let re = regex::Regex::new(r"[0-9]").unwrap();
    assert_eq!(re.replacen("age: 1234", 2, "Z"), "age: ZZ34");
}

#[test]
fn replacen_with_captures() {
    let re = regex::Regex::new(r"([0-9])").unwrap();
    assert_eq!(re.replacen("age: 1234", 2, "${1}Z"), "age: 1Z2Z34");
}

// ---------------------------------------------------------------------------
// Bounded replacement tests
// ---------------------------------------------------------------------------

#[test]
fn replace_all_bounded_string_fits() {
    let re = regex::Regex::new(r"\d").unwrap();
    let hay = "a1b2c3";
    // "a1b2c3" is 6 bytes; each of the 3 matches adds 1 byte ("0" -> "00"),
    // so the result is 9 bytes.
    let result = re.replace_all_bounded(hay, 9, "00").unwrap();
    assert_eq!(result, "a00b00c00");
    // The original input is untouched.
    assert_eq!(hay, "a1b2c3");
}

#[test]
fn replace_all_bounded_string_no_match_borrows() {
    let re = regex::Regex::new(r"\d").unwrap();
    let hay = "abc";
    // No matches: the whole haystack is copied unchanged. It fits, so we get
    // a borrowed value and no allocation.
    let result = re.replace_all_bounded(hay, 3, "00").unwrap();
    assert_eq!(result, "abc");
    assert!(matches!(result, std::borrow::Cow::Borrowed(_)));
}

#[test]
fn replace_all_bounded_string_no_match_too_big() {
    let re = regex::Regex::new(r"\d").unwrap();
    let hay = "abcdef";
    // No matches, but the haystack itself (6 bytes) exceeds the budget.
    let err = re.replace_all_bounded(hay, 5, "00").unwrap_err();
    assert_eq!(err.budget(), 5);
    assert_eq!(err.needed(), 6);
}

#[test]
fn replace_all_bounded_string_fails_without_half_string() {
    let re = regex::Regex::new(r"\d").unwrap();
    let hay = "a1b2c3";
    // Result would be 9 bytes; budget of 8 is insufficient.
    let err = re.replace_all_bounded(hay, 8, "00").unwrap_err();
    assert_eq!(err.budget(), 8);
    // The error reports a lower bound on the needed bytes.
    assert!(err.needed() > err.budget());
    // Display works.
    let msg = err.to_string();
    assert!(msg.contains("8"));
}

#[test]
fn replace_all_bounded_string_callback_runs_once_per_match() {
    let re = regex::Regex::new(r"\d").unwrap();
    let hay = "a1b2c3";
    let mut calls = 0;
    let result = re.replace_all_bounded(hay, 8, |caps: &regex::Captures| {
        calls += 1;
        // Return a replacement that expands each match to two bytes.
        format!("{}0", &caps[0])
    });
    assert!(result.is_err());
    // The callback ran exactly once per match, never twice.
    assert_eq!(calls, 3);
}

#[test]
fn replace_all_bounded_string_callback_runs_once_when_fits() {
    let re = regex::Regex::new(r"\d").unwrap();
    let hay = "a1b2c3";
    let mut calls = 0;
    let result = re
        .replace_all_bounded(hay, 100, |caps: &regex::Captures| {
            calls += 1;
            format!("{}0", &caps[0])
        })
        .unwrap();
    assert_eq!(result, "a10b20c30");
    assert_eq!(calls, 3);
}

#[test]
fn replace_all_bounded_string_zero_width_no_infinite_loop() {
    let re = regex::Regex::new(r"").unwrap();
    let hay = "abc";
    let result = re.replace_all_bounded(hay, 100, "-").unwrap();
    assert_eq!(result, "-a-b-c-");
}

#[test]
fn replace_all_bounded_string_zero_width_budget() {
    let re = regex::Regex::new(r"").unwrap();
    let hay = "ab";
    // Empty pattern matches at 3 positions; each replacement is 1 byte and
    // each unmatched byte is 1 byte. Result is "-a-b-" = 5 bytes.
    assert_eq!(re.replace_all_bounded(hay, 5, "-").unwrap(), "-a-b-");
    assert!(re.replace_all_bounded(hay, 4, "-").is_err());
}

#[test]
fn replace_all_bounded_string_utf8_counted_in_bytes() {
    let re = regex::Regex::new(r"é").unwrap();
    let hay = "aé";
    // "a" is 1 byte, "é" is 2 bytes. Replacing "é" (2 bytes) with "x" (1 byte)
    // gives "ax" = 2 bytes.
    assert_eq!(re.replace_all_bounded(hay, 2, "x").unwrap(), "ax");
    assert!(re.replace_all_bounded(hay, 1, "x").is_err());
    // Replacing with "é" (2 bytes) gives "aé" = 3 bytes.
    assert_eq!(re.replace_all_bounded(hay, 3, "é").unwrap(), "aé");
    assert!(re.replace_all_bounded(hay, 2, "é").is_err());
}

#[test]
fn replace_all_bounded_string_original_untouched() {
    let re = regex::Regex::new(r"\d").unwrap();
    let hay = "a1b2c3";
    let _ = re.replace_all_bounded(hay, 1, "00");
    let _ = re.replace_all_bounded(hay, 100, "00");
    assert_eq!(hay, "a1b2c3");
}

#[test]
fn replace_all_bounded_string_preserves_replace_all() {
    // The bounded routine must agree with `replace_all` when the budget is
    // generous enough.
    let re = regex::Regex::new(r"(\w+)").unwrap();
    let hay = "hello world";
    let expected = re.replace_all(hay, "[$1]");
    let got = re.replace_all_bounded(hay, 1000, "[$1]").unwrap();
    assert_eq!(got, expected);
}

#[test]
fn replace_all_bounded_bytes_fits() {
    let re = regex::bytes::Regex::new(r"\d").unwrap();
    let hay = b"a1b2c3";
    let result = re.replace_all_bounded(hay, 9, b"00").unwrap();
    assert_eq!(result, &b"a00b00c00"[..]);
    assert_eq!(hay, b"a1b2c3");
}

#[test]
fn replace_all_bounded_bytes_no_match_borrows() {
    let re = regex::bytes::Regex::new(r"\d").unwrap();
    let hay = b"abc";
    let result = re.replace_all_bounded(hay, 3, b"00").unwrap();
    assert_eq!(result, &b"abc"[..]);
    assert!(matches!(result, std::borrow::Cow::Borrowed(_)));
}

#[test]
fn replace_all_bounded_bytes_fails_without_half_string() {
    let re = regex::bytes::Regex::new(r"\d").unwrap();
    let hay = b"a1b2c3";
    let err = re.replace_all_bounded(hay, 8, b"00").unwrap_err();
    assert_eq!(err.budget(), 8);
    assert!(err.needed() > err.budget());
}

#[test]
fn replace_all_bounded_bytes_callback_runs_once_per_match() {
    let re = regex::bytes::Regex::new(r"\d").unwrap();
    let hay = b"a1b2c3";
    let mut calls = 0;
    let result =
        re.replace_all_bounded(hay, 8, |caps: &regex::bytes::Captures| {
            calls += 1;
            let mut out = Vec::new();
            out.extend_from_slice(&caps[0]);
            out.push(b'0');
            out
        });
    assert!(result.is_err());
    assert_eq!(calls, 3);
}

#[test]
fn replace_all_bounded_bytes_zero_width_no_infinite_loop() {
    let re = regex::bytes::Regex::new(r"").unwrap();
    let hay = b"abc";
    let result = re.replace_all_bounded(hay, 100, b"-").unwrap();
    assert_eq!(result, &b"-a-b-c-"[..]);
}

#[test]
fn replace_all_bounded_bytes_utf8_counted_in_bytes() {
    let re = regex::bytes::Regex::new(r"(?-u)\xFF").unwrap();
    let hay = b"a\xFF";
    // "a" is 1 byte, "\xFF" is 1 byte. Replacing "\xFF" with "xx" (2 bytes)
    // gives "axx" = 3 bytes.
    assert_eq!(re.replace_all_bounded(hay, 3, b"xx").unwrap(), &b"axx"[..]);
    assert!(re.replace_all_bounded(hay, 2, b"xx").is_err());
}

#[test]
fn replace_all_bounded_bytes_preserves_replace_all() {
    let re = regex::bytes::Regex::new(r"(\w+)").unwrap();
    let hay = b"hello world";
    let expected = re.replace_all(hay, b"[$1]");
    let got = re.replace_all_bounded(hay, 1000, b"[$1]").unwrap();
    assert_eq!(got, expected);
}

#[test]
fn replace_all_bounded_string_empty_haystack() {
    let re = regex::Regex::new("").unwrap();
    let hay = "";
    // The empty regex matches once on an empty haystack.
    assert_eq!(re.replace_all_bounded(hay, 100, "-").unwrap(), "-");
    // Budget of 0: an empty haystack with no match borrows unchanged.
    let re2 = regex::Regex::new("a").unwrap();
    assert_eq!(re2.replace_all_bounded(hay, 0, "b").unwrap(), "");
    // A non-empty result with budget 0 fails.
    assert!(re2.replace_all_bounded("a", 0, "b").is_err());
}

#[test]
fn replace_all_bounded_bytes_empty_haystack() {
    let re = regex::bytes::Regex::new("").unwrap();
    let hay = b"";
    assert_eq!(re.replace_all_bounded(hay, 100, b"-").unwrap(), &b"-"[..]);
    let re2 = regex::bytes::Regex::new("a").unwrap();
    assert_eq!(re2.replace_all_bounded(hay, 0, b"b").unwrap(), &b""[..]);
    assert!(re2.replace_all_bounded(b"a", 0, b"b").is_err());
}

#[test]
fn replace_all_bounded_string_agrees_with_replace_all() {
    // Exhaustively check agreement with `replace_all` over many patterns and
    // replacements, using a generous budget.
    let patterns = ["", "a", "a+", r"\d", r"\w+", r"(\w+)\s+(\w+)"];
    let hays = ["", "a", "aaa", "a1b2c3", "hello world", "  "];
    let replacements = ["", "-", "x", "[$0]", "${1}"];
    for pat in patterns {
        let re = regex::Regex::new(pat).unwrap();
        for hay in hays {
            for rep in replacements {
                let expected = re.replace_all(hay, rep);
                let got = re.replace_all_bounded(hay, 10_000, rep).unwrap();
                assert_eq!(
                    got, expected,
                    "pat={pat:?} hay={hay:?} rep={rep:?}"
                );
            }
        }
    }
}
