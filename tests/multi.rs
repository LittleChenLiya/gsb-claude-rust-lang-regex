use regex::multi::{ChunkedHaystack, MultiMatcher, Rule};
use regex::Error;

/// Collects matches as `(rule id, range)` triples.
fn triples<'a>(
    matcher: &'a MultiMatcher<'a>,
    haystack: &'a str,
) -> Vec<(&'a str, usize, usize)> {
    matcher
        .find_iter(haystack)
        .map(|mat| (mat.id(), mat.start(), mat.end()))
        .collect()
}

#[test]
fn aba_and_ba_overlap() {
    // The motivating log-analysis example: `aba` and `ba` against `ababa`.
    let matcher = MultiMatcher::from_rules([
        Rule::new("aba", "aba").unwrap(),
        Rule::new("ba", "ba").unwrap(),
    ])
    .unwrap();

    let got = triples(&matcher, "ababa");
    assert_eq!(
        got,
        vec![("aba", 0, 3), ("ba", 1, 3), ("aba", 2, 5), ("ba", 3, 5),],
    );
}

#[test]
fn ties_resolve_by_end_then_declaration_order() {
    // At start 0, `a|ab` picks `a` (leftmost-first) while `a` picks `a` too;
    // rule declaration order breaks exact ties.
    let matcher = MultiMatcher::from_rules([
        Rule::new("first", "a").unwrap(),
        Rule::new("second", "a").unwrap(),
        Rule::new("short", "a|ab").unwrap(),
    ])
    .unwrap();

    let got = triples(&matcher, "ab");
    assert_eq!(got, vec![("first", 0, 1), ("second", 0, 1), ("short", 0, 1),],);
}

#[test]
fn every_legal_start_is_enumerated() {
    // Enumeration is anchored at every legal start, so greedy rules also
    // report the shorter match reached from a start inside a longer span.
    let matcher =
        MultiMatcher::from_rules([Rule::new("num", r"\d+").unwrap()]).unwrap();
    assert_eq!(triples(&matcher, "12"), vec![("num", 0, 2), ("num", 1, 2)],);
}

#[test]
fn utf8_boundaries_are_respected() {
    // Rules may only start on UTF-8 scalar boundaries. `a` matches the ASCII
    // byte inside the haystack, and iteration never splits the Chinese
    // characters (each 3 bytes: 中 = E4 B8 AD, 文 = E6 96 87).
    let matcher = MultiMatcher::from_rules([
        Rule::new("han", "中文").unwrap(),
        Rule::new("a", "a").unwrap(),
        Rule::new("empty", "").unwrap(),
    ])
    .unwrap();

    let haystack = "中文a中文";
    let got = triples(&matcher, haystack);
    assert_eq!(
        got,
        vec![
            ("empty", 0, 0),
            ("han", 0, 6),
            ("empty", 3, 3),
            ("empty", 6, 6),
            ("a", 6, 7),
            ("empty", 7, 7),
            ("han", 7, 13),
            ("empty", 10, 10),
            ("empty", 13, 13),
        ],
    );

    // Every reported slice must itself be valid UTF-8.
    for mat in matcher.find_iter(haystack) {
        assert!(core::str::from_utf8(mat.as_str().as_bytes()).is_ok());
    }
}

#[test]
fn empty_matches_at_every_boundary() {
    // An empty rule matches once at every legal start position, including the
    // end of the haystack.
    let matcher =
        MultiMatcher::from_rules([Rule::new("e", "").unwrap()]).unwrap();
    let got: Vec<(usize, usize)> =
        matcher.find_iter("ab").map(|mat| (mat.start(), mat.end())).collect();
    assert_eq!(got, vec![(0, 0), (1, 1), (2, 2)]);
}

#[test]
fn empty_match_mixed_with_nonempty() {
    let matcher = MultiMatcher::from_rules([
        Rule::new("empty", "").unwrap(),
        Rule::new("a", "a").unwrap(),
    ])
    .unwrap();
    let got = triples(&matcher, "a");
    // At start 0: empty match sorts before non-empty; then start 1.
    assert_eq!(got, vec![("empty", 0, 0), ("a", 0, 1), ("empty", 1, 1)],);
}

#[test]
fn captures_are_reported_per_rule() {
    let matcher = MultiMatcher::from_rules([
        Rule::new("kv", r"(\w+)=(\d+)").unwrap(),
        Rule::new("num", r"\d+").unwrap(),
    ])
    .unwrap();

    let mats: Vec<_> = matcher.find_iter("a=12 b=9").collect();
    assert_eq!(
        mats.iter().map(|m| (m.id(), m.range())).collect::<Vec<_>>(),
        vec![
            ("kv", 0..4),
            ("num", 2..4),
            ("num", 3..4),
            ("kv", 5..8),
            ("num", 7..8),
        ],
    );

    let first = mats[0];
    let caps = first.captures();
    assert_eq!(&caps[0], "a=12");
    assert_eq!(&caps[1], "a");
    assert_eq!(&caps[2], "12");
    assert_eq!(caps.get_match().range(), first.range());

    // The reusable-locations API reports the same groups.
    let mut locs = first.rule().regex().capture_locations();
    let overall = first.read_captures(&mut locs);
    assert_eq!(overall.range(), 0..4);
    assert_eq!(locs.get(1), Some((0, 1)));
    assert_eq!(locs.get(2), Some((2, 4)));
    assert_eq!(locs.get(3), None);

    // Named captures resolve through the rule's regex.
    let named = MultiMatcher::from_rules([Rule::new(
        "named",
        r"(?<year>\d{4})-(?<month>\d{2})",
    )
    .unwrap()])
    .unwrap();
    let mat = named.find_iter("2024-07").next().unwrap();
    let caps = mat.captures();
    assert_eq!(&caps["year"], "2024");
    assert_eq!(&caps["month"], "07");
}

#[test]
fn anchors_use_full_haystack_context() {
    // `\A` only matches at offset 0 even though find_at is called at every
    // boundary; `$` and `\b` see the full haystack via the anchored span.
    let matcher = MultiMatcher::from_rules([
        Rule::new("head", r"\A\w+").unwrap(),
        Rule::new("tail", r"\w+$").unwrap(),
        Rule::new("word", r"\b\w+\b").unwrap(),
    ])
    .unwrap();
    let got = triples(&matcher, "hi yo");
    assert_eq!(
        got,
        vec![
            ("head", 0, 2),
            ("word", 0, 2),
            ("tail", 3, 5),
            ("word", 3, 5),
            ("tail", 4, 5),
        ],
    );
}

#[test]
fn duplicate_rule_ids_are_rejected() {
    let mut matcher = MultiMatcher::new();
    matcher.add_pattern("dup", "a").unwrap();
    let err = matcher.add_pattern("dup", "b").unwrap_err();
    match err {
        Error::DuplicateRuleId(id) => assert_eq!(id, "dup"),
        other => panic!("expected DuplicateRuleId, got {other:?}"),
    }

    let err = MultiMatcher::from_rules([
        Rule::new("x", "a").unwrap(),
        Rule::new("x", "b").unwrap(),
    ])
    .unwrap_err();
    assert!(matches!(err, Error::DuplicateRuleId(_)));
}

#[test]
fn invalid_pattern_is_syntax_error() {
    assert!(matches!(Rule::new("x", "("), Err(Error::Syntax(_))));
}

#[test]
fn chunked_reading_matches_one_shot_reading() {
    let full = "中ababa文ba";
    let matcher = MultiMatcher::from_rules([
        Rule::new("aba", "aba").unwrap(),
        Rule::new("ba", "ba").unwrap(),
    ])
    .unwrap();

    let expected = triples(&matcher, full);

    // Split into byte chunks, deliberately inside the 3-byte Han scalars.
    let bytes = full.as_bytes();
    let cuts = [0usize, 1, 2, 3, 5, 8, 9, 12, 13, bytes.len()];
    let mut search = matcher.chunked();
    for window in cuts.windows(2) {
        search.extend(&bytes[window[0]..window[1]]).unwrap();
    }
    let mut results = search.finish().unwrap();
    assert_eq!(results.haystack(), full);
    assert_eq!(results.collect_remaining(), to_ranges(&expected));

    // `&str` chunks concatenate identically.
    let mut search = matcher.chunked();
    for piece in [&full[..4], &full[4..7], &full[7..]] {
        search.extend_str(piece);
    }
    let mut results = search.finish().unwrap();
    assert_eq!(results.collect_remaining(), to_ranges(&expected));

    // The low-level accumulator reconstructs the whole haystack even when a
    // scalar is split across three chunks.
    let mut acc = ChunkedHaystack::new();
    let zhong = "中".as_bytes();
    acc.extend(&zhong[..1]).unwrap();
    acc.extend(&zhong[1..2]).unwrap();
    assert_eq!(acc.as_str(), "");
    acc.extend(&zhong[2..]).unwrap();
    assert_eq!(acc.finish().unwrap(), "中");
}

#[test]
fn chunked_reading_reports_invalid_utf8() {
    let matcher = MultiMatcher::new();
    let mut search = matcher.chunked();
    // 0xFF is never a valid UTF-8 start byte.
    assert!(search.extend(&[0xFF]).is_err());

    let mut acc = ChunkedHaystack::new();
    acc.extend("中".as_bytes()[..2].try_into().unwrap()).unwrap();
    assert!(acc.finish().is_err());
}

fn to_ranges<'a>(
    triples: &'a [(&'a str, usize, usize)],
) -> Vec<(&'a str, core::ops::Range<usize>)> {
    triples.iter().map(|(id, start, end)| (*id, *start..*end)).collect()
}

#[test]
fn empty_matcher_yields_nothing() {
    let matcher = MultiMatcher::new();
    assert_eq!(triples(&matcher, "anything"), vec![]);
}
