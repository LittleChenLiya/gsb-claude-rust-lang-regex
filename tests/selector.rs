/*!
Acceptance tests for the independent log-classification selector.

These cover the contract directly: rules stay independent (they are not one
big OR), the earliest match start wins, ties at the same byte offset go to
the earlier declared rule, a rule's own leftmost-first alternation semantics
are preserved, zero-width hits participate, the byte variant keeps non-UTF-8
bytes, and failure to match returns `None`.
*/

use regex::selector::Selector;

#[test]
fn acceptance_z_and_ab_picks_ab_on_xxabz() {
    // Rule 0 = z, rule 1 = ab.
    let sel = Selector::new(["z", "ab"]).unwrap();
    let pick = sel.classify("xxabz").expect("a rule must match");
    assert_eq!(pick.rule(), 1, "ab starts before z, so rule 1 wins");
    assert_eq!(pick.start(), 2);
    assert_eq!(pick.end(), 4);
    assert_eq!(pick.matched(), "ab");
    // The captures returned are those of rule 1 run on its own.
    assert_eq!(pick.captures().get(0).unwrap().range(), 2..4);
}

#[test]
fn acceptance_same_position_picks_first_declared() {
    // The exact acceptance scenario: both rules applicable at position 2.
    // Use patterns whose leftmost hits coincide; declaration order decides.
    let sel = Selector::new(["z", "zz"]).unwrap();
    let pick = sel.classify("xxzz").unwrap();
    assert_eq!(pick.rule(), 0);
    assert_eq!(pick.start(), 2);
    assert_eq!(pick.matched(), "z");

    // And reversing the declaration order reverses the answer, even though
    // both rules still match at the same position.
    let sel = Selector::new(["zz", "z"]).unwrap();
    let pick = sel.classify("xxzz").unwrap();
    assert_eq!(pick.rule(), 0);
    assert_eq!(pick.matched(), "zz");
}

#[test]
fn acceptance_single_rule_a_or_ab_keeps_leftmost_first() {
    let sel = Selector::new(["a|ab"]).unwrap();
    let pick = sel.classify("ab").unwrap();
    assert_eq!(pick.rule(), 0);
    assert_eq!(pick.matched(), "a", "a|ab must still choose a, not ab");
    assert_eq!(pick.range(), 0..1);

    // The same rule placed after another rule keeps its internal preference.
    let sel = Selector::new(["z+", "a|ab"]).unwrap();
    let pick = sel.classify("abzz").unwrap();
    assert_eq!(pick.rule(), 1);
    assert_eq!(pick.matched(), "a");
}

#[test]
fn acceptance_later_rule_can_match_earlier_than_first_registered() {
    // A rule registered first that only matches late must not hide another
    // rule matching earlier in the haystack.
    let sel = Selector::new([r"z$", r"ab"]).unwrap();
    let pick = sel.classify("xxabz").unwrap();
    assert_eq!(pick.rule(), 1);
    assert_eq!(pick.start(), 2);
}

#[test]
fn acceptance_no_match_is_none() {
    let sel = Selector::new(["z", "ab"]).unwrap();
    assert!(sel.classify("0123456789").is_none());
}

#[test]
fn acceptance_zero_width_match_participates() {
    // Zero-width example (string variant): an empty anchor at position 0 is
    // a genuine hit earlier than "ab" at position 2, and it must count.
    let sel = Selector::new([r"^", r"ab"]).unwrap();
    let pick = sel.classify("xxab").unwrap();
    assert_eq!(pick.rule(), 0);
    assert_eq!(pick.range(), 0..0);
    assert!(pick.is_empty());
    assert_eq!(pick.matched(), "");
    assert_eq!(pick.captures().get(0).unwrap().range(), 0..0);

    // Zero-width tie broken by declaration order: in " x" the word boundary
    // and the literal "x" both start at byte 1.
    let sel = Selector::new([r"\b", r"x"]).unwrap();
    let pick = sel.classify(" x").unwrap();
    assert_eq!(pick.rule(), 0);
    assert_eq!(pick.start(), 1);
    assert!(pick.is_empty());
}

#[test]
fn acceptance_captures_belong_to_winning_rule() {
    let sel = Selector::new([r"z(?P<tail>\w*)", r"(?P<first>a)(?P<second>b)"])
        .unwrap();
    let pick = sel.classify("xxabz").unwrap();
    assert_eq!(pick.rule(), 1);
    assert_eq!(pick.matched(), "ab");
    assert_eq!(pick.captures().name("first").unwrap().as_str(), "a");
    assert_eq!(pick.captures().name("second").unwrap().as_str(), "b");
    assert!(pick.captures().name("tail").is_none());
}

#[test]
fn acceptance_string_offsets_split_utf8_boundaries() {
    let hay = "caf\u{e9}ab";
    // 'e-acute' occupies 2 bytes, so "ab" begins at byte 5.
    let sel = Selector::new(["z", "ab"]).unwrap();
    let pick = sel.classify(hay).unwrap();
    assert_eq!(pick.rule(), 1);
    assert_eq!(pick.start(), 5);
    assert!(hay.is_char_boundary(pick.start()));
    assert_eq!(&hay[pick.range()], "ab");
}

#[test]
fn acceptance_invalid_pattern_error_and_empty_selector() {
    assert!(Selector::new(["("]).is_err());
    let empty = Selector::empty();
    assert_eq!(empty.len(), 0);
    assert!(empty.is_empty());
    assert!(empty.classify("whatever").is_none());
}

mod bytes_tests {
    //! Byte variant: non-UTF-8 haystacks are preserved (this case cannot be
    //! expressed through the `&str` API).
    use regex::selector::bytes::Selector;

    #[test]
    fn acceptance_non_utf8_bytes_preserved() {
        let hay: &[u8] = b"xxab\xFF\xFFz";
        // Rule 0 finds "z" at 6; rule 1 finds the invalid byte run at 4.
        let sel = Selector::new([r"z", r"(?-u)\xFF+"]).unwrap();
        let pick = sel.classify(hay).unwrap();
        assert_eq!(pick.rule(), 1);
        assert_eq!(pick.range(), 4..6);
        assert_eq!(pick.matched(), b"\xFF\xFF");
        // The returned slice really is the original, non-UTF-8 bytes.
        assert_eq!(&hay[pick.range()], b"\xFF\xFF");
        assert!(std::str::from_utf8(&hay[pick.range()]).is_err());
    }

    #[test]
    fn acceptance_non_utf8_with_zero_width_tie() {
        let hay: &[u8] = b"\xFFa";
        // ASCII-compatible word boundary matches empty right before 'a' at
        // byte 1, the same position where rule "a" starts; it is declared
        // first, so the zero-width hit wins. The leading 0xFF byte is
        // invalid UTF-8 and stays untouched.
        let sel = Selector::new([r"(?-u)\b", "a"]).unwrap();
        let pick = sel.classify(hay).unwrap();
        assert_eq!(pick.rule(), 0);
        assert_eq!(pick.start(), 1);
        assert_eq!(pick.end(), 1);
        assert!(pick.is_empty());
        assert_eq!(&hay[..1], b"\xFF");
    }

    #[test]
    fn acceptance_bytes_same_contract_as_str() {
        let sel = Selector::new(["z", "ab"]).unwrap();
        let pick = sel.classify(b"xxabz").unwrap();
        assert_eq!(pick.rule(), 1);
        assert_eq!(pick.matched(), b"ab");
        let alt = Selector::new(["a|ab"]).unwrap();
        assert_eq!(alt.classify(b"ab").unwrap().matched(), b"a");
    }
}
