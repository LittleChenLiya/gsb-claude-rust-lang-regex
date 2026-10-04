/*!
Targeted tests for windowed regex searches.

A "windowed" search restricts the *complete match* to a byte range of the
haystack while the engine still sees the full haystack as boundary context.
All reported offsets must remain absolute offsets into the original
haystack. See the `WindowError` docs for details.
*/

use regex::{bytes::Regex as BytesRegex, Regex, WindowError};

// Acceptance: "xab", window [1, 3), `^ab` does not match while `ab` matches
// and is reported at [1, 3).
#[test]
fn window_anchor_uses_whole_haystack() {
    let hay = "xab";

    let anchored = Regex::new("^ab").unwrap();
    assert_eq!(None, anchored.find_in(hay, 1..3).unwrap());
    assert!(!anchored.is_match_in(hay, 1..3).unwrap());

    let unanchored = Regex::new("ab").unwrap();
    let m = unanchored.find_in(hay, 1..3).unwrap().unwrap();
    assert_eq!(1..3, m.range());
    assert_eq!("ab", m.as_str());
    assert!(unanchored.is_match_in(hay, 1..3).unwrap());

    // Slicing the haystack would have given a match for `^ab`; the window
    // API deliberately avoids that re-basing.
    assert!(anchored.is_match(&hay[1..]));
}

// `$` and multiline anchors are resolved against the whole haystack, not
// against the window edges.
#[test]
fn window_dollar_and_multiline_anchors() {
    let hay = "ab\ncd";

    let dollar = Regex::new(r"cd$").unwrap();
    assert_eq!(
        Some(3..5),
        dollar.find_in(hay, 3..5).unwrap().map(|m| m.range())
    );
    let dollar = Regex::new(r"ab$").unwrap();
    assert_eq!(None, dollar.find_in(hay, 0..2).unwrap());

    // (?m)^ matches at offset 3 (start of the second line), which is the
    // start of the window, but not the start of the haystack.
    let mline = Regex::new(r"(?m)^cd").unwrap();
    let m = mline.find_in(hay, 3..5).unwrap().unwrap();
    assert_eq!(3..5, m.range());
}

// Acceptance: "a猫b" (a=0, 猫=1..4, b=4); a window that splits the UTF-8
// encoding of the cat is rejected with a range error.
#[test]
fn window_rejects_non_char_boundary() {
    let hay = "a猫b";
    assert_eq!(5, hay.len());
    let re = Regex::new(".").unwrap();

    // The start at 2 lies inside the cat code point.
    let err = re.find_in(hay, 2..4).unwrap_err();
    assert_eq!(WindowError::NotCharBoundary { offset: 2 }, err);
    // The end at 3 also lies inside it.
    let err = re.find_in(hay, 1..3).unwrap_err();
    assert_eq!(WindowError::NotCharBoundary { offset: 3 }, err);
    // Both bounds on complete code points: accepted.
    assert!(re.find_in(hay, 1..4).is_ok());
    // The cat itself may be found via a window covering exactly its bytes.
    let cat = Regex::new("猫").unwrap();
    let m = cat.find_in(hay, 1..4).unwrap().unwrap();
    assert_eq!(1..4, m.range());
    assert_eq!("猫", m.as_str());
}

// Out-of-bounds and inverted ranges are rejected before searching.
#[test]
fn window_rejects_out_of_bounds() {
    let hay = "abc";
    let re = Regex::new(".").unwrap();

    assert_eq!(
        WindowError::OutOfBounds { haystack_len: 3, start: 0, end: 4 },
        re.find_in(hay, 0..4).unwrap_err(),
    );
    // Construct the inverted range at runtime to avoid the
    // `reversed_empty_ranges` lint; the window API must reject it.
    let inverted = {
        let (start, end) = (3, 2);
        start..end
    };
    assert_eq!(
        WindowError::OutOfBounds { haystack_len: 3, start: 3, end: 2 },
        re.find_in(hay, inverted).unwrap_err(),
    );
    // The full range, the empty window at the end and unbounded ends are all
    // valid.
    assert!(re.find_in(hay, ..).is_ok());
    assert!(re.find_in(hay, 0..3).is_ok());
    assert!(re.find_in(hay, 3..3).is_ok());
    assert!(re.find_in(hay, 2..).is_ok());
    assert!(re.find_in(hay, ..2).is_ok());
}

// The word boundary at a window edge uses the full haystack as context.
#[test]
fn window_word_boundary_sees_outside_context() {
    // "ab" starts at offset 1; the byte before it is a word byte, so the
    // word-boundary pattern cannot match even though the window begins there.
    let hay = "aab";
    let re = Regex::new(r"\bab\b").unwrap();
    assert_eq!(None, re.find_in(hay, 1..3).unwrap());

    // "ab" preceded by a space at offset 0: the boundary holds even though
    // the space is outside the window.
    let hay = " ab";
    let m = re.find_in(hay, 1..3).unwrap().unwrap();
    assert_eq!(1..3, m.range());

    // Trailing context is visible too: the window excludes the final byte.
    let hay = "abc";
    let re = Regex::new(r"\bab\b").unwrap();
    assert_eq!(None, re.find_in(hay, 0..2).unwrap());
    let hay = "ab ";
    let m = re.find_in(hay, 0..2).unwrap().unwrap();
    assert_eq!(0..2, m.range());
}

// Empty matches inside a window follow the standard non-overlapping
// iteration rules, including the empty match at the end of the window and
// the forced 1-byte advance.
#[test]
fn window_empty_match_iteration_string() {
    let hay = "abab";

    // The empty pattern yields empty matches at every position inside the
    // window, including the end boundary.
    let empty = Regex::new("").unwrap();
    let got: Vec<_> =
        empty.find_iter_in(hay, 1..3).unwrap().map(|m| m.range()).collect();
    assert_eq!(vec![1..1, 2..2, 3..3], got);

    // a| against [1, 4) follows the crate-wide empty-match rule: an empty
    // match that begins where the preceding match ended is discarded and the
    // search resumes one byte later. Thus the first result is the empty
    // match at 1, then the non-empty a at 2, and finally the trailing empty
    // match at the window end (4).
    let a_or_empty = Regex::new("a|").unwrap();
    let got: Vec<_> = a_or_empty
        .find_iter_in(hay, 1..4)
        .unwrap()
        .map(|m| m.range())
        .collect();
    assert_eq!(vec![1..1, 2..3, 4..4], got);
}

// Captures reported through a window carry absolute original offsets.
#[test]
fn window_capture_offsets_string() {
    let hay = "x(ab)y";
    let re = Regex::new(r"\((a)(b)\)").unwrap();
    let caps = re.captures_in(hay, 1..5).unwrap().unwrap();
    assert_eq!(1..5, caps.get(0).unwrap().range());
    assert_eq!(2..3, caps.get(1).unwrap().range());
    assert_eq!(3..4, caps.get(2).unwrap().range());

    let mut locs = re.capture_locations();
    let m = re.captures_read_in(&mut locs, hay, 1..5).unwrap().unwrap();
    assert_eq!(1..5, m.range());
    assert_eq!(Some((2, 3)), locs.get(1));
    assert_eq!(Some((3, 4)), locs.get(2));
}

// Iterating captures over a window stops at the window end while keeping
// original coordinates.
#[test]
fn window_captures_iter_string() {
    let hay = "a1 a2 a3";
    let re = Regex::new(r"a(\d)").unwrap();
    let got: Vec<_> = re
        .captures_iter_in(hay, 0..6)
        .unwrap()
        .map(|caps| {
            (caps.get(0).unwrap().range(), caps.get(1).unwrap().range())
        })
        .collect();
    assert_eq!(vec![(0..2, 1..2), (3..5, 4..5)], got);
}

// --- bytes mode -----------------------------------------------------------

#[test]
fn window_bytes_anchor_and_match() {
    let hay = b"xab";
    let anchored = BytesRegex::new("^ab").unwrap();
    assert_eq!(None, anchored.find_in(hay, 1..3).unwrap());
    let plain = BytesRegex::new("ab").unwrap();
    let m = plain.find_in(hay, 1..3).unwrap().unwrap();
    assert_eq!(1..3, m.range());
    assert_eq!(b"ab", m.as_bytes());
}

#[test]
fn window_bytes_allows_arbitrary_byte_bounds() {
    // Byte windows may split UTF-8 code points: only bounds checking applies.
    let hay = "a猫b".as_bytes();
    // 猫 is encoded as E7 8C AB, so byte 2 is 0x8C.
    let re = BytesRegex::new(r"(?-u).").unwrap();
    let m = re.find_in(hay, 2..3).unwrap().unwrap();
    assert_eq!(2..3, m.range());
    assert_eq!(&[0x8C], m.as_bytes());

    // But out-of-bounds windows are still rejected.
    let err = re.find_in(hay, 2..99).unwrap_err();
    assert_eq!(
        WindowError::OutOfBounds { haystack_len: 5, start: 2, end: 99 },
        err,
    );
}

#[test]
fn window_bytes_word_boundary_context() {
    let hay = b"aab ab";
    let re = BytesRegex::new(r"\bab\b").unwrap();
    // At [1, 3): preceded by a word byte -> no boundary.
    assert_eq!(None, re.find_in(hay, 1..3).unwrap());
    // At [4, 6): preceded by a space -> boundary holds.
    let m = re.find_in(hay, 4..6).unwrap().unwrap();
    assert_eq!(4..6, m.range());
}

#[test]
fn window_bytes_empty_match_iteration() {
    let hay = b"abab";
    let empty = BytesRegex::new("").unwrap();
    let got: Vec<_> =
        empty.find_iter_in(hay, 1..3).unwrap().map(|m| m.range()).collect();
    assert_eq!(vec![1..1, 2..2, 3..3], got);

    let a_or_empty = BytesRegex::new("a|").unwrap();
    let got: Vec<_> = a_or_empty
        .find_iter_in(hay, 1..4)
        .unwrap()
        .map(|m| m.range())
        .collect();
    assert_eq!(vec![1..1, 2..3, 4..4], got);
}

#[test]
fn window_bytes_capture_offsets() {
    let hay = b"x(ab)y";
    let re = BytesRegex::new(r"\((a)(b)\)").unwrap();
    let caps = re.captures_in(hay, 1..5).unwrap().unwrap();
    assert_eq!(1..5, caps.get(0).unwrap().range());
    assert_eq!(2..3, caps.get(1).unwrap().range());
    assert_eq!(3..4, caps.get(2).unwrap().range());

    let got: Vec<_> = re
        .captures_iter_in(b"x(ab)y(ab)z", 1..10)
        .unwrap()
        .map(|caps| {
            (caps.get(0).unwrap().range(), caps.get(1).unwrap().range())
        })
        .collect();
    assert_eq!(vec![(1..5, 2..3), (6..10, 7..8)], got);
}

// A window may be completely empty; behavior matches searching at a single
// offset in the original haystack.
#[test]
fn window_empty_window() {
    let hay = "ab";
    let re = Regex::new("").unwrap();
    let got: Vec<_> =
        re.find_iter_in(hay, 1..1).unwrap().map(|m| m.range()).collect();
    assert_eq!(vec![1..1], got);

    let non_empty = Regex::new("a").unwrap();
    assert_eq!(None, non_empty.find_in(hay, 1..1).unwrap());
}

// A match that starts inside the window but would extend past its end must
// not be reported; the window bounds the *complete* match.
#[test]
fn window_match_must_be_fully_inside() {
    let hay = "abcdef";
    let re = Regex::new("abc").unwrap();
    assert_eq!(Some(0..3), re.find_in(hay, 0..3).unwrap().map(|m| m.range()));
    assert_eq!(None, re.find_in(hay, 0..2).unwrap());
    assert_eq!(None, re.find_in(hay, 1..3).unwrap());

    // Only a shorter alternative within the window matches.
    let re = Regex::new("abc|b").unwrap();
    let m = re.find_in(hay, 1..3).unwrap().unwrap();
    assert_eq!(1..2, m.range());

    // Same guarantee for captures iteration.
    let re = Regex::new("(ab)(c)").unwrap();
    assert!(re.captures_in(hay, 0..2).unwrap().is_none());

    // And for the bytes API.
    let re = BytesRegex::new("abc").unwrap();
    assert_eq!(None, re.find_in(hay.as_bytes(), 0..2).unwrap());
}

// Resolved bounds from any RangeBounds type behave like concrete ranges.
#[test]
fn window_range_bounds_shapes() {
    let hay = "abcdef";
    let re = Regex::new("c").unwrap();
    assert_eq!(Some(2..3), re.find_in(hay, 2..=2).unwrap().map(|m| m.range()));
    assert_eq!(Some(2..3), re.find_in(hay, ..3).unwrap().map(|m| m.range()));
    assert_eq!(Some(2..3), re.find_in(hay, 2..).unwrap().map(|m| m.range()));
    assert_eq!(Some(2..3), re.find_in(hay, ..).unwrap().map(|m| m.range()));
}

// A full-haystack window is observationally identical to the ordinary,
// non-windowed search entry points (including empty-match iteration).
#[test]
fn window_full_range_equals_plain_search() {
    for pat in ["", "a", "a|", r"\w+", "(a)(b)?"] {
        let re = Regex::new(pat).unwrap();
        for hay in ["", "abab", "a猫b猫c"] {
            let plain: Vec<_> = re.find_iter(hay).map(|m| m.range()).collect();
            let windowed: Vec<_> = re
                .find_iter_in(hay, 0..hay.len())
                .unwrap()
                .map(|m| m.range())
                .collect();
            assert_eq!(plain, windowed, "pat={pat:?} hay={hay:?}");

            let plain = re.find(hay).map(|m| m.range());
            let windowed = re.find_in(hay, ..).unwrap().map(|m| m.range());
            assert_eq!(plain, windowed, "pat={pat:?} hay={hay:?}");
        }
    }
}
