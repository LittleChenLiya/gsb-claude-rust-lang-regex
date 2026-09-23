use regex::{Regex, StreamError, StreamingFinder};

type M = (u64, u64, String);

fn expected(pattern: &str, text: &str) -> Vec<M> {
    Regex::new(pattern)
        .unwrap()
        .find_iter(text)
        .map(|m| (m.start() as u64, m.end() as u64, m.as_str().to_string()))
        .collect()
}

fn to_tuples(ms: Vec<regex::StreamMatch>) -> Vec<M> {
    ms.into_iter().map(|m| (m.start, m.end, m.text)).collect()
}

/// Feeds every chunk and returns (per-feed outputs, finish output).
fn run(pattern: &str, chunks: &[&[u8]]) -> (Vec<Vec<M>>, Vec<M>) {
    let mut f = StreamingFinder::new(pattern).unwrap();
    let mut per_feed = vec![];
    for c in chunks {
        per_feed.push(to_tuples(f.feed(c).unwrap()));
    }
    let fin = to_tuples(f.finish().unwrap());
    (per_feed, fin)
}

fn run_total(pattern: &str, chunks: &[&[u8]]) -> Vec<M> {
    let (per_feed, fin) = run(pattern, chunks);
    let mut all: Vec<M> = per_feed.into_iter().flatten().collect();
    all.extend(fin);
    all
}

#[test]
fn greedy_match_waits_for_all_chunks() {
    // `abb` fed as `a`, `b`, `b` must yield exactly one match, `abb`,
    // and only at finish. Reporting `ab` early would be wrong.
    let (per_feed, fin) = run(r"ab{1,2}", &[b"a", b"b", b"b"]);
    assert!(per_feed.iter().all(|m| m.is_empty()), "{:?}", per_feed);
    assert_eq!(fin, vec![(0, 3, "abb".to_string())]);
}

#[test]
fn no_premature_short_match() {
    // Greedy repetition must extend as far as possible.
    let (per_feed, fin) = run(r"a{1,3}", &[b"a", b"a", b"a"]);
    assert!(per_feed.iter().all(|m| m.is_empty()), "{:?}", per_feed);
    assert_eq!(fin, vec![(0, 3, "aaa".to_string())]);

    // When the input really is over, the shorter match is correct.
    let (per_feed, fin) = run(r"ab{1,2}", &[b"a", b"b"]);
    assert!(per_feed.iter().all(|m| m.is_empty()), "{:?}", per_feed);
    assert_eq!(fin, vec![(0, 2, "ab".to_string())]);

    // Alternation order must not be committed before the input ends.
    let (per_feed, fin) = run(r"a|ab", &[b"a", b"b"]);
    assert!(per_feed.iter().all(|m| m.is_empty()), "{:?}", per_feed);
    assert_eq!(fin, expected(r"a|ab", "ab"));
}

#[test]
fn multibyte_char_split_across_chunks() {
    let text = "xh\u{e9}llo y\u{e9}";
    let pattern = r"\w{1,4}";
    let b = text.as_bytes();
    // Split inside the two-byte \u{e9} (0xC3 0xA9).
    let e9 = text.find('\u{e9}').unwrap();
    assert_eq!(&b[e9..e9 + 2], &[0xC3, 0xA9]);
    let chunks: &[&[u8]] =
        &[&b[..e9], &b[e9..e9 + 1], &b[e9 + 1..e9 + 3], &b[e9 + 3..]];
    assert_eq!(run_total(pattern, chunks), expected(pattern, text));
}

#[test]
fn empty_pattern_with_multibyte() {
    let text = "a\u{e9}";
    let chunks: &[&[u8]] = &[b"a", b"\xC3", b"\xA9"];
    assert_eq!(run_total("", chunks), expected("", text));
    // Empty matches land on codepoint boundaries only: 0, 1, 3.
    assert_eq!(
        run_total("", chunks),
        vec![
            (0, 0, String::new()),
            (1, 1, String::new()),
            (3, 3, String::new()),
        ]
    );
}

#[test]
fn dollar_end_anchor_waits_for_finish() {
    // `b$` cannot be decided while more input may arrive.
    let (per_feed, fin) = run(r"b$", &[b"a", b"b"]);
    assert!(per_feed.iter().all(|m| m.is_empty()), "{:?}", per_feed);
    assert_eq!(fin, vec![(1, 2, "b".to_string())]);

    // And it must not match when more input actually does arrive.
    assert_eq!(run_total(r"b$", &[b"a", b"b", b"c"]), Vec::new());
}

#[test]
fn unicode_word_boundary_across_chunks() {
    let text = "a\u{e9} b \u{4e16}\u{754c}";
    let pattern = r"\b";
    let b = text.as_bytes();
    // Feed one byte at a time to force splits inside every codepoint.
    let chunks: Vec<&[u8]> = b.chunks(1).collect();
    assert_eq!(run_total(pattern, &chunks), expected(pattern, text));

    let pattern = r"\b\w{1,3}\b";
    assert_eq!(run_total(pattern, &chunks), expected(pattern, text));
}

#[test]
fn exhaustive_chunkings_match_find_iter() {
    let cases: &[(&str, &str)] = &[
        (r"ab{1,2}", "abbbab"),
        (r"ab{1,2}", "xabbay"),
        (r"a{1,3}", "aaaa"),
        ("", "a\u{e9}b"),
        (r"b$", "abb"),
        (r"a\z", "ba"),
        (r"a\z|b", "ba"),
        (r"^a", "aab"),
        (r"(?m)^b", "ab\nb"),
        (r"(?m)a$", "a\na"),
        (r"\b", "a\u{e9} b"),
        (r"\b\w{1,2}\b", "a\u{e9} bc"),
        (r"a|b", "abab"),
        (r"a|ab", "aab"),
        (r"x?y", "xyy"),
        (r"a{2}", "aaaa"),
        (r"\d{1,2}", "a12b3"),
        (r"\u{e9}{1,2}", "\u{e9}\u{e9}\u{e9}"),
        (r"\Aab", "ab"),
        (r"\B\w", "ab c"),
        (r"[ab]{1,2}", "abba"),
        (r"z", "aaa"),
    ];
    for &(pattern, text) in cases {
        let want = expected(pattern, text);
        let n = text.len();
        assert!(n <= 12, "case too long for exhaustive chunking");
        // Iterate over every possible way to split the raw bytes into
        // chunks, including splits inside multi-byte codepoints.
        for mask in 0..(1u32 << n.saturating_sub(1)) {
            let mut chunks: Vec<&[u8]> = vec![];
            let mut start = 0;
            for i in 1..n {
                if mask & (1 << (i - 1)) != 0 {
                    chunks.push(&text.as_bytes()[start..i]);
                    start = i;
                }
            }
            chunks.push(&text.as_bytes()[start..]);
            let got = run_total(pattern, &chunks);
            assert_eq!(
                got, want,
                "pattern {:?} text {:?} chunks {:?}",
                pattern, text, chunks
            );
        }
    }
}

#[test]
fn buffer_stays_bounded_on_long_input() {
    // No matches at all: buffer must not accumulate the input.
    let mut f = StreamingFinder::new(r"ab{1,2}").unwrap();
    let mut max_buf = 0;
    let chunk = [b'a'; 4096];
    for _ in 0..64 {
        f.feed(&chunk).unwrap();
        max_buf = max_buf.max(f.buffered_len());
    }
    assert!(f.finish().unwrap().is_empty());
    assert!(max_buf <= 16, "max_buf = {}", max_buf);

    // Dense matches: buffer must stay bounded too.
    let mut f = StreamingFinder::new(r"ab{1,2}").unwrap();
    let mut max_buf = 0;
    let mut total = 0;
    for _ in 0..100_000 {
        total += f.feed(b"ab").unwrap().len();
        max_buf = max_buf.max(f.buffered_len());
    }
    total += f.finish().unwrap().len();
    assert_eq!(total, 100_000);
    assert!(max_buf <= 16, "max_buf = {}", max_buf);

    // Multi-byte pattern fed one byte at a time.
    let mut f = StreamingFinder::new("\u{e9}{1,3}x").unwrap();
    let mut max_buf = 0;
    let text = "\u{e9}\u{e9}x".repeat(10_000);
    let mut total = 0;
    for b in text.as_bytes() {
        total += f.feed(&[*b]).unwrap().len();
        max_buf = max_buf.max(f.buffered_len());
    }
    total += f.finish().unwrap().len();
    assert_eq!(total, 10_000);
    assert!(max_buf <= 24, "max_buf = {}", max_buf);

    // An end-anchored alternative must not force unbounded retention.
    let mut f = StreamingFinder::new(r"a\z|b").unwrap();
    let mut max_buf = 0;
    for _ in 0..100_000 {
        f.feed(b"a").unwrap();
        max_buf = max_buf.max(f.buffered_len());
    }
    assert_eq!(f.finish().unwrap().len(), 1);
    assert!(max_buf <= 16, "max_buf = {}", max_buf);
}

#[test]
fn unbounded_patterns_fail_at_construction() {
    for pattern in
        [r"a+", r"a*", r"a{2,}", r"\w+", r"a|b+", r"(ab)*", r"a{0,3}b*"]
    {
        match StreamingFinder::new(pattern) {
            Err(StreamError::Unbounded) => {}
            x => panic!(
                "pattern {:?}: expected Unbounded, got {:?}",
                pattern, x
            ),
        }
    }
    // Bounded patterns are accepted.
    for pattern in [r"ab{1,2}", "", r"\b", r"a$", r"\w{1,5}"] {
        assert!(StreamingFinder::new(pattern).is_ok(), "{:?}", pattern);
    }
}

#[test]
fn invalid_utf8_poisons_finder() {
    let mut f = StreamingFinder::new(r"ab{1,2}").unwrap();
    assert!(f.feed(b"ab").is_ok());
    match f.feed(b"\xFF") {
        Err(StreamError::InvalidUtf8) => {}
        x => panic!("expected InvalidUtf8, got {:?}", x),
    }
    match f.feed(b"b") {
        Err(StreamError::Poisoned) => {}
        x => panic!("expected Poisoned, got {:?}", x),
    }
    match f.finish() {
        Err(StreamError::Poisoned) => {}
        x => panic!("expected Poisoned, got {:?}", x),
    }
}

#[test]
fn truncated_codepoint_at_finish_is_invalid() {
    let mut f = StreamingFinder::new(r"ab{1,2}").unwrap();
    assert!(f.feed(b"\xC3").is_ok());
    match f.finish() {
        Err(StreamError::InvalidUtf8) => {}
        x => panic!("expected InvalidUtf8, got {:?}", x),
    }
}

#[test]
fn feed_after_finish_and_double_finish_are_errors() {
    let mut f = StreamingFinder::new(r"ab{1,2}").unwrap();
    assert!(f.feed(b"ab").is_ok());
    assert_eq!(f.finish().unwrap().len(), 1);
    match f.feed(b"ab") {
        Err(StreamError::AlreadyFinished) => {}
        x => panic!("expected AlreadyFinished, got {:?}", x),
    }
    // The error poisons the finder.
    match f.finish() {
        Err(StreamError::Poisoned) => {}
        x => panic!("expected Poisoned, got {:?}", x),
    }

    let mut f = StreamingFinder::new(r"ab{1,2}").unwrap();
    assert!(f.finish().is_ok());
    match f.finish() {
        Err(StreamError::AlreadyFinished) => {}
        x => panic!("expected AlreadyFinished, got {:?}", x),
    }
    match f.feed(b"a") {
        Err(StreamError::Poisoned) => {}
        x => panic!("expected Poisoned, got {:?}", x),
    }
}
