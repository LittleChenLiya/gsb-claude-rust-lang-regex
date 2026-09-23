use regex::{StreamError, StreamFinder};

fn baseline(pattern: &str, text: &str) -> Vec<(u64, u64, String)> {
    regex::Regex::new(pattern)
        .unwrap()
        .find_iter(text)
        .map(|m| (m.start() as u64, m.end() as u64, m.as_str().to_string()))
        .collect()
}

/// Feeds the chunks through a StreamFinder and returns one entry per
/// call: one per feed, plus a final entry for finish.
fn stream_calls(
    pattern: &str,
    chunks: &[&[u8]],
) -> Vec<Vec<(u64, u64, String)>> {
    let mut finder = StreamFinder::new(pattern).unwrap();
    let mut calls = vec![];
    for chunk in chunks {
        let got = finder.feed(chunk).unwrap();
        calls.push(
            got.iter()
                .map(|m| (m.start(), m.end(), m.as_str().to_string()))
                .collect(),
        );
    }
    let got = finder.finish().unwrap();
    calls.push(
        got.iter()
            .map(|m| (m.start(), m.end(), m.as_str().to_string()))
            .collect(),
    );
    calls
}

fn flatten(calls: &[Vec<(u64, u64, String)>]) -> Vec<(u64, u64, String)> {
    calls.iter().flatten().cloned().collect()
}

#[test]
fn greedy_match_not_reported_before_it_is_final() {
    let mut finder = StreamFinder::new(r"ab{1,2}").unwrap();
    assert!(finder.feed(b"a").unwrap().is_empty());
    // "ab" matches here, but a greedy "abb" is still possible, so
    // nothing may be reported yet.
    assert!(finder.feed(b"b").unwrap().is_empty());
    assert!(finder.feed(b"b").unwrap().is_empty());
    let matches = finder.finish().unwrap();
    assert_eq!(matches.len(), 1);
    assert_eq!(matches[0].range(), 0..3);
    assert_eq!(matches[0].as_str(), "abb");
}

#[test]
fn no_premature_match_from_failed_greedy_path() {
    // On the truncated input "ab", the leftmost-first match of this
    // pattern is "a". Only more input can reveal "abc".
    let mut finder = StreamFinder::new(r"a(?:bc)?").unwrap();
    assert!(finder.feed(b"ab").unwrap().is_empty());
    assert!(finder.feed(b"c").unwrap().is_empty());
    let matches = finder.finish().unwrap();
    assert_eq!(matches.len(), 1);
    assert_eq!(matches[0].range(), 0..3);
    assert_eq!(matches[0].as_str(), "abc");
}

#[test]
fn greedy_quantifier_spans_chunks() {
    let calls = stream_calls(r"a{1,3}", &[b"aa", b"aa"]);
    assert!(calls[0].is_empty());
    assert_eq!(calls[1].len(), 1);
    assert_eq!(calls[1][0].0..calls[1][0].1, 0..3);
    assert_eq!(calls[2].len(), 1);
    assert_eq!(calls[2][0].0..calls[2][0].1, 3..4);
}

#[test]
fn multibyte_chars_split_across_chunks() {
    let text = "xé字yé";
    let pattern = "é.";
    let expected = baseline(pattern, text);
    let bytes = text.as_bytes();
    // Split inside é (2 bytes) and inside 字 (3 bytes).
    let chunks: Vec<&[u8]> =
        vec![&bytes[0..2], &bytes[2..3], &bytes[3..6], &bytes[6..]];
    let got = flatten(&stream_calls(pattern, &chunks));
    assert_eq!(expected, got);
}

#[test]
fn empty_pattern_matches_find_iter_byte_by_byte() {
    let text = "aé";
    let expected = baseline("", text);
    assert_eq!(expected.len(), 3);
    let chunks: Vec<&[u8]> =
        text.as_bytes().iter().map(core::slice::from_ref).collect();
    let got = flatten(&stream_calls("", &chunks));
    assert_eq!(expected, got);
}

#[test]
fn dollar_anchor_waits_for_end_of_input() {
    let mut finder = StreamFinder::new(r"a$").unwrap();
    assert!(finder.feed(b"b").unwrap().is_empty());
    // "a" is at the end of the known input: $ cannot be decided yet.
    assert!(finder.feed(b"a").unwrap().is_empty());
    let matches = finder.finish().unwrap();
    assert_eq!(matches.len(), 1);
    assert_eq!(matches[0].range(), 1..2);

    // If more input actually follows, the match must not exist.
    let mut finder = StreamFinder::new(r"a$").unwrap();
    assert!(finder.feed(b"ba").unwrap().is_empty());
    assert!(finder.feed(b"c").unwrap().is_empty());
    assert!(finder.finish().unwrap().is_empty());
}

#[test]
fn multiline_dollar_before_newline_is_prompt() {
    let mut finder = StreamFinder::new(r"(?m)b$").unwrap();
    let got = finder.feed(b"ab\n").unwrap();
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].range(), 1..2);
    assert!(finder.finish().unwrap().is_empty());
}

#[test]
fn unicode_word_boundary_across_chunks() {
    let text = "x éy";
    let pattern = r"\b\w";
    let expected = baseline(pattern, text);
    assert_eq!(expected.len(), 2);
    let bytes = text.as_bytes();
    // Split inside the multi-byte é.
    let chunks: Vec<&[u8]> = vec![&bytes[0..3], &bytes[3..4], &bytes[4..]];
    let got = flatten(&stream_calls(pattern, &chunks));
    assert_eq!(expected, got);
}

#[test]
fn word_boundary_waits_for_next_char() {
    // On "a " there is no boundary after the space yet; the boundary
    // only materializes once "b" arrives.
    let mut finder = StreamFinder::new(r" \b").unwrap();
    assert!(finder.feed(b"a ").unwrap().is_empty());
    let got = finder.feed(b"b").unwrap();
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].range(), 1..2);
    assert!(finder.finish().unwrap().is_empty());
}

#[test]
fn all_chunkings_match_find_iter() {
    let patterns = [
        r"ab{1,2}",
        r"a|b",
        r"",
        r"^a",
        r"a$",
        r"(?m)^a",
        r"(?m)a$",
        r"\b.",
        r"a(?:bc)?",
        r"a{1,3}",
        "é?",
        r"\w\b",
        r"\A.",
        r".\z",
    ];
    let texts = ["ab abb b\nab", "héllo wörld", "a\nb\na", "é字é", "abcb"];
    for &pattern in patterns.iter() {
        for text in texts.iter() {
            let expected = baseline(pattern, text);
            let bytes = text.as_bytes();
            let n = bytes.len();
            // Every possible way to split the text into chunks.
            for mask in 0..(1u64 << (n - 1)) {
                let mut chunks: Vec<&[u8]> = vec![];
                let mut start = 0;
                for i in 0..n - 1 {
                    if mask & (1 << i) != 0 {
                        chunks.push(&bytes[start..=i]);
                        start = i + 1;
                    }
                }
                chunks.push(&bytes[start..]);
                let got = flatten(&stream_calls(pattern, &chunks));
                assert_eq!(
                    expected, got,
                    "pattern {:?} text {:?} chunk mask {:#x}",
                    pattern, text, mask
                );
            }
        }
    }
}

#[test]
fn buffer_stays_bounded_on_long_input() {
    // No matches at all: only the look-back window may be retained.
    let mut finder = StreamFinder::new(r"xay").unwrap();
    let chunk = [b'a'; 8192];
    // Bound: chunk + max_width(3) + look-behind char(<=4) + an
    // incomplete UTF-8 tail(<=3).
    let bound = chunk.len() + 3 + 4 + 3;
    for _ in 0..128 {
        assert!(finder.feed(&chunk).unwrap().is_empty());
        assert!(
            finder.buffered_len() <= bound,
            "buffered {} bytes, bound {}",
            finder.buffered_len(),
            bound
        );
    }
    assert!(finder.finish().unwrap().is_empty());

    // Constant matching: the buffer must stay small as well.
    let mut finder = StreamFinder::new(r"a{1,3}").unwrap();
    let mut total = 0;
    for _ in 0..128 {
        total += finder.feed(&chunk).unwrap().len();
        assert!(finder.buffered_len() <= bound);
    }
    total += finder.finish().unwrap().len();
    let expected = regex::Regex::new(r"a{1,3}")
        .unwrap()
        .find_iter(&"a".repeat(chunk.len() * 128))
        .count();
    assert_eq!(expected, total);
}

#[test]
fn unbounded_patterns_are_rejected_at_construction() {
    let unbounded =
        [r"a+", r"a*", r"a{2,}", r"x|y+", r"(a+)?", r"\w+", r"a{0}b*"];
    for &pattern in unbounded.iter() {
        match StreamFinder::new(pattern) {
            Err(StreamError::UnboundedPattern) => {}
            other => panic!(
                "pattern {:?}: expected UnboundedPattern, got {:?}",
                pattern,
                other.map(|f| f.pattern().to_string())
            ),
        }
    }
    let bounded = [r"", r"a", r"a{2}", r"a{1,3}", r"a{0}", r"(a|b)c{2,5}"];
    for &pattern in bounded.iter() {
        assert!(StreamFinder::new(pattern).is_ok(), "pattern {:?}", pattern);
    }
}

#[test]
fn invalid_pattern_is_a_compile_error() {
    assert!(matches!(StreamFinder::new(r"("), Err(StreamError::Compile(_))));
}

#[test]
fn invalid_utf8_poisons_the_finder() {
    let mut finder = StreamFinder::new(r"a").unwrap();
    assert!(matches!(finder.feed(&[0xFF]), Err(StreamError::InvalidUtf8)));
    assert!(matches!(finder.feed(b"a"), Err(StreamError::Poisoned)));
    assert!(matches!(finder.finish(), Err(StreamError::Poisoned)));
}

#[test]
fn split_utf8_char_is_not_an_error() {
    let mut finder = StreamFinder::new("é").unwrap();
    assert!(finder.feed(&[0xC3]).unwrap().is_empty());
    // The char is complete now, but the match still touches the end of
    // the known input.
    assert!(finder.feed(&[0xA9]).unwrap().is_empty());
    let got = finder.finish().unwrap();
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].range(), 0..2);
    assert_eq!(got[0].as_str(), "é");
}

#[test]
fn incomplete_utf8_at_finish_is_an_error() {
    let mut finder = StreamFinder::new(r"a").unwrap();
    assert!(finder.feed(&[0xC3]).unwrap().is_empty());
    assert!(matches!(finder.finish(), Err(StreamError::InvalidUtf8)));
    assert!(matches!(finder.feed(b"a"), Err(StreamError::Poisoned)));
}

#[test]
fn finish_is_terminal() {
    let mut finder = StreamFinder::new(r"a").unwrap();
    assert!(finder.feed(b"a").unwrap().is_empty());
    assert_eq!(finder.finish().unwrap().len(), 1);
    assert!(matches!(finder.finish(), Err(StreamError::AlreadyFinished)));
    assert!(matches!(finder.feed(b"a"), Err(StreamError::Poisoned)));
}

#[test]
fn feed_after_finish_is_an_error() {
    let mut finder = StreamFinder::new(r"a").unwrap();
    assert!(finder.finish().unwrap().is_empty());
    assert!(matches!(finder.feed(b"a"), Err(StreamError::AlreadyFinished)));
    assert!(matches!(finder.finish(), Err(StreamError::Poisoned)));
}
