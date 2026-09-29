use regex::{MultiRule, MultiRuleMatch};

fn hits<I: Clone>(
    rules: &MultiRule<I>,
    haystack: &str,
) -> Vec<MultiRuleMatch<I>> {
    rules.find_iter(haystack).collect()
}

fn spans<I>(hits: &[MultiRuleMatch<I>]) -> Vec<(usize, usize, usize)> {
    hits.iter().map(|m| (m.start(), m.end(), m.rule_index())).collect()
}

#[test]
fn overlapping_hits_are_all_reported() {
    let rules = MultiRule::new([("R1", "aba"), ("R2", "ba")]).unwrap();
    let hay = "ababa";
    let got: Vec<(usize, usize, &str, &str)> = rules
        .find_iter(hay)
        .map(|m| (m.start(), m.end(), *m.id(), m.as_str(hay)))
        .collect();
    assert_eq!(
        got,
        vec![
            (0, 3, "R1", "aba"),
            (1, 3, "R2", "ba"),
            (2, 5, "R1", "aba"),
            (3, 5, "R2", "ba"),
        ]
    );
    // Every expected overlapping position is enumerable.
    let got = spans(&hits(&rules, hay));
    assert!(got.contains(&(0, 3, 0)));
    assert!(got.contains(&(2, 5, 0)));
    assert!(got.contains(&(1, 3, 1)));
    assert!(got.contains(&(3, 5, 1)));
}

#[test]
fn hits_are_sorted_by_span_then_declaration_order() {
    // Declaration order is the tie-breaker for identical spans.
    let rules = MultiRule::new([("z", "ba"), ("a", "aba")]).unwrap();
    let got: Vec<&str> =
        hits(&rules, "ababa").iter().map(|m| *m.id()).collect();
    assert_eq!(got, vec!["a", "z", "a", "z"]);

    // Shorter matches at the same start position come first.
    let rules = MultiRule::new([("long", "aba"), ("short", "ab")]).unwrap();
    let got = spans(&hits(&rules, "aba"));
    assert_eq!(got, vec![(0, 2, 1), (0, 3, 0)]);
}

#[test]
fn duplicate_patterns_each_report_their_hits() {
    let rules = MultiRule::new([("first", "aba"), ("second", "aba")]).unwrap();
    let got: Vec<(usize, usize, &str)> = hits(&rules, "ababa")
        .iter()
        .map(|m| (m.start(), m.end(), *m.id()))
        .collect();
    assert_eq!(
        got,
        vec![
            (0, 3, "first"),
            (0, 3, "second"),
            (2, 5, "first"),
            (2, 5, "second"),
        ]
    );
}

#[test]
fn multibyte_char_boundaries() {
    let hay = "a中b文c";
    let rules = MultiRule::new([("dot", "."), ("han", "中|文")]).unwrap();
    let got: Vec<(usize, usize, &str, &str)> = rules
        .find_iter(hay)
        .map(|m| (m.start(), m.end(), *m.id(), m.as_str(hay)))
        .collect();
    assert_eq!(
        got,
        vec![
            (0, 1, "dot", "a"),
            (1, 4, "dot", "中"),
            (1, 4, "han", "中"),
            (4, 5, "dot", "b"),
            (5, 8, "dot", "文"),
            (5, 8, "han", "文"),
            (8, 9, "dot", "c"),
        ]
    );
    // Every hit range is a valid char boundary slice.
    for m in hits(&rules, hay) {
        assert!(hay.is_char_boundary(m.start()));
        assert!(hay.is_char_boundary(m.end()));
    }
}

#[test]
fn empty_matches_respect_char_boundaries() {
    let rules = MultiRule::new([("empty", "")]).unwrap();
    let got = spans(&hits(&rules, "中文"));
    assert_eq!(got, vec![(0, 0, 0), (3, 3, 0), (6, 6, 0)]);
}

#[test]
fn empty_matches_terminate_and_overlap() {
    let rules = MultiRule::new([("star", "a*")]).unwrap();
    let got = spans(&hits(&rules, "baab"));
    assert_eq!(
        got,
        vec![(0, 0, 0), (1, 3, 0), (2, 3, 0), (3, 3, 0), (4, 4, 0)]
    );

    // An empty haystack still yields the empty match.
    let got = spans(&hits(&rules, ""));
    assert_eq!(got, vec![(0, 0, 0)]);
}

#[test]
fn captures_are_available_per_hit() {
    let rules = MultiRule::new([("pair", r"(\d+)-(\d+)")]).unwrap();
    let hay = "1-22 333-4";
    let got = hits(&rules, hay);
    assert_eq!(
        spans(&got),
        vec![(0, 4, 0), (5, 10, 0), (6, 10, 0), (7, 10, 0)]
    );

    let caps = rules.captures(&got[0], hay).unwrap();
    assert_eq!(&caps[0], "1-22");
    assert_eq!(&caps[1], "1");
    assert_eq!(&caps[2], "22");

    let caps = rules.captures(&got[1], hay).unwrap();
    assert_eq!(&caps[0], "333-4");
    assert_eq!(&caps[1], "333");
    assert_eq!(&caps[2], "4");
}

#[test]
fn anchors_are_respected() {
    let rules =
        MultiRule::new([("start", "^a"), ("line", "(?m)^b"), ("end", "a$")])
            .unwrap();
    let hay = "ab\nba\na";
    let got: Vec<(usize, usize, &str)> = hits(&rules, hay)
        .iter()
        .map(|m| (m.start(), m.end(), *m.id()))
        .collect();
    assert_eq!(got, vec![(0, 1, "start"), (3, 4, "line"), (6, 7, "end")]);
}

#[test]
fn segmented_reading_equals_oneshot() {
    let rules =
        MultiRule::new([("R1", "aba"), ("R2", "ba"), ("han", "文")]).unwrap();
    let hay = "ababa文ab文a";
    let oneshot = hits(&rules, hay);
    assert!(!oneshot.is_empty());

    let segmentations: Vec<Vec<&str>> = vec![
        vec!["ababa文ab文a"],
        vec!["a", "b", "a", "b", "a", "文", "a", "b", "文", "a"],
        vec!["ab", "aba", "文a", "b文a"],
        vec!["", "ababa文", "ab文a", ""],
    ];
    for segments in segmentations {
        let mut reader = rules.reader();
        for segment in segments {
            reader.push(segment);
        }
        assert_eq!(reader.haystack(), hay);
        assert_eq!(reader.finish(), oneshot);
    }
}

#[test]
fn misc_edge_cases() {
    // No rules means no hits.
    let rules: MultiRule<&str> =
        MultiRule::new(Vec::<(&str, &str)>::new()).unwrap();
    assert!(rules.is_empty());
    assert_eq!(rules.len(), 0);
    assert_eq!(rules.find_iter("abc").next(), None);

    // Compile errors are reported.
    assert!(MultiRule::new([("bad", "(")]).is_err());

    // Rule IDs may be any cloneable type.
    let rules = MultiRule::new([(7u32, "a"), (42, "b")]).unwrap();
    let got: Vec<u32> = hits(&rules, "ab").iter().map(|m| *m.id()).collect();
    assert_eq!(got, vec![7, 42]);
}
