/*!
Tests for the independent multi-rule replacement component.

Selection results are cross-checked against a deliberately simple,
sequential reference implementation written directly from the specification
(see the reference_select function in this file).
*/

use regex::multi_replace::{
    MatchReport, MultiReplace, MultiReplaceBuilder, MultiReplaceError,
    ReplacementRule,
};

/// One rule as seen by the reference implementation.
struct RefRule {
    pattern: &'static str,
    priority: i64,
    template: &'static str,
}

/// A reference candidate: (start, end, priority, rule id).
type RefCandidate = (usize, usize, i64, usize);

/// Enumerate and select adopted candidates exactly as the spec says, using
/// the ordinary single-regex find_iter and a plain sequential scan.
fn reference_select(haystack: &str, rules: &[RefRule]) -> Vec<RefCandidate> {
    let mut candidates: Vec<RefCandidate> = Vec::new();
    for (id, rule) in rules.iter().enumerate() {
        let re = regex::Regex::new(rule.pattern).unwrap();
        for m in re.find_iter(haystack) {
            candidates.push((m.start(), m.end(), rule.priority, id));
        }
    }
    candidates.sort_by(|a, b| {
        a.0.cmp(&b.0)
            .then_with(|| b.2.cmp(&a.2))
            .then_with(|| (b.1 - b.0).cmp(&(a.1 - a.0)))
            .then_with(|| a.3.cmp(&b.3))
    });
    let mut accepted: Vec<RefCandidate> = Vec::new();
    for cand in candidates {
        let blocked = accepted.iter().any(|chosen| {
            chosen.0 == cand.0 || (chosen.0 < cand.1 && cand.0 < chosen.1)
        });
        if !blocked {
            accepted.push(cand);
        }
    }
    accepted
}

/// Build a MultiReplace from reference rules.
fn build(rules: &[RefRule]) -> MultiReplace {
    MultiReplace::new(
        rules
            .iter()
            .map(|r| ReplacementRule::new(r.pattern, r.priority, r.template)),
    )
    .unwrap()
}

fn adopted(reports: &[MatchReport]) -> Vec<(usize, usize, usize)> {
    reports
        .iter()
        .filter(|r| r.adopted)
        .map(|r| (r.range.start, r.range.end, r.rule))
        .collect()
}

fn check_against_reference(haystack: &str, rules: &[RefRule]) {
    let expected: Vec<(usize, usize, usize)> =
        reference_select(haystack, rules)
            .into_iter()
            .map(|(s, e, _, id)| (s, e, id))
            .collect();
    let out = build(rules).replace_all(haystack).unwrap();
    assert_eq!(adopted(&out.reports), expected, "selection mismatch");

    // Independently rebuild the output from the adopted selection.
    let mut rebuilt = String::new();
    let mut cursor = 0;
    for (start, end, id) in &expected {
        rebuilt.push_str(&haystack[cursor..*start]);
        let re = regex::Regex::new(rules[*id].pattern).unwrap();
        let caps = re.captures_at(haystack, *start).unwrap();
        caps.expand(rules[*id].template, &mut rebuilt);
        cursor = *end;
    }
    rebuilt.push_str(&haystack[cursor..]);
    assert_eq!(out.text, rebuilt);
}

#[test]
fn earlier_start_beats_higher_priority() {
    // On "abcde", abc (priority 1) starts at 0 and bcde (priority 9) starts
    // at 1. The earlier-start candidate wins despite its lower priority.
    let rules = [
        RefRule { pattern: "abc", priority: 1, template: "[abc]" },
        RefRule { pattern: "bcde", priority: 9, template: "[bcde]" },
    ];
    let out = build(&rules).replace_all("abcde").unwrap();
    assert_eq!(out.text, "[abc]de");
    check_against_reference("abcde", &rules);
}

#[test]
fn same_start_priority_wins() {
    // At the same start, ab (priority 2) beats abc (priority 1).
    let rules = [
        RefRule { pattern: "abc", priority: 1, template: "[abc]" },
        RefRule { pattern: "bcde", priority: 9, template: "[bcde]" },
        RefRule { pattern: "ab", priority: 2, template: "[ab]" },
    ];
    let out = build(&rules).replace_all("abcde").unwrap();
    assert_eq!(out.text, "[ab]cde");
    check_against_reference("abcde", &rules);
}

#[test]
fn same_start_and_priority_longer_wins() {
    let rules = [
        RefRule { pattern: "ab", priority: 0, template: "X" },
        RefRule { pattern: "abc", priority: 0, template: "Y" },
    ];
    let out = build(&rules).replace_all("abcde").unwrap();
    assert_eq!(out.text, "Yde");
    check_against_reference("abcde", &rules);
}

#[test]
fn registration_order_is_final_tiebreaker() {
    let rules = [
        RefRule { pattern: "bc", priority: 0, template: "first" },
        RefRule { pattern: "bc", priority: 0, template: "second" },
    ];
    let out = build(&rules).replace_all("abc").unwrap();
    assert_eq!(out.text, "afirst");
    assert_eq!(
        out.reports,
        vec![
            MatchReport { rule: 0, range: 1..3, adopted: true },
            MatchReport { rule: 1, range: 1..3, adopted: false },
        ]
    );
    check_against_reference("abc", &rules);
}

#[test]
fn chinese_and_named_groups() {
    let rules = [RefRule {
        pattern: r"(?<word>\p{Han}+)",
        priority: 0,
        template: "\u{27e8}$word\u{27e9}",
    }];
    let hay = "你好 world 世界";
    let out = build(&rules).replace_all(hay).unwrap();
    assert_eq!(out.text, "\u{27e8}你好\u{27e9} world \u{27e8}世界\u{27e9}");
    // Ranges are byte offsets that stay on UTF-8 boundaries.
    assert_eq!(out.reports[0].range, 0..6);
    assert_eq!(out.reports[1].range, 13..19);
    check_against_reference(hay, &rules);
}

#[test]
fn empty_matches_boundaries_and_shadowing() {
    // An empty pattern produces a zero-width candidate at every UTF-8 safe
    // boundary, including the end. find_iter must terminate.
    let rules = [RefRule { pattern: "", priority: 0, template: "." }];
    let out = build(&rules).replace_all("ab").unwrap();
    assert_eq!(out.text, ".a.b.");
    check_against_reference("ab", &rules);

    // The empty candidate at position 0 shares the start with "abc" and is
    // shadowed by the higher-priority rule; empty candidates strictly inside
    // the interval are shadowed too. The one exactly at the trailing end
    // survives, so only a trailing marker remains.
    let rules = [
        RefRule { pattern: "abc", priority: 1, template: "X" },
        RefRule { pattern: "", priority: 0, template: "|" },
    ];
    let hay = "abc";
    let out = build(&rules).replace_all(hay).unwrap();
    assert_eq!(out.text, "X|");
    check_against_reference(hay, &rules);

    // Zero-width candidates at the same start compete by priority too.
    let rules = [
        RefRule { pattern: "", priority: 0, template: "a" },
        RefRule { pattern: "$", priority: 1, template: "z" },
    ];
    let out = build(&rules).replace_all("x").unwrap();
    assert_eq!(out.text, "axz");
    check_against_reference("x", &rules);
}

#[test]
fn replacement_text_is_not_re_searched() {
    // Expanding "a" to "aa" must not let the "aa" rule match the expansion.
    let rules = [
        RefRule { pattern: "aa", priority: 1, template: "B" },
        RefRule { pattern: "a", priority: 0, template: "aa" },
    ];
    let out = build(&rules).replace_all("a").unwrap();
    assert_eq!(out.text, "aa");
    check_against_reference("a", &rules);

    // Same idea with a longer expansion.
    let rules = [
        RefRule { pattern: "bbb", priority: 9, template: "C" },
        RefRule { pattern: "b", priority: 0, template: "bbb" },
    ];
    let out = build(&rules).replace_all("b").unwrap();
    assert_eq!(out.text, "bbb");
    check_against_reference("b", &rules);
}

#[test]
fn adjacent_matches_do_not_block() {
    let rules = [
        RefRule { pattern: "ab", priority: 0, template: "1" },
        RefRule { pattern: "cd", priority: 0, template: "2" },
    ];
    let out = build(&rules).replace_all("abcd").unwrap();
    assert_eq!(out.text, "12");
    check_against_reference("abcd", &rules);
}

#[test]
fn templates_are_validated_before_processing() {
    // Unknown numeric group.
    let err =
        MultiReplace::new([ReplacementRule::new("(a)", 0, "$2")]).unwrap_err();
    assert!(matches!(err, MultiReplaceError::InvalidTemplate { rule: 0, .. }));

    // Unknown named group.
    let err = MultiReplace::new([ReplacementRule::new("(?<a>x)", 0, "$b")])
        .unwrap_err();
    assert!(matches!(err, MultiReplaceError::InvalidTemplate { rule: 0, .. }));

    // Unclosed braced reference. The \x24 escape is just a dollar sign.
    let err = MultiReplace::new([ReplacementRule::new("x", 0, "\x24{a")])
        .unwrap_err();
    assert!(matches!(err, MultiReplaceError::InvalidTemplate { rule: 0, .. }));

    // A valid template with an escaped dollar, a braced name and $1 builds.
    let mr = MultiReplace::new([ReplacementRule::new(
        "(?<a>x)",
        0,
        "\x24\x24 \x24{a} $1",
    )])
    .unwrap();
    assert_eq!(mr.replace_all("x").unwrap().text, "$ x x");
}

#[test]
fn invalid_regex_reports_rule_without_pattern() {
    let err = MultiReplace::new([
        ReplacementRule::new("(ok)", 0, "x"),
        ReplacementRule::new("(", 0, "y"),
    ])
    .unwrap_err();
    match err {
        MultiReplaceError::InvalidRegex { rule, message } => {
            assert_eq!(rule, 1);
            assert!(!message.contains('('));
            assert_eq!(message, "the regex pattern failed to compile");
        }
        other => panic!("expected InvalidRegex, got {other:?}"),
    }
}

#[test]
fn candidate_limit_is_enforced_without_partial_result() {
    // An empty pattern over "abcde" yields one candidate per byte boundary
    // (6 total). A cap of 3 must fail outright.
    let mut builder = MultiReplaceBuilder::new();
    builder.add("", 0, ".");
    builder.max_candidates(3);
    let mr = builder.build().unwrap();
    let err = mr.replace_all("abcde").unwrap_err();
    assert_eq!(
        err,
        MultiReplaceError::TooManyCandidates { limit: 3, found: 4 }
    );
}

#[test]
fn output_limit_is_enforced() {
    let mut builder = MultiReplaceBuilder::new();
    builder.add("a", 0, "aaaaaa");
    builder.max_output_bytes(3);
    let mr = builder.build().unwrap();
    let err = mr.replace_all("a").unwrap_err();
    assert_eq!(err, MultiReplaceError::OutputTooLarge { limit: 3 });
}

#[test]
fn zero_candidate_cap_and_no_rules() {
    let mut builder = MultiReplaceBuilder::new();
    builder.add("a", 0, "x");
    builder.max_candidates(0);
    let mr = builder.build().unwrap();
    // Any candidate at all exceeds a cap of zero.
    assert!(matches!(
        mr.replace_all("a").unwrap_err(),
        MultiReplaceError::TooManyCandidates { limit: 0, .. }
    ));

    // With no rules and no candidates, the original text is unchanged and no
    // matches are reported.
    let empty = MultiReplaceBuilder::new().build().unwrap();
    assert!(empty.is_empty());
    let out = empty.replace_all("abc").unwrap();
    assert_eq!(out.text, "abc");
    assert!(out.reports.is_empty());
}

#[test]
fn reports_never_contain_haystack_text() {
    let secret = "SUPER-SECRET";
    let rules = [
        RefRule { pattern: "SECRET", priority: 1, template: "X" },
        RefRule { pattern: "E", priority: 0, template: "Y" },
    ];
    let out = build(&rules).replace_all(secret).unwrap();
    let debug = format!("{:?}", out.reports);
    assert!(!debug.contains("SECRET"));
    assert!(!debug.contains(secret));
}

#[test]
fn brute_force_reference_cross_check() {
    // A fixed matrix of patterns and inputs keeps the real implementation
    // aligned with the sequential reference.
    let patterns = ["a", "", "ab", "b", "a*", "$", r"\b"];
    let inputs = ["", "a", "ab", "aba", "中文ab", "bba"];
    let rules: Vec<RefRule> = patterns
        .iter()
        .enumerate()
        .map(|(id, pattern)| RefRule {
            pattern,
            priority: (id as i64) % 3,
            template: "x",
        })
        .collect();
    for input in inputs {
        check_against_reference(input, &rules);
    }
}
