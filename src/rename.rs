/*!
Explicit renaming of capture group names.

This module provides [`rename_captures`], a syntax-tree based tool that
rewrites _only_ the names of named capture groups in a regex pattern. The
match language and the capture group numbers are preserved: every group
keeps its position in the pattern, so group `i` in the original regex is
still group `i` in the renamed regex. Only the name tokens of `(?P<name>...)`
and `(?<name>...)` groups are touched. Literal text, character classes and
escape sequences that merely _look_ like a capture name are never modified,
because rewrites are driven by the parsed syntax tree and applied at the
exact source spans of the group names.

Conflicting rename requests (two groups mapped to the same new name, a new
name that collides with an existing group that is not itself being renamed,
duplicate or unknown old names, ...) are collected and rejected as a set
before anything is built. A renamed regex is only delivered if the rewritten
pattern compiles successfully into a [`Regex`].

Along with the new [`Regex`], callers get a migration table
([`Renamed::renames`] and [`Renamed::name_map`]) mapping each old capture
name to its new name, its (unchanged) capture index and its source spans in
the original pattern, so that existing queries can be migrated.

The original regex is untouched; it keeps behaving exactly as before.

# Example

```
let renamed = regex::rename::rename_captures(
    r"(?P<year>\d{4})-(?P<month>\d{2})",
    [("year", "y")],
)?;
assert_eq!(renamed.pattern(), r"(?P<y>\d{4})-(?P<month>\d{2})");
let caps = renamed.regex().captures("2026-10").unwrap();
// The old `year` content is now fetched via `y`, still group number 1.
assert_eq!(caps.name("y").unwrap().as_str(), "2026");
assert_eq!(caps.get(1).unwrap().as_str(), "2026");
assert_eq!(renamed.new_name("year"), Some("y"));
# Ok::<(), Box<dyn std::error::Error>>(())
```
*/

use alloc::{
    collections::BTreeMap,
    string::{String, ToString},
    vec::Vec,
};

use regex_syntax::ast::{self, Ast, GroupKind};

use crate::Regex;

/// Renames capture group names in `pattern` according to `renames`.
///
/// Each `(old, new)` pair asks for the named group `old` to be renamed to
/// `new`. All renames are applied simultaneously, so swapping two names is
/// fine as long as the overall request is conflict free.
///
/// The rewrite is purely textual at the source spans of the group names
/// found in the syntax tree: nothing else in the pattern is modified, so
/// the match language and all capture group numbers are preserved.
///
/// # Errors
///
/// * [`RenameError::Syntax`] if `pattern` does not parse.
/// * [`RenameError::Conflicts`] if the rename request is inconsistent.
///   All conflicts are gathered and reported together.
/// * [`RenameError::Build`] if the rewritten pattern fails to compile into
///   a [`Regex`] (for example because a new name is not a valid capture
///   name). In that case nothing is delivered.
pub fn rename_captures<K, V, I>(
    pattern: &str,
    renames: I,
) -> Result<Renamed, RenameError>
where
    K: AsRef<str>,
    V: AsRef<str>,
    I: IntoIterator<Item = (K, V)>,
{
    // Parse first, so that every position we report is grounded in the
    // syntax tree of the original pattern.
    let ast = ast::parse::Parser::new()
        .parse(pattern)
        .map_err(|err| RenameError::Syntax(err.to_string()))?;
    let groups = match ast::visit(&ast, Collector::default()) {
        Ok(groups) => groups,
        Err(never) => match never {},
    };

    // Normalize the request and gather _all_ conflicts before rejecting.
    let mut request: BTreeMap<String, String> = BTreeMap::new();
    let mut conflicts: Vec<RenameConflict> = Vec::new();
    for (old, new) in renames {
        let (old, new) = (old.as_ref(), new.as_ref());
        if old == new {
            conflicts
                .push(RenameConflict::NameUnchanged { name: old.to_string() });
            continue;
        }
        if request.insert(old.to_string(), new.to_string()).is_some() {
            conflicts.push(
                RenameConflict::DuplicateOldName { name: old.to_string() },
            );
        }
    }

    // Invert the request to detect two groups being mapped to the same
    // new name.
    let mut new_to_olds: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for (old, new) in request.iter() {
        new_to_olds.entry(new.as_str()).or_default().push(old.as_str());
    }
    for (new, olds) in new_to_olds.iter() {
        if olds.len() > 1 {
            conflicts.push(RenameConflict::DuplicateNewName {
                name: new.to_string(),
                old_names: olds.iter().map(|old| old.to_string()).collect(),
            });
        }
    }

    // Check the request against the names that actually exist in the
    // pattern. A new name may not collide with an existing group unless
    // that group is itself being renamed away (simultaneous rename).
    for old in request.keys() {
        if !groups.iter().any(|g| g.name == *old) {
            conflicts
                .push(RenameConflict::OldNameNotFound { name: old.clone() });
        }
    }
    for new in new_to_olds.keys() {
        let exists = groups.iter().any(|g| g.name == *new);
        if exists && !request.contains_key(*new) {
            conflicts.push(RenameConflict::NewNameAlreadyExists {
                name: new.to_string(),
            });
        }
    }
    if !conflicts.is_empty() {
        return Err(RenameError::Conflicts(conflicts));
    }

    // Apply the renames at the exact source spans of the group names.
    // Character classes, escapes and literal text never appear here,
    // because only `GroupKind::CaptureName` nodes carry these spans.
    let mut edits: Vec<((usize, usize), &str)> = Vec::new();
    let mut table: Vec<CaptureRename> = Vec::new();
    for g in groups.iter() {
        if let Some(new) = request.get(&g.name) {
            edits.push((g.name_span, new.as_str()));
            table.push(CaptureRename {
                old_name: g.name.clone(),
                new_name: new.clone(),
                index: g.index,
                group_span: SourceSpan {
                    start: g.group_span.0,
                    end: g.group_span.1,
                },
                name_span: SourceSpan {
                    start: g.name_span.0,
                    end: g.name_span.1,
                },
            });
        }
    }
    edits.sort_by(|a, b| b.0.cmp(&a.0));
    let mut new_pattern = pattern.to_string();
    for ((start, end), new) in edits {
        new_pattern.replace_range(start..end, new);
    }

    // Only deliver the renamed regex if the rewritten pattern compiles.
    let regex = Regex::new(&new_pattern).map_err(RenameError::Build)?;
    Ok(Renamed { regex, pattern: new_pattern, renames: table })
}

/// The result of a successful [`rename_captures`] call.
///
/// Bundles the freshly compiled [`Regex`], the rewritten pattern and the
/// migration table describing every rename that was applied.
#[derive(Clone, Debug)]
pub struct Renamed {
    regex: Regex,
    pattern: String,
    renames: Vec<CaptureRename>,
}

impl Renamed {
    /// Returns the compiled regex with the renamed capture groups.
    ///
    /// It matches exactly the same language as the original regex and
    /// assigns the same capture group numbers; only the names changed.
    pub fn regex(&self) -> &Regex {
        &self.regex
    }

    /// Consumes this value and returns the compiled regex.
    pub fn into_regex(self) -> Regex {
        self.regex
    }

    /// Returns the rewritten pattern.
    pub fn pattern(&self) -> &str {
        &self.pattern
    }

    /// Returns the migration table: one entry per renamed group, in the
    /// source order of the groups in the original pattern.
    pub fn renames(&self) -> &[CaptureRename] {
        &self.renames
    }

    /// Returns a map from each old capture name to its new name, suitable
    /// for migrating existing queries to the renamed regex.
    pub fn name_map(&self) -> BTreeMap<&str, &str> {
        self.renames
            .iter()
            .map(|r| (r.old_name.as_str(), r.new_name.as_str()))
            .collect()
    }

    /// Returns the new name for `old_name`, or `None` if `old_name` was
    /// not renamed.
    pub fn new_name(&self, old_name: &str) -> Option<&str> {
        self.renames
            .iter()
            .find(|r| r.old_name == old_name)
            .map(|r| r.new_name.as_str())
    }

    /// Renders the migration table as a human readable text table with
    /// one row per renamed group: its (unchanged) capture index, the old
    /// and new names and the source spans in the original pattern.
    pub fn migration_table(&self) -> String {
        use core::fmt::Write;
        let mut out = String::new();
        let _ = writeln!(
            out,
            "{:<5}  {:<20}  {:<20}  {:<10}  {:<10}",
            "INDEX", "OLD", "NEW", "GROUP", "NAME"
        );
        for r in self.renames.iter() {
            let _ = writeln!(
                out,
                "{:<5}  {:<20}  {:<20}  {:<10}  {:<10}",
                r.index,
                r.old_name,
                r.new_name,
                format_span(&r.group_span),
                format_span(&r.name_span),
            );
        }
        out
    }
}

fn format_span(span: &SourceSpan) -> String {
    alloc::format!("{}..{}", span.start, span.end)
}

/// A single applied capture group rename.
///
/// Records the old and new names, the capture index (which is unchanged
/// by the rename) and the source spans of the group and of the name token
/// in the original pattern, so the origin of every rename stays traceable
/// even for deeply nested groups.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CaptureRename {
    old_name: String,
    new_name: String,
    index: u32,
    group_span: SourceSpan,
    name_span: SourceSpan,
}

impl CaptureRename {
    /// Returns the old capture group name.
    pub fn old_name(&self) -> &str {
        &self.old_name
    }

    /// Returns the new capture group name.
    pub fn new_name(&self) -> &str {
        &self.new_name
    }

    /// Returns the capture index of the renamed group. This is the same
    /// in the original and in the renamed regex.
    pub fn index(&self) -> u32 {
        self.index
    }

    /// Returns the byte span of the whole group (including parentheses)
    /// in the original pattern.
    pub fn group_span(&self) -> SourceSpan {
        self.group_span
    }

    /// Returns the byte span of the renamed name token in the original
    /// pattern.
    pub fn name_span(&self) -> SourceSpan {
        self.name_span
    }
}

/// A byte span in the original pattern string.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SourceSpan {
    start: usize,
    end: usize,
}

impl SourceSpan {
    /// Returns the start byte offset (inclusive).
    pub fn start(&self) -> usize {
        self.start
    }

    /// Returns the end byte offset (exclusive).
    pub fn end(&self) -> usize {
        self.end
    }

    /// Returns the length of the span in bytes.
    pub fn len(&self) -> usize {
        self.end - self.start
    }

    /// Returns true if the span is empty.
    pub fn is_empty(&self) -> bool {
        self.start == self.end
    }
}

/// An error returned by [`rename_captures`].
#[derive(Clone, Debug)]
#[non_exhaustive]
pub enum RenameError {
    /// The original pattern failed to parse. Contains the syntax error
    /// message.
    Syntax(String),
    /// The rename request is inconsistent. Every detected conflict is
    /// included; nothing is built or delivered in this case.
    Conflicts(Vec<RenameConflict>),
    /// The rewritten pattern failed to compile into a [`Regex`]. This
    /// happens, for example, when a new name is not a valid capture group
    /// name. Nothing is delivered in this case.
    Build(crate::Error),
}

impl core::fmt::Display for RenameError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match *self {
            RenameError::Syntax(ref msg) => {
                write!(f, "pattern failed to parse: {msg}")
            }
            RenameError::Conflicts(ref conflicts) => {
                writeln!(f, "conflicting capture group renames:")?;
                for c in conflicts.iter() {
                    writeln!(f, "  {c}")?;
                }
                Ok(())
            }
            RenameError::Build(ref err) => {
                write!(f, "renamed pattern failed to compile: {err}")
            }
        }
    }
}

#[cfg(feature = "std")]
impl std::error::Error for RenameError {}

/// A single conflict in a capture group rename request.
#[derive(Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum RenameConflict {
    /// The same old name was given more than once in the request.
    DuplicateOldName {
        /// The duplicated old name.
        name: String,
    },
    /// Two or more groups were mapped to the same new name.
    DuplicateNewName {
        /// The duplicated new name.
        name: String,
        /// The old names that were all mapped to `name`.
        old_names: Vec<String>,
    },
    /// The new name already belongs to a group in the pattern that is not
    /// itself being renamed away.
    NewNameAlreadyExists {
        /// The colliding name.
        name: String,
    },
    /// The old name does not correspond to any named group in the pattern.
    OldNameNotFound {
        /// The unknown old name.
        name: String,
    },
    /// The old and new names are identical, so the rename is a no-op.
    NameUnchanged {
        /// The unchanged name.
        name: String,
    },
}

impl core::fmt::Display for RenameConflict {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match *self {
            RenameConflict::DuplicateOldName { ref name } => {
                write!(f, "old name '{name}' was given more than once")
            }
            RenameConflict::DuplicateNewName { ref name, ref old_names } => {
                let olds = old_names
                    .iter()
                    .map(|old| alloc::format!("'{old}'"))
                    .collect::<Vec<_>>()
                    .join(", ");
                write!(
                    f,
                    "groups {olds} are all renamed to the same name '{name}'",
                )
            }
            RenameConflict::NewNameAlreadyExists { ref name } => write!(
                f,
                "new name '{name}' collides with an existing group that \
                 is not being renamed"
            ),
            RenameConflict::OldNameNotFound { ref name } => {
                write!(f, "no capture group named '{name}' in the pattern")
            }
            RenameConflict::NameUnchanged { ref name } => {
                write!(f, "old and new name are both '{name}'")
            }
        }
    }
}

#[cfg(feature = "std")]
impl std::error::Error for RenameConflict {}

/// A named capture group found in the syntax tree of the original pattern.
struct NamedGroup {
    name: String,
    index: u32,
    group_span: (usize, usize),
    name_span: (usize, usize),
}

/// Collects every named capture group, in source order, with its spans.
/// Nested groups are visited too, so their origins stay traceable.
#[derive(Default)]
struct Collector {
    groups: Vec<NamedGroup>,
}

impl ast::Visitor for Collector {
    type Output = Vec<NamedGroup>;
    type Err = core::convert::Infallible;

    fn finish(self) -> Result<Self::Output, Self::Err> {
        Ok(self.groups)
    }

    fn visit_pre(&mut self, ast: &Ast) -> Result<(), Self::Err> {
        if let Ast::Group(ref group) = *ast {
            if let GroupKind::CaptureName { ref name, .. } = group.kind {
                self.groups.push(NamedGroup {
                    name: name.name.clone(),
                    index: name.index,
                    group_span: (
                        group.span.start.offset,
                        group.span.end.offset,
                    ),
                    name_span: (
                        name.span.start.offset,
                        name.span.end.offset,
                    ),
                });
            }
        }
        Ok(())
    }
}
