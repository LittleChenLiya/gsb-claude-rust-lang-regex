//! Explicit capture group renaming based on the regex syntax tree.
//!
//! This module provides [`rename_captures`], which rewrites the names of
//! named capture groups in a regular expression according to an explicit
//! old-to-new name mapping. The rewrite is performed on the parsed syntax
//! tree, so:
//!
//! * only capture group names are changed — the matching language and every
//!   group's index are preserved;
//! * character classes, escape sequences and literal text are never touched;
//! * nested groups are visited recursively, and each rename records the
//!   source span of the original name.
//!
//! The mapping is validated as a whole before anything is rewritten: old
//! names that do not exist, two old names mapping to the same new name, and
//! new names colliding with groups that are not renamed are all rejected up
//! front. The rewritten pattern is only delivered once it compiles
//! successfully.

use alloc::collections::BTreeMap;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use regex_syntax::ast::parse::Parser;
use regex_syntax::ast::print::Printer;
use regex_syntax::ast::{Ast, Group, GroupKind, Span};

use crate::Regex;

/// A record describing one renamed capture group.
///
/// The capture group's index is unchanged by renaming.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CaptureRename {
    /// The capture group's name before renaming.
    pub old_name: String,
    /// The capture group's name after renaming.
    pub new_name: String,
    /// The capture group's index. Renaming never changes group indices.
    pub index: u32,
    /// The byte span of the old name in the original pattern.
    pub span: Span,
}

/// The result of a successful capture group renaming.
#[derive(Clone, Debug)]
pub struct RenamedRegex {
    /// The regex compiled from the rewritten pattern.
    pub regex: Regex,
    /// The rewritten pattern source.
    pub pattern: String,
    /// The migration table mapping old capture names to their rename records.
    pub migrations: BTreeMap<String, CaptureRename>,
}

/// An error that can occur while renaming capture groups.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RenameError {
    /// The original pattern could not be parsed.
    Parse(String),
    /// An old name in the mapping does not exist in the pattern.
    UnknownName(String),
    /// Two distinct old names map to the same new name.
    Conflict {
        /// One of the old names.
        old: String,
        /// The new name they both map to.
        new: String,
        /// The other old name.
        also_renamed_from: String,
    },
    /// A new name collides with an existing capture group that is not renamed.
    Collision {
        /// The new name that caused the collision.
        new_name: String,
        /// The existing capture group name.
        existing_name: String,
        /// The index of the existing capture group.
        index: u32,
    },
    /// A new name is not a valid capture group name.
    InvalidName(String),
    /// The rewritten pattern failed to compile.
    Compile(String),
}

impl core::fmt::Display for RenameError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match *self {
            RenameError::Parse(ref msg) => {
                write!(f, "failed to parse pattern: {msg}")
            }
            RenameError::UnknownName(ref name) => {
                write!(
                    f,
                    "capture group `{name}` does not exist in the pattern"
                )
            }
            RenameError::Conflict {
                ref old,
                ref new,
                ref also_renamed_from,
            } => {
                write!(
                    f,
                    "capture groups `{old}` and `{also_renamed_from}` both map \
                     to `{new}`"
                )
            }
            RenameError::Collision {
                ref new_name,
                ref existing_name,
                index,
            } => {
                write!(
                    f,
                    "new name `{new_name}` collides with existing capture group \
                     `{existing_name}` (index {index})"
                )
            }
            RenameError::InvalidName(ref name) => {
                write!(f, "`{name}` is not a valid capture group name")
            }
            RenameError::Compile(ref msg) => {
                write!(f, "rewritten pattern failed to compile: {msg}")
            }
        }
    }
}

#[cfg(feature = "std")]
impl std::error::Error for RenameError {}

/// Rename the named capture groups in `pattern` according to `renames`.
///
/// `renames` is an explicit mapping from old capture group names to new
/// names. Only groups whose old name appears in the mapping are changed; all
/// other groups, character classes, escape sequences and literal text are
/// left untouched.
///
/// The mapping is validated as a whole before anything is rewritten. In
/// particular, the following are rejected up front:
///
/// * an old name that does not exist in the pattern;
/// * two distinct old names mapping to the same new name;
/// * a new name colliding with an existing group that is not renamed;
/// * a new name that is not a valid capture group name.
///
/// The rewritten pattern is only delivered once it compiles successfully.
///
/// # Examples
///
/// ```
/// use regex::rename::rename_captures;
///
/// let result = rename_captures(r"(?P<year>\d{4})-(?P<month>\d{2})", [("year", "y")]).unwrap();
/// assert_eq!(result.pattern, r"(?P<y>\d{4})-(?P<month>\d{2})");
/// let caps = result.regex.captures("2026-10").unwrap();
/// assert_eq!(&caps["y"], "2026");
/// assert_eq!(&caps["month"], "10");
/// ```
pub fn rename_captures<S1, S2, I>(
    pattern: &str,
    renames: I,
) -> Result<RenamedRegex, RenameError>
where
    S1: AsRef<str>,
    S2: AsRef<str>,
    I: IntoIterator<Item = (S1, S2)>,
{
    // Parse the pattern into an AST. All rewriting is done on the tree.
    let mut ast = Parser::new()
        .parse(pattern)
        .map_err(|e| RenameError::Parse(e.to_string()))?;

    // Collect every named group with its index and the span of its name.
    let mut groups: Vec<(String, u32, Span)> = Vec::new();
    walk_groups(&ast, &mut |g| {
        if let GroupKind::CaptureName { name, .. } = &g.kind {
            groups.push((name.name.clone(), name.index, name.span));
        }
    });

    // Build the explicit rename map. Reject the same old name being given
    // two different new names.
    let mut rename_map: BTreeMap<String, String> = BTreeMap::new();
    for (old, new) in renames {
        let old = old.as_ref().to_string();
        let new = new.as_ref().to_string();
        if let Some(existing) = rename_map.get(&old) {
            if existing != &new {
                return Err(RenameError::Conflict {
                    old: existing.clone(),
                    new: new.clone(),
                    also_renamed_from: old,
                });
            }
            continue;
        }
        rename_map.insert(old, new);
    }

    // Validate the mapping as a whole before rewriting anything.
    for (old, new) in &rename_map {
        // The old name must exist in the pattern.
        if !groups.iter().any(|(name, _, _)| name == old) {
            return Err(RenameError::UnknownName(old.clone()));
        }
        // The new name must be a valid capture group name.
        if !is_valid_capture_name(new) {
            return Err(RenameError::InvalidName(new.clone()));
        }
    }

    // Two distinct old names must not map to the same new name.
    let mut seen_new: BTreeMap<&str, &str> = BTreeMap::new();
    for (old, new) in &rename_map {
        if let Some(prev_old) = seen_new.get(new.as_str()) {
            if *prev_old != old.as_str() {
                return Err(RenameError::Conflict {
                    old: (*prev_old).to_string(),
                    new: new.clone(),
                    also_renamed_from: old.clone(),
                });
            }
        } else {
            seen_new.insert(new.as_str(), old.as_str());
        }
    }

    // A new name must not collide with an existing group that is not renamed.
    for (existing_name, index, _) in &groups {
        if rename_map.contains_key(existing_name) {
            // This group is being renamed, so its old name will disappear.
            continue;
        }
        if let Some(new_name) =
            rename_map.values().find(|n| n == &existing_name)
        {
            return Err(RenameError::Collision {
                new_name: new_name.clone(),
                existing_name: existing_name.clone(),
                index: *index,
            });
        }
    }

    // Rewrite the named groups in the AST.
    let mut migrations: BTreeMap<String, CaptureRename> = BTreeMap::new();
    walk_groups_mut(&mut ast, &mut |g| {
        if let GroupKind::CaptureName { name, .. } = &mut g.kind {
            if let Some(new_name) = rename_map.get(&name.name) {
                let old_name =
                    core::mem::replace(&mut name.name, new_name.clone());
                migrations.insert(
                    old_name.clone(),
                    CaptureRename {
                        old_name,
                        new_name: new_name.clone(),
                        index: name.index,
                        span: name.span,
                    },
                );
            }
        }
    });

    // Print the rewritten AST back to a pattern string.
    let mut pattern = String::new();
    Printer::new()
        .print(&ast, &mut pattern)
        .map_err(|e| RenameError::Compile(e.to_string()))?;

    // Only deliver if the rewritten pattern compiles successfully.
    let regex = Regex::new(&pattern)
        .map_err(|e| RenameError::Compile(e.to_string()))?;

    Ok(RenamedRegex { regex, pattern, migrations })
}

/// Recursively visit every group in `ast`.
fn walk_groups<'a>(ast: &'a Ast, f: &mut impl FnMut(&'a Group)) {
    match ast {
        Ast::Group(g) => {
            f(g);
            walk_groups(&g.ast, f);
        }
        Ast::Repetition(r) => walk_groups(&r.ast, f),
        Ast::Alternation(a) => {
            for ast in &a.asts {
                walk_groups(ast, f);
            }
        }
        Ast::Concat(c) => {
            for ast in &c.asts {
                walk_groups(ast, f);
            }
        }
        _ => {}
    }
}

/// Recursively visit every group in `ast`, mutably.
fn walk_groups_mut(ast: &mut Ast, f: &mut impl FnMut(&mut Group)) {
    match ast {
        Ast::Group(g) => {
            f(g);
            walk_groups_mut(&mut g.ast, f);
        }
        Ast::Repetition(r) => walk_groups_mut(&mut r.ast, f),
        Ast::Alternation(a) => {
            for ast in &mut a.asts {
                walk_groups_mut(ast, f);
            }
        }
        Ast::Concat(c) => {
            for ast in &mut c.asts {
                walk_groups_mut(ast, f);
            }
        }
        _ => {}
    }
}

/// Returns true if `name` is a valid capture group name.
///
/// This mirrors the rules used by the parser: the first character must be an
/// alphabetic character or underscore, and subsequent characters may be
/// alphanumeric, underscore, dot or square brackets.
fn is_valid_capture_name(name: &str) -> bool {
    let mut chars = name.chars();
    match chars.next() {
        None => false,
        Some(c) => {
            if !(c == '_' || c.is_alphabetic()) {
                return false;
            }
            chars.all(|c| {
                c == '_'
                    || c == '.'
                    || c == '['
                    || c == ']'
                    || c.is_alphanumeric()
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rename_year_to_y() {
        let result = rename_captures(
            r"(?P<year>\d{4})-(?P<month>\d{2})",
            [("year", "y")],
        )
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
        assert_eq!(migration.span.start.offset, 4);
        assert_eq!(migration.span.end.offset, 8);
    }

    #[test]
    fn matching_bytes_and_group_numbers_unchanged() {
        let original =
            Regex::new(r"(?P<year>\d{4})-(?P<month>\d{2})").unwrap();
        let renamed = rename_captures(
            r"(?P<year>\d{4})-(?P<month>\d{2})",
            [("year", "y")],
        )
        .unwrap();

        for hay in ["2026-10", "1999-01", "not a date", "1234-56"] {
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
                    // The old name resolves on the original; the new name
                    // resolves on the renamed regex to the same bytes.
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
        assert!(
            matches!(err, RenameError::Conflict { .. }),
            "expected conflict, got {err:?}"
        );
    }

    #[test]
    fn similar_names_in_text_are_not_changed() {
        // The literal text `year` and `month` must not be touched.
        let result = rename_captures(
            r"(?P<year>\d{4}) year (?P<month>\d{2}) month",
            [("year", "y")],
        )
        .unwrap();
        assert_eq!(
            result.pattern,
            r"(?P<y>\d{4}) year (?P<month>\d{2}) month"
        );
        let caps = result.regex.captures("2026 year 10 month").unwrap();
        assert_eq!(&caps["y"], "2026");
        assert_eq!(&caps["month"], "10");
    }

    #[test]
    fn character_classes_and_escapes_are_untouched() {
        let result = rename_captures(
            r"(?P<year>\d{4}) [a-z]+ \x41 \pL",
            [("year", "y")],
        )
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
        assert_eq!(outer.span.start.offset, 4);
        assert_eq!(inner.span.start.offset, 14);
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
        assert!(
            matches!(err, RenameError::Collision { .. }),
            "expected collision, got {err:?}"
        );
    }

    #[test]
    fn chained_rename_is_allowed() {
        // year -> y, y -> z: the second group is renamed, so no collision.
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
        let err = rename_captures(r"(?P<year>\d{4})", [("year", "1bad")])
            .unwrap_err();
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
        let result =
            rename_captures(r"(?<year>\d{4})", [("year", "y")]).unwrap();
        assert_eq!(result.pattern, r"(?<y>\d{4})");
        assert_eq!(&result.regex.captures("2026").unwrap()["y"], "2026");
    }

    #[test]
    fn valid_special_char_names_are_accepted() {
        // Names with dots and brackets are accepted by the compiler.
        let result =
            rename_captures(r"(?P<year>\d{4})", [("year", "y.0")]).unwrap();
        assert_eq!(result.pattern, r"(?P<y.0>\d{4})");
        assert_eq!(&result.regex.captures("2026").unwrap()["y.0"], "2026");
    }
}
