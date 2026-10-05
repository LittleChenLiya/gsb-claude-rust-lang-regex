//! Integration tests for the explicit capture group renamer.

use regex::rename::{rename_captures, RenameError};

#[test]
fn rename_year_to_y() {
    let result =
        rename_captures(r"(?P<year>\d{4})-(?P<month>\d{2})", [("year", "y")])
            .unwrap();
    assert_eq!(result.pattern, r"(?P<y>\d{4})-(?P<month>\d{2})");

    let caps = result.regex.captures("2026-10").unwrap();
    assert_eq!(&caps["y"], "2026");
    assert_eq!(&caps["month"], "10");
    assert_eq!(&caps[1], "2026");
    assert_eq!(&caps[2], "10");

    let migration = result.migrations.get("year").unwrap();
    assert_eq!(migration.old_name, "year");
    assert_eq!(migration.new_name, "y");
    assert_eq!(migration.index, 1);
}

#[test]
fn matching_bytes_and_group_numbers_unchanged() {
    let original =
        regex::Regex::new(r"(?P<year>\d{4})-(?P<month>\d{2})").unwrap();
    let renamed =
        rename_captures(r"(?P<year>\d{4})-(?P<month>\d{2})", [("year", "y")])
            .unwrap();

    for hay in ["2026-10", "1999-01", "nope", "1234-56"] {
        let o = original.captures(hay);
        let r = renamed.regex.captures(hay);
        match (o, r) {
            (Some(o), Some(r)) => {
                assert_eq!(
                    o.get(0).unwrap().range(),
                    r.get(0).unwrap().range()
                );
                assert_eq!(o.len(), r.len());
                for i in 1..o.len() {
                    assert_eq!(
                        o.get(i).unwrap().range(),
                        r.get(i).unwrap().range()
                    );
                    assert_eq!(&o[i], &r[i]);
                }
                assert_eq!(&o["year"], &r["y"]);
                assert_eq!(&o["month"], &r["month"]);
            }
            (None, None) => {}
            _ => panic!("match mismatch for {hay:?}"),
        }
    }
}

#[test]
fn two_groups_mapping_to_same_name_fail_first() {
    let err = rename_captures(
        r"(?P<year>\d{4})-(?P<month>\d{2})",
        [("year", "y"), ("month", "y")],
    )
    .unwrap_err();
    assert!(matches!(err, RenameError::Conflict { .. }));
}

#[test]
fn similar_names_in_text_are_not_changed() {
    let result = rename_captures(
        r"(?P<year>\d{4}) year (?P<month>\d{2}) month",
        [("year", "y")],
    )
    .unwrap();
    assert_eq!(result.pattern, r"(?P<y>\d{4}) year (?P<month>\d{2}) month");
    let caps = result.regex.captures("2026 year 10 month").unwrap();
    assert_eq!(&caps["y"], "2026");
    assert_eq!(&caps["month"], "10");
}

#[test]
fn character_classes_and_escapes_are_untouched() {
    let result =
        rename_captures(r"(?P<year>\d{4}) [a-z]+ \x41 \pL", [("year", "y")])
            .unwrap();
    assert_eq!(result.pattern, r"(?P<y>\d{4}) [a-z]+ \x41 \pL");
    assert!(result.regex.is_match("2026 abc A x"));
}

#[test]
fn nested_groups_are_tracked() {
    let result = rename_captures(
        r"(?P<outer>(?P<inner>\d+))",
        [("outer", "o"), ("inner", "i")],
    )
    .unwrap();
    assert_eq!(result.pattern, r"(?P<o>(?P<i>\d+))");
    let caps = result.regex.captures("42").unwrap();
    assert_eq!(&caps["o"], "42");
    assert_eq!(&caps["i"], "42");
    assert_eq!(&caps[1], "42");
    assert_eq!(&caps[2], "42");

    let outer = result.migrations.get("outer").unwrap();
    let inner = result.migrations.get("inner").unwrap();
    assert_eq!(outer.index, 1);
    assert_eq!(inner.index, 2);
}

#[test]
fn unknown_old_name_is_rejected() {
    let err =
        rename_captures(r"(?P<year>\d{4})", [("nope", "y")]).unwrap_err();
    assert!(matches!(err, RenameError::UnknownName(_)));
}

#[test]
fn collision_with_unrenamed_group_is_rejected() {
    let err =
        rename_captures(r"(?P<year>\d{4})-(?P<y>[a-z]+)", [("year", "y")])
            .unwrap_err();
    assert!(matches!(err, RenameError::Collision { .. }));
}

#[test]
fn chained_rename_is_allowed() {
    let result = rename_captures(
        r"(?P<year>\d{4})-(?P<y>[a-z]+)",
        [("year", "y"), ("y", "z")],
    )
    .unwrap();
    assert_eq!(result.pattern, r"(?P<y>\d{4})-(?P<z>[a-z]+)");
    let caps = result.regex.captures("2026-abc").unwrap();
    assert_eq!(&caps["y"], "2026");
    assert_eq!(&caps["z"], "abc");
}

#[test]
fn self_mapping_is_a_noop() {
    let result =
        rename_captures(r"(?P<year>\d{4})", [("year", "year")]).unwrap();
    assert_eq!(result.pattern, r"(?P<year>\d{4})");
    assert_eq!(result.migrations.get("year").unwrap().new_name, "year");
}

#[test]
fn invalid_new_name_is_rejected() {
    let err =
        rename_captures(r"(?P<year>\d{4})", [("year", "1bad")]).unwrap_err();
    assert!(matches!(err, RenameError::InvalidName(_)));
}

#[test]
fn parse_error_is_reported() {
    let err = rename_captures(r"(?P<year>", [("year", "y")]).unwrap_err();
    assert!(matches!(err, RenameError::Parse(_)));
}

#[test]
fn index_groups_are_untouched() {
    let result =
        rename_captures(r"(a)(?P<year>b)(c)", [("year", "y")]).unwrap();
    assert_eq!(result.pattern, r"(a)(?P<y>b)(c)");
    let caps = result.regex.captures("abc").unwrap();
    assert_eq!(&caps[1], "a");
    assert_eq!(&caps["y"], "b");
    assert_eq!(&caps[3], "c");
}

#[test]
fn question_lt_syntax_preserved() {
    let result = rename_captures(r"(?<year>\d{4})", [("year", "y")]).unwrap();
    assert_eq!(result.pattern, r"(?<y>\d{4})");
    assert_eq!(&result.regex.captures("2026").unwrap()["y"], "2026");
}
