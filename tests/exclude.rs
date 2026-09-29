// One-element range slices are used deliberately throughout to exercise the
// `&[Range<usize>]` API.
#![allow(clippy::single_range_in_vec_init)]

use regex::{bytes, ExclusionError, Regex};

/// Helper that returns the byte ranges of every reported match.
fn ranges_str(
    re: &Regex,
    hay: &str,
    excluded: &[core::ops::Range<usize>],
) -> Vec<core::ops::Range<usize>> {
    re.find_iter_excluding(hay, excluded).unwrap().map(|m| m.range()).collect()
}

fn ranges_bytes(
    re: &bytes::Regex,
    hay: &[u8],
    excluded: &[core::ops::Range<usize>],
) -> Vec<core::ops::Range<usize>> {
    re.find_iter_excluding(hay, excluded).unwrap().map(|m| m.range()).collect()
}

// Chinese text: every character occupies 3 UTF-8 bytes.
//
//   你  好  ，  世  界  ！  你  好  ，  世  界  ！
//   0   3   6   9  12  15  18  21  24  27  30  33  36
const ZH: &str = "你好，世界！你好，世界！";

#[test]
fn chinese_pattern_crossing_hidden_range() {
    // `好.世界` matches 好 ， 世 界, i.e. 3..15 and 21..33.
    let re = Regex::new("好.世界").unwrap();

    // Hide the first 世 (9..12). The 3..15 match crosses the hidden range
    // and is discarded; searching resumes and finds 21..33.
    let got = ranges_str(&re, ZH, &[9..12]);
    println!("chinese crossing ranges: {:?}", got);
    assert_eq!(got, vec![21..33]);

    // A pattern that stays inside visible text is unaffected.
    let re = Regex::new("你好").unwrap();
    let got = ranges_str(&re, ZH, &[9..12]);
    println!("chinese visible ranges: {:?}", got);
    assert_eq!(got, vec![0..6, 18..24]);

    // Hide the second half of the text entirely: only 0..3 survives.
    let re = Regex::new("你好").unwrap();
    let got = ranges_str(&re, ZH, &[18..36]);
    assert_eq!(got, vec![0..6]);
}

#[test]
fn anchors_use_whole_haystack() {
    // `^def` (non-multiline) only matches at offset 0 of the *whole*
    // haystack, even if "def" starts a visible segment.
    let re = Regex::new(r"^def").unwrap();
    assert!(ranges_str(&re, "abcdef", &[0..3]).is_empty());

    // `abc$` only matches at the end of the whole haystack.
    let re = Regex::new(r"abc$").unwrap();
    assert!(ranges_str(&re, "abcdef", &[3..6]).is_empty());

    // Positive control: without exclusions the same searches behave as
    // usual.
    let re = Regex::new(r"^abc").unwrap();
    assert_eq!(ranges_str(&re, "abcdef", &[3..6]), vec![0..3]);

    // Multiline `^` still recognizes a line boundary provided by a hidden
    // newline.
    let re = Regex::new(r"(?m)^b$").unwrap();
    assert_eq!(ranges_str(&re, "a\nb", &[0..1]), vec![2..3]);
}

#[test]
fn word_boundary_sees_hidden_context() {
    // The hidden `x` is still a word character, so there is no boundary
    // between it and `bar`. A naive split into "foo" / "bar" would wrongly
    // report the match.
    let re = Regex::new(r"\bbar\b").unwrap();
    assert!(ranges_str(&re, "fooxbar", &[3..4]).is_empty());

    // Boundaries in "ab cd" are at 0, 2, 3, 5. Hiding the space removes
    // the zero-width match at position 2 (which belongs to the half-open
    // range 2..3), while the boundary at position 3 remains because the
    // hidden space still contributes context to the assertion.
    let re = Regex::new(r"\b").unwrap();
    let got = ranges_str(&re, "ab cd", &[2..3]);
    println!("word boundary ranges: {:?}", got);
    assert_eq!(got, vec![0..0, 3..3, 5..5]);
}

#[test]
fn zero_length_matches() {
    // Empty matches of `""` are at every offset.
    let re = Regex::new(r"").unwrap();

    // Hide the single byte 1..2: position 1 is suppressed.
    assert_eq!(ranges_str(&re, "abc", &[1..2]), vec![0..0, 2..2, 3..3]);

    // A zero-width match at the range start belongs to the range and is
    // suppressed; the position just after it is reported.
    assert_eq!(ranges_str(&re, "abc", &[2..3]), vec![0..0, 1..1, 3..3],);

    // Standard empty-match progression is preserved: the empty match at
    // the end of the previous match is forced forward.
    let re = Regex::new(r"a|").unwrap();
    assert_eq!(ranges_str(&re, "aba", &[1..2]), vec![0..1, 2..3]);
}

#[test]
fn adjacent_ranges_stay_separate_and_overlaps_merge() {
    let re = Regex::new(r"").unwrap();

    // Adjacent ranges 2..4 and 4..6 are not merged, so the zero-width
    // match at their shared endpoint 4 is reported.
    let got = ranges_str(&re, "abcdefg", &[2..4, 4..6]);
    println!("adjacent ranges: {:?}", got);
    assert_eq!(got, vec![0..0, 1..1, 4..4, 6..6, 7..7]);

    // Unsorted, overlapping ranges are validated and merged into 1..6.
    let got = ranges_str(&re, "abcdefg", &[4..6, 1..3, 2..5]);
    println!("merged ranges:   {:?}", got);
    assert_eq!(got, vec![0..0, 6..6, 7..7]);

    // Empty ranges hide nothing.
    assert_eq!(ranges_str(&re, "ab", &[1..1]), vec![0..0, 1..1, 2..2],);
}

#[test]
fn invalid_ranges_for_str_are_rejected() {
    let re = Regex::new(r"").unwrap();

    // Each Chinese character is three bytes, so offset 1 splits UTF-8.
    assert_eq!(
        re.find_iter_excluding(ZH, &[1..2]).unwrap_err(),
        ExclusionError::NotCharBoundary { index: 0, offset: 1 },
    );
    assert_eq!(
        re.find_iter_excluding(ZH, &[0..1]).unwrap_err(),
        ExclusionError::NotCharBoundary { index: 0, offset: 1 },
    );
    // Endpoints on character boundaries are accepted.
    assert!(re.find_iter_excluding(ZH, &[0..3, 3..6]).is_ok(),);

    assert_eq!(
        re.find_iter_excluding("abc", &[0..4]).unwrap_err(),
        ExclusionError::OutOfBounds { index: 0, end: 4, haystack_len: 3 },
    );
    // Runtime bounds avoid clippy's `reversed_empty_ranges` lint; the
    // reversed order is exactly the error under test.
    let reversed = usize::from(2u8)..usize::from(1u8);
    assert_eq!(
        re.find_iter_excluding("abc", &[reversed]).unwrap_err(),
        ExclusionError::InvalidRange { index: 0, start: 2, end: 1 },
    );
}

#[test]
fn bytes_api_allows_invalid_utf8_boundaries() {
    // The `&[u8]` API accepts ranges that split UTF-8 sequences.
    let re = bytes::Regex::new(r"(?-u)a").unwrap();
    let hay = b"\xFFa\xFF";
    assert!(ranges_bytes(&re, hay, &[1..2]).is_empty());
    assert_eq!(ranges_bytes(&re, hay, &[0..1]), vec![1..2]);

    // A big match spanning invalid UTF-8 that crosses a hidden range is
    // dropped entirely.
    let re = bytes::Regex::new(r"(?-u)\xFFa\xFF").unwrap();
    assert!(ranges_bytes(&re, hay, &[2..3]).is_empty());
}

#[test]
fn large_match_starting_inside_hidden_region() {
    // Positions: X0 h1 i2 d3 e4 X5 z6 ' '7 X8 o9 k10 z11.
    let hay = "XhideXz Xokz";
    // Hide "XhideX" (0..6). The greedy match X\w*z = 0..7 starts inside
    // the hidden region, so iteration resumes at 6 and finds the later,
    // fully visible match 8..12.
    let re = Regex::new(r"X\w*z").unwrap();
    let got = ranges_str(&re, hay, &[0..6]);
    println!("big match ranges: {:?}", got);
    assert_eq!(got, vec![8..12]);
}

#[test]
fn no_exclusions_matches_normal_iterator() {
    let re = Regex::new(r"\w+|").unwrap();
    let hay = "one two 三";
    let expected: Vec<_> = re.find_iter(hay).map(|m| m.range()).collect();
    let got = ranges_str(&re, hay, &[]);
    assert_eq!(got, expected);
}

/// Deterministic pseudo-random generator (xorshift) for the differential
/// test below.
struct Rng(u64);

impl Rng {
    fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next_u64() % n as u64) as usize
    }
}

/// Reference implementation: take the ordinary iterator's output (which
/// already implements the standard empty-match progression) and drop every
/// match touching an excluded byte. This must agree with the dedicated
/// iterator for a wide variety of inputs.
fn naive(
    re: &Regex,
    hay: &str,
    excluded: &[core::ops::Range<usize>],
) -> Vec<core::ops::Range<usize>> {
    let mut sorted = excluded.to_vec();
    sorted.sort_by_key(|r| r.start);
    // Merge overlapping ranges exactly as the implementation does.
    let mut merged: Vec<core::ops::Range<usize>> = vec![];
    for range in sorted {
        match merged.last_mut() {
            Some(last) if range.start < last.end => {
                if range.end > last.end {
                    last.end = range.end;
                }
            }
            _ => merged.push(range),
        }
    }
    re.find_iter(hay)
        .map(|m| m.range())
        .filter(|m| {
            let visible_zero_width_at = |pos: usize| {
                merged.windows(2).any(|pair| {
                    pair[0].start < pos
                        && pair[0].end == pos
                        && pair[1].start == pos
                        && pair[1].end > pos
                })
            };
            !merged.iter().any(|r| {
                if m.start == m.end {
                    m.start >= r.start
                        && m.start < r.end
                        && !visible_zero_width_at(m.start)
                } else {
                    m.start < r.end && m.end > r.start
                }
            })
        })
        .collect()
}

#[test]
fn matches_naive_filtering_of_standard_iterator() {
    let patterns = [r"", r"a", r"a|", r"\b", r"a*", r"\w+", r"(?m)^."];
    let alphabet = ['a', 'b', ' ', '\n', '中'];
    let mut rng = Rng(0x1234_5678_9abc_def0);

    for pat in patterns {
        let re = Regex::new(pat).unwrap();
        for _ in 0..2000 {
            let len = rng.below(12);
            let mut hay = String::new();
            for _ in 0..len {
                hay.push(alphabet[rng.below(alphabet.len())]);
            }
            let byte_len = hay.len();
            // Character boundaries are the valid range endpoints for `&str`.
            let chars: Vec<usize> = hay
                .char_indices()
                .map(|(i, _)| i)
                .chain(core::iter::once(byte_len))
                .collect();
            let mut excluded = vec![];
            for _ in 0..rng.below(4) {
                let a = chars[rng.below(chars.len())];
                let b = chars[rng.below(chars.len())];
                excluded.push(a.min(b)..a.max(b));
            }
            let expected = naive(&re, &hay, &excluded);
            let got = ranges_str(&re, &hay, &excluded);
            assert_eq!(
                got, expected,
                "pattern={pat:?}, hay={hay:?}, \
                                      excluded={excluded:?}"
            );
        }
    }
}

/// Same differential check for the byte-oriented API, where range
/// endpoints are free to split UTF-8 sequences.
#[test]
fn bytes_matches_naive_filtering_of_standard_iterator() {
    let patterns = [
        r"",
        r"(?-u)a",
        r"(?-u)a|",
        r"(?-u)\b",
        r"(?-u)a*",
        r"(?-u)[\x00-\xff]",
    ];
    let mut rng = Rng(0xdead_beef_cafe_babe);

    for pat in patterns {
        let re = bytes::Regex::new(pat).unwrap();
        for _ in 0..2000 {
            let len = rng.below(10);
            let hay: Vec<u8> =
                (0..len).map(|_| (rng.next_u64() & 0xff) as u8).collect();
            let mut excluded = vec![];
            for _ in 0..rng.below(4) {
                let a = rng.below(hay.len() + 1);
                let b = rng.below(hay.len() + 1);
                excluded.push(a.min(b)..a.max(b));
            }
            let mut merged = excluded.clone();
            merged.sort_by_key(|r| r.start);
            let mut normalized: Vec<core::ops::Range<usize>> = vec![];
            for range in merged {
                match normalized.last_mut() {
                    Some(last) if range.start < last.end => {
                        if range.end > last.end {
                            last.end = range.end;
                        }
                    }
                    _ => normalized.push(range),
                }
            }
            let expected: Vec<_> = re
                .find_iter(&hay)
                .map(|m| m.range())
                .filter(|m| {
                    let visible_zero_width = m.start == m.end
                        && normalized.windows(2).any(|pair| {
                            pair[0].start < m.start
                                && pair[0].end == m.start
                                && pair[1].start == m.start
                                && pair[1].end > m.start
                        });
                    if visible_zero_width {
                        return true;
                    }
                    !normalized.iter().any(|r| {
                        if m.start == m.end {
                            m.start >= r.start && m.start < r.end
                        } else {
                            m.start < r.end && m.end > r.start
                        }
                    })
                })
                .collect();
            let got = ranges_bytes(&re, &hay, &excluded);
            assert_eq!(
                got, expected,
                "pattern={pat:?}, hay={hay:?}, excluded={excluded:?}",
            );
        }
    }
}
