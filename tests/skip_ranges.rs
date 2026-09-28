//! Acceptance tests for the skip-ranges finder (regex::skip_ranges).
//!
//! All byte offsets below were computed by hand from the UTF-8 encoding of
//! the haystacks and are asserted exactly.

use regex::skip_ranges::{self, ExcludedRangesError, SkipRanges};
use regex::Regex;

// Hand-computed UTF-8 byte offsets for "你好，世界！hello 世界":
//
//   你    好    ，    世    界    ！    h e l l o  ' '  世    界
//   0..3  3..6  6..9  9..12 12..15 15..18 18..23   23   24..27 27..30
//
// Total length: 30 bytes.
const HAY: &str = "你好，世界！hello 世界";

#[test]
fn chinese_haystack_hand_checked_ranges() {
    // Hide "，世界！" (bytes 6..18), keep everything else.
    let re = Regex::new(r"\w+").unwrap();
    let got = skip_ranges::find_all(&re, HAY, &[6..18]).unwrap();
    // Hand-computed expectation: 你好 [0,6), hello [18,23), 世界 [24,30).
    let expected = vec![0..6, 18..23, 24..30];
    println!("haystack: {:?}", HAY);
    println!("excluded: [6..18)");
    println!("matched byte ranges: {:?}", got);
    println!("hand-computed:       {:?}", expected);
    assert_eq!(got, expected);

    // The matched substrings must be exactly these.
    let texts: Vec<&str> = got.iter().map(|r| &HAY[r.start..r.end]).collect();
    assert_eq!(texts, vec!["你好", "hello", "世界"]);
}

#[test]
fn pattern_crossing_hidden_range_is_dropped_not_split() {
    // The pattern matches across the hidden range on the whole haystack.
    // The match [0,23) intersects the excluded range [6,18), so it must be
    // skipped entirely (and nothing is left after it).
    let re = Regex::new(r"你好.*hello").unwrap();
    let got = skip_ranges::find_all(&re, HAY, &[6..18]).unwrap();
    println!("crossing pattern matches: {:?}", got);
    assert!(got.is_empty());

    // Sanity: without any excluded ranges the same pattern matches [0,23).
    let got = skip_ranges::find_all(&re, HAY, &[]).unwrap();
    assert_eq!(got, vec![0..23]);
}

#[test]
fn anchors_are_judged_against_whole_haystack() {
    // Multi-line ^ and $ still see every line of the whole haystack.
    let hay = "ab\ncd\nef";
    let re = Regex::new(r"(?m)^\w+$").unwrap();
    // Lines: ab [0,2), cd [3,5), ef [6,8). Hide "cd" (bytes 3..5).
    let got = skip_ranges::find_all(&re, hay, &[3..5]).unwrap();
    println!("multiline anchors: {:?}", got);
    assert_eq!(got, vec![0..2, 6..8]);

    // A start-of-text anchor must NOT match at the start of the visible
    // segment that follows a hidden range (it would if we had split the
    // haystack into pieces).
    let re = Regex::new(r"^yy").unwrap();
    let got = skip_ranges::find_all(&re, "xxyy", &[0..2]).unwrap();
    assert!(got.is_empty());

    // End-of-text anchor still works for a match at the very end.
    let re = Regex::new(r"yy$").unwrap();
    let got = skip_ranges::find_all(&re, "xxyy", &[0..2]).unwrap();
    assert_eq!(got, vec![2..4]);
}

#[test]
fn word_boundaries_are_judged_against_whole_haystack() {
    // There is no word boundary between 'o' and 'b' in the whole haystack
    // "foobar", so \bbar\b cannot match even though "bar" is fully visible.
    // Splitting the haystack would have wrongly produced a match.
    let re = Regex::new(r"\bbar\b").unwrap();
    let got = skip_ranges::find_all(&re, "foobar", &[0..3]).unwrap();
    println!(r"\bbar\b with hidden foo: {:?}", got);
    assert!(got.is_empty());

    // Without the boundary assertions the visible "bar" is found.
    let re = Regex::new(r"bar").unwrap();
    let got = skip_ranges::find_all(&re, "foobar", &[0..3]).unwrap();
    assert_eq!(got, vec![3..6]);
}

#[test]
fn zero_length_matches() {
    // x* produces zero-length matches at every position of "abc".
    // With [0,3) excluded, positions strictly inside (1 and 2) are skipped,
    // while positions exactly on the boundary (0 and 3) are kept.
    let re = Regex::new(r"x*").unwrap();
    let got = skip_ranges::find_all(&re, "abc", &[0..3]).unwrap();
    println!("zero-length matches: {:?}", got);
    assert_eq!(got, vec![0..0, 3..3]);

    // Zero-length matches are also kept when nothing is excluded.
    let got = skip_ranges::find_all(&re, "ab", &[]).unwrap();
    assert_eq!(got, vec![0..0, 1..1, 2..2]);

    // Zero-width matches on a Chinese haystack must land on UTF-8
    // boundaries and match the plain find_iter behavior exactly.
    let got = skip_ranges::find_all(&re, HAY, &[]).unwrap();
    let baseline: Vec<_> =
        re.find_iter(HAY).map(|m| m.start()..m.end()).collect();
    println!("zero-length matches on Chinese text: {:?}", got);
    assert_eq!(got, baseline);
    // "你好，世界！hello 世界" has codepoints at 0,3,6,9,12,15,18..23,
    // 24,27,30; all must be char boundaries.
    assert!(got.iter().all(|r| HAY.get(r.start..r.start).is_some()));
}

#[test]
fn adjacent_and_overlapping_ranges_are_merged() {
    let re = Regex::new(r"\w+").unwrap();

    // Adjacent ranges [3,6) and [6,9) merge into [3,9) (好 and ，).
    let it = SkipRanges::new(&re, HAY, &[3..6, 6..9]).unwrap();
    assert_eq!(it.excluded_ranges(), &[3..9]);
    let got: Vec<_> = it.map(|m| m.start()..m.end()).collect();
    println!("adjacent merge matches: {:?}", got);
    // Note: 你好 is a single \w+ match [0,6) on the whole haystack. It
    // intersects the merged hidden range [3,9), so it is dropped whole
    // rather than truncated to the visible prefix 你.
    assert_eq!(got, vec![9..15, 18..23, 24..30]);

    // Overlapping ranges [3,9) and [6,12) merge into [3,12).
    let it = SkipRanges::new(&re, HAY, &[6..12, 3..9]).unwrap();
    assert_eq!(it.excluded_ranges(), &[3..12]);

    // Empty ranges exclude nothing and unordered input is sorted.
    let it = SkipRanges::new(&re, HAY, &[24..30, 3..3, 0..6]).unwrap();
    assert_eq!(it.excluded_ranges(), &[0..6, 24..30]);
}

#[test]
fn invalid_ranges_are_rejected() {
    let re = Regex::new(r"\w+").unwrap();

    // Byte 1 is in the middle of 你 (bytes 0..3).
    let err = skip_ranges::find_all(&re, HAY, &[1..5]).unwrap_err();
    assert_eq!(err, ExcludedRangesError::NotCharBoundary { offset: 1 });

    // Byte 4 is in the middle of 好 (bytes 3..6).
    let err = skip_ranges::find_all(&re, HAY, &[0..4]).unwrap_err();
    assert_eq!(err, ExcludedRangesError::NotCharBoundary { offset: 4 });

    // Reversed range.
    let err = skip_ranges::find_all(&re, HAY, &[5..3]).unwrap_err();
    assert_eq!(err, ExcludedRangesError::Reversed(5..3));

    // Out of bounds (haystack is 30 bytes long).
    let err = skip_ranges::find_all(&re, HAY, &[0..31]).unwrap_err();
    assert_eq!(
        err,
        ExcludedRangesError::OutOfBounds { range: 0..31, haystack_len: 30 }
    );
}

#[test]
fn big_match_starting_in_excluded_range() {
    // A greedy match that starts inside the hidden comment is skipped, and
    // the search recovers at the end of the hidden range.
    let hay = "/*x*/ real";
    // Bytes: / * x * / ' ' r e a l  ->  "/*x*/" is [0,5), "real" is [6,10).
    let re = Regex::new(r"\w+").unwrap();
    let got = skip_ranges::find_all(&re, hay, &[0..5]).unwrap();
    println!("after hidden comment: {:?}", got);
    assert_eq!(got, vec![6..10]);

    // A big greedy match starting inside the excluded range and spanning
    // past its end is dropped wholesale.
    let re = Regex::new(r"a.*c").unwrap();
    let got = skip_ranges::find_all(&re, "aXbXc", &[1..4]).unwrap();
    assert!(got.is_empty());

    // Same, but the whole match is the whole haystack.
    let re = Regex::new(r"/.*l").unwrap();
    let got = skip_ranges::find_all(&re, hay, &[0..5]).unwrap();
    assert!(got.is_empty());
}

#[test]
fn no_excluded_ranges_matches_find_iter() {
    let re = Regex::new(r"\w+").unwrap();
    let got = skip_ranges::find_all(&re, HAY, &[]).unwrap();
    let baseline: Vec<_> =
        re.find_iter(HAY).map(|m| m.start()..m.end()).collect();
    println!("no exclusions: {:?}", got);
    assert_eq!(got, baseline);
    assert_eq!(got, vec![0..6, 9..15, 18..23, 24..30]);
}
