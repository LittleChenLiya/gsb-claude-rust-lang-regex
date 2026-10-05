// Targeted tests for the explicit capture group renamer (`regex::rename`).
//
// Run with: cargo test --test rename

use regex::rename::{rename_captures, RenameConflict, RenameError};
use regex::Regex;

fn spans_of(
    caps: &regex::Captures,
) -> Vec<Option<(usize, usize)>> {
    (0..caps.len())
        .map(|i| caps.get(i).map(|m| (m.start(), m.end())))
        .collect()
}

// Acceptance: rename `year` to `y`. All matched bytes and all group
// numbers are identical, and the old `year` content is fetched via `y`.
#[test]
fn year_to_y_preserves_bytes_and_group_numbers() {
    let pattern = r"(?P<year>\d{4})-(?P<month>\d{2})-(?P<day>\d{2})";
    let renamed = rename_captures(pattern, [("year", "y")]).unwrap();
    assert_eq!(
        renamed.pattern(),
        r"(?P<y>\d{4})-(?P<month>\d{2})-(?P<day>\d{2})"
    );

    let old = Regex::new(pattern).unwrap();
    let new = renamed.regex();
    for haystack in ["2026-10-05", "x 1999-12-31 y", "0000-01-01"] {
        let caps_old = old.captures(haystack).unwrap();
        let caps_new = new.captures(haystack).unwrap();
        // Same number of groups, same byte ranges for every group number.
        assert_eq!(caps_old.len(), caps_new.len());
        assert_eq!(spans_of(&caps_old), spans_of(&caps_new));
        // The old `year` content is available under `y`...
        assert_eq!(
            caps_new.name("y").map(|m| m.as_str()),
            caps_old.name("year").map(|m| m.as_str()),
        );
        // ... at the same group number (1) as before.
        assert_eq!(caps_new.get(1).map(|m| m.as_str()), caps_old.get(1).map(|m| m.as_str()));
        assert_eq!(caps_new.name("y").map(|m| m.as_str()), caps_new.get(1).map(|m| m.as_str()));
        // The old name is gone from the renamed regex.
        assert!(caps_new.name("year").is_none());
    }
    // Group numbers are unchanged: year/y is 1, month is 2, day is 3.
    let names: Vec<Option<&str>> = new.capture_names().collect();
    assert_eq!(names, vec![None, Some("y"), Some("month"), Some("day")]);

    // The migration table maps the old name to the new one.
    assert_eq!(renamed.new_name("year"), Some("y"));
    assert_eq!(renamed.name_map().get("year"), Some(&"y"));
    assert_eq!(renamed.renames().len(), 1);
    assert_eq!(renamed.renames()[0].index(), 1);

    // The original regex is untouched and keeps working with `year`.
    let caps = old.captures("2026-10-05").unwrap();
    assert_eq!(caps.name("year").unwrap().as_str(), "2026");
}

// Acceptance: mapping two groups to the same new name fails before
// anything is delivered.
#[test]
fn two_groups_mapped_to_same_name_rejected() {
    let pattern = r"(?P<a>\d+)-(?P<b>\d+)";
    let err = rename_captures(pattern, [("a", "n"), ("b", "n")]).unwrap_err();
    match err {
        RenameError::Conflicts(conflicts) => {
            assert!(
                conflicts.contains(&RenameConflict::DuplicateNewName {
                    name: "n".to_string(),
                    old_names: vec!["a".to_string(), "b".to_string()],
                }),
                "unexpected conflicts: {conflicts:?}",
            );
        }
        err => panic!("expected conflicts, got {err:?}"),
    }
}

// Acceptance: similar names in literal text, character classes and
// escapes are not renamed; only the group name token is.
#[test]
fn similar_names_in_text_classes_and_escapes_untouched() {
    // Literal `year`, a `[year]` class, an escaped `\x79` ('y') followed
    // by literal `ear`, and a `(?P<year>...)` group.
    let pattern = r"year(?P<year>\d{4})[year]\x79ear";
    let renamed = rename_captures(pattern, [("year", "y")]).unwrap();
    // Only the group name changed.
    assert_eq!(renamed.pattern(), r"year(?P<y>\d{4})[year]\x79ear");

    let old = Regex::new(pattern).unwrap();
    let caps_old = old.captures("year2026yyear").unwrap();
    let caps_new = renamed.regex().captures("year2026yyear").unwrap();
    assert_eq!(spans_of(&caps_old), spans_of(&caps_new));
    assert_eq!(caps_new.get(0).unwrap().as_str(), "year2026yyear");
    assert_eq!(caps_new.name("y").unwrap().as_str(), "2026");
}

// Nested groups: every rename keeps a traceable source position in the
// original pattern, and group numbers stay put.
#[test]
fn nested_groups_have_traceable_source_spans() {
    let pattern = r"(?P<outer>a(?P<inner>b)(?P<inner2>c))";
    let renamed =
        rename_captures(pattern, [("inner", "in"), ("outer", "out")]).unwrap();
    assert_eq!(renamed.pattern(), r"(?P<out>a(?P<in>b)(?P<inner2>c))");

    let table = renamed.renames();
    assert_eq!(table.len(), 2);
    // Entries are in source order: the outer group comes first.
    let outer = &table[0];
    assert_eq!(outer.old_name(), "outer");
    assert_eq!(outer.new_name(), "out");
    assert_eq!(outer.index(), 1);
    // The spans point into the *original* pattern.
    assert_eq!(
        &pattern[outer.name_span().start()..outer.name_span().end()],
        "outer"
    );
    assert_eq!(
        &pattern[outer.group_span().start()..outer.group_span().end()],
        "(?P<outer>a(?P<inner>b)(?P<inner2>c))"
    );
    let inner = &table[1];
    assert_eq!(inner.old_name(), "inner");
    assert_eq!(inner.new_name(), "in");
    assert_eq!(inner.index(), 2);
    assert_eq!(
        &pattern[inner.name_span().start()..inner.name_span().end()],
        "inner"
    );
    assert_eq!(
        &pattern[inner.group_span().start()..inner.group_span().end()],
        "(?P<inner>b)"
    );

    // Group numbers are preserved; the untouched nested group keeps its
    // name and number.
    let names: Vec<Option<&str>> = renamed.regex().capture_names().collect();
    assert_eq!(names, vec![None, Some("out"), Some("in"), Some("inner2")]);

    // Matching still works through nesting.
    let caps = renamed.regex().captures("abc").unwrap();
    assert_eq!(caps.name("out").unwrap().as_str(), "abc");
    assert_eq!(caps.name("in").unwrap().as_str(), "b");
    assert_eq!(caps.name("inner2").unwrap().as_str(), "c");
}

// A new name that collides with an existing group that is not itself
// being renamed is rejected.
#[test]
fn new_name_colliding_with_existing_group_rejected() {
    let err = rename_captures(r"(?P<a>x)(?P<b>y)", [("a", "b")]).unwrap_err();
    match err {
        RenameError::Conflicts(conflicts) => {
            assert!(
                conflicts.contains(&RenameConflict::NewNameAlreadyExists {
                    name: "b".to_string(),
                }),
                "unexpected conflicts: {conflicts:?}",
            );
        }
        err => panic!("expected conflicts, got {err:?}"),
    }
}

// Simultaneous renames make swaps possible.
#[test]
fn simultaneous_swap_is_allowed() {
    let renamed =
        rename_captures(r"(?P<a>x)(?P<b>y)", [("a", "b"), ("b", "a")])
            .unwrap();
    assert_eq!(renamed.pattern(), r"(?P<b>x)(?P<a>y)");
    let caps = renamed.regex().captures("xy").unwrap();
    assert_eq!(caps.name("b").unwrap().as_str(), "x");
    assert_eq!(caps.name("a").unwrap().as_str(), "y");
}

// Renaming a group that does not exist is a conflict.
#[test]
fn unknown_old_name_rejected() {
    let err = rename_captures(r"(?P<a>x)", [("zzz", "q")]).unwrap_err();
    match err {
        RenameError::Conflicts(conflicts) => {
            assert!(
                conflicts.contains(&RenameConflict::OldNameNotFound {
                    name: "zzz".to_string(),
                }),
                "unexpected conflicts: {conflicts:?}",
            );
        }
        err => panic!("expected conflicts, got {err:?}"),
    }
}

// All conflicts in a request are gathered and reported together.
#[test]
fn conflicts_are_collected_as_a_set() {
    // Two groups mapped to the same new name AND an unknown old name.
    let err = rename_captures(
        r"(?P<a>x)(?P<b>y)",
        [("a", "n"), ("b", "n"), ("zzz", "q")],
    )
    .unwrap_err();
    match err {
        RenameError::Conflicts(conflicts) => {
            assert!(
                conflicts.contains(&RenameConflict::DuplicateNewName {
                    name: "n".to_string(),
                    old_names: vec!["a".to_string(), "b".to_string()],
                }),
                "unexpected conflicts: {conflicts:?}",
            );
            assert!(
                conflicts.contains(&RenameConflict::OldNameNotFound {
                    name: "zzz".to_string(),
                }),
                "unexpected conflicts: {conflicts:?}",
            );
        }
        err => panic!("expected conflicts, got {err:?}"),
    }
}

// Duplicate old names in the request are rejected.
#[test]
fn duplicate_old_name_rejected() {
    let err =
        rename_captures(r"(?P<a>x)", [("a", "b"), ("a", "c")]).unwrap_err();
    match err {
        RenameError::Conflicts(conflicts) => {
            assert!(
                conflicts.contains(&RenameConflict::DuplicateOldName {
                    name: "a".to_string(),
                }),
                "unexpected conflicts: {conflicts:?}",
            );
        }
        err => panic!("expected conflicts, got {err:?}"),
    }
}

// A no-op rename (old == new) is rejected as a conflict.
#[test]
fn unchanged_name_rejected() {
    let err = rename_captures(r"(?P<a>x)", [("a", "a")]).unwrap_err();
    match err {
        RenameError::Conflicts(conflicts) => {
            assert!(
                conflicts.contains(&RenameConflict::NameUnchanged {
                    name: "a".to_string(),
                }),
                "unexpected conflicts: {conflicts:?}",
            );
        }
        err => panic!("expected conflicts, got {err:?}"),
    }
}

// An invalid new capture name makes the rewritten pattern fail to
// compile, so nothing is delivered.
#[test]
fn invalid_new_name_fails_compile_and_delivers_nothing() {
    let err = rename_captures(r"(?P<a>x)", [("a", "bad-name")]).unwrap_err();
    match err {
        RenameError::Build(_) => {}
        err => panic!("expected build error, got {err:?}"),
    }
}

// A pattern that does not parse is a syntax error.
#[test]
fn unparsable_pattern_is_syntax_error() {
    let err = rename_captures(r"(?P<a>x", [("a", "b")]).unwrap_err();
    match err {
        RenameError::Syntax(_) => {}
        err => panic!("expected syntax error, got {err:?}"),
    }
}

// The rendered migration table mentions every rename.
#[test]
fn migration_table_mentions_all_renames() {
    let renamed = rename_captures(
        r"(?P<year>\d{4})-(?P<month>\d{2})",
        [("year", "y"), ("month", "m")],
    )
    .unwrap();
    let table = renamed.migration_table();
    assert!(table.contains("year"), "table: {table}");
    assert!(table.contains("month"), "table: {table}");
    assert!(table.contains('y'), "table: {table}");
    assert!(table.contains('m'), "table: {table}");
    assert_eq!(renamed.name_map().len(), 2);
    assert_eq!(renamed.new_name("month"), Some("m"));
    assert_eq!(renamed.new_name("day"), None);
}
