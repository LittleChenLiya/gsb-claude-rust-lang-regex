/*!
Shared machinery for precompiled replacement plans.

A replacement plan parses a replacement template (using the exact same `$`
syntax as [`Captures::expand`](crate::Captures::expand)) up front, resolves
every capture group reference against the group layout of a specific
[`Regex`](crate::Regex) and reports unknown references as an error at
preparation time instead of silently interpolating them as the empty string
at replacement time.

The public string and bytes APIs built on top of this module live in
[`crate::regex::string`] and [`crate::regex::bytes`] respectively.
*/

use alloc::{string::String, vec::Vec};

use core::fmt;

/// An error that occurred while preparing or applying a
/// [`ReplacementPlan`](crate::ReplacementPlan).
///
/// A plan can fail in exactly two ways, and both are reported before any
/// replacement is written:
///
/// * While a plan is being prepared, every capture group reference in the
///   template is resolved against the group layout of the regex. A reference
///   to a group that does not exist (an unknown name or an out-of-range
///   index) is an error at preparation time.
/// * When a plan is applied, the group layout of the regex given must match
///   the layout of the regex the plan was prepared with. Two regexes with
///   the same number of groups but different group names (or a different
///   number of groups) are not interchangeable.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReplacementPlanError {
    kind: ReplacementPlanErrorKind,
}

/// The kind of a [`ReplacementPlanError`].
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReplacementPlanErrorKind {
    /// The template referenced a capture group (by name or by index) that
    /// does not exist in the regex the plan was prepared with.
    ///
    /// This is detected while the plan is being prepared, before any
    /// replacement happens.
    UnknownGroup {
        /// The offending reference, e.g. `missing` for `$missing` or `5`
        /// for `$5`.
        reference: String,
    },
    /// The plan was applied to a regex whose capture group layout differs
    /// from the layout of the regex the plan was prepared with.
    ///
    /// This is detected before any replacement happens.
    LayoutMismatch,
}

impl ReplacementPlanError {
    /// Returns the kind of this error.
    pub fn kind(&self) -> &ReplacementPlanErrorKind {
        &self.kind
    }
}

#[cfg(feature = "std")]
impl std::error::Error for ReplacementPlanError {}

impl fmt::Display for ReplacementPlanError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.kind {
            ReplacementPlanErrorKind::UnknownGroup { ref reference } => {
                write!(
                    f,
                    "replacement template references unknown capture \
                     group: ${}",
                    reference,
                )
            }
            ReplacementPlanErrorKind::LayoutMismatch => {
                write!(
                    f,
                    "replacement plan was prepared with a different \
                     capture group layout (group count and names must \
                     match exactly)",
                )
            }
        }
    }
}

impl From<ReplacementPlanErrorKind> for ReplacementPlanError {
    fn from(kind: ReplacementPlanErrorKind) -> ReplacementPlanError {
        ReplacementPlanError { kind }
    }
}

/// A capture group reference parsed out of a replacement template.
#[derive(Debug)]
pub(crate) enum Ref {
    Named(String),
    Number(usize),
}

/// A raw (unresolved) segment of a replacement template.
#[derive(Debug)]
pub(crate) enum RawSegment {
    /// Literal bytes to emit as is.
    Lit(Vec<u8>),
    /// A reference to a capture group, not yet resolved to an index.
    Ref(Ref),
}

/// A resolved segment of a replacement plan.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Segment {
    /// Literal bytes to emit as is.
    Lit(Vec<u8>),
    /// The index of the capture group whose value should be emitted. A
    /// group that did not participate in a match emits nothing.
    Group(usize),
}

/// Parses a replacement template into raw segments.
///
/// This follows exactly the same rules as the interpolation done by
/// `Captures::expand`: `$$` is a literal `$`, `$name`/`$1` are unbraced
/// references (longest possible parse over `[0-9A-Za-z_]`), `${...}` is a
/// braced reference, and a `$` that doesn't start a valid reference is a
/// literal `$`.
pub(crate) fn parse(template: &[u8]) -> Vec<RawSegment> {
    let mut segs = Vec::new();
    let mut lit: Vec<u8> = Vec::new();
    let mut rep = template;
    while !rep.is_empty() {
        match rep.iter().position(|&b| b == b'$') {
            None => {
                lit.extend_from_slice(rep);
                break;
            }
            Some(i) => {
                lit.extend_from_slice(&rep[..i]);
                rep = &rep[i..];
            }
        }
        // Handle escaping of '$'.
        if rep.get(1).map_or(false, |&b| b == b'$') {
            lit.push(b'$');
            rep = &rep[2..];
            continue;
        }
        match find_cap_ref(rep) {
            None => {
                lit.push(b'$');
                rep = &rep[1..];
            }
            Some((cap_ref, end)) => {
                if !lit.is_empty() {
                    segs.push(RawSegment::Lit(core::mem::take(&mut lit)));
                }
                segs.push(RawSegment::Ref(cap_ref));
                rep = &rep[end..];
            }
        }
    }
    if !lit.is_empty() {
        segs.push(RawSegment::Lit(lit));
    }
    segs
}

/// Resolves every reference in the parsed template against the given group
/// layout (names indexed by group number, with group 0 unnamed).
///
/// Returns an error if any reference points at a group that doesn't exist.
pub(crate) fn resolve(
    raw: Vec<RawSegment>,
    names: &[Option<&str>],
) -> Result<Vec<Segment>, ReplacementPlanError> {
    let mut segs = Vec::with_capacity(raw.len());
    for seg in raw {
        match seg {
            RawSegment::Lit(lit) => segs.push(Segment::Lit(lit)),
            RawSegment::Ref(Ref::Number(index)) => {
                if index < names.len() {
                    segs.push(Segment::Group(index));
                } else {
                    let reference = alloc::string::ToString::to_string(&index);
                    let kind =
                        ReplacementPlanErrorKind::UnknownGroup { reference };
                    return Err(kind.into());
                }
            }
            RawSegment::Ref(Ref::Named(name)) => {
                let index =
                    names.iter().position(|n| *n == Some(name.as_str()));
                match index {
                    Some(index) => segs.push(Segment::Group(index)),
                    None => {
                        let kind = ReplacementPlanErrorKind::UnknownGroup {
                            reference: name,
                        };
                        return Err(kind.into());
                    }
                }
            }
        }
    }
    Ok(segs)
}

/// Parses a possible reference to a capture group at the start of `rep`
/// (which must begin with `$`). Returns the reference and the offset just
/// past it. This mirrors the parsing done by the interpolation routines in
/// `regex-automata`.
fn find_cap_ref(rep: &[u8]) -> Option<(Ref, usize)> {
    if rep.len() <= 1 || rep[0] != b'$' {
        return None;
    }
    if rep[1] == b'{' {
        return find_cap_ref_braced(rep);
    }
    let mut cap_end = 1;
    while rep.get(cap_end).map_or(false, |&b| is_valid_cap_letter(b)) {
        cap_end += 1;
    }
    if cap_end == 1 {
        return None;
    }
    // We just verified that the range 1..cap_end is valid ASCII, so it must
    // therefore be valid UTF-8.
    let cap = core::str::from_utf8(&rep[1..cap_end])
        .expect("valid UTF-8 capture name");
    let cap_ref = match cap.parse::<usize>() {
        Ok(i) => Ref::Number(i),
        Err(_) => Ref::Named(alloc::string::ToString::to_string(cap)),
    };
    Some((cap_ref, cap_end))
}

/// Looks for a braced reference, e.g., `${foo1}`. Assumes `rep` starts with
/// `${`.
fn find_cap_ref_braced(rep: &[u8]) -> Option<(Ref, usize)> {
    let start = 2;
    let mut i = start;
    while rep.get(i).map_or(false, |&b| b != b'}') {
        i += 1;
    }
    if !rep.get(i).map_or(false, |&b| b == b'}') {
        return None;
    }
    // When looking at braced names, we don't put any restrictions on the
    // name, so it's possible it could be invalid UTF-8. But a capture group
    // name can never be invalid UTF-8, so if we have invalid UTF-8, then we
    // treat it as if no reference was found at all (matching interpolation
    // semantics).
    let cap = match core::str::from_utf8(&rep[start..i]) {
        Err(_) => return None,
        Ok(cap) => cap,
    };
    let cap_ref = match cap.parse::<usize>() {
        Ok(i) => Ref::Number(i),
        Err(_) => Ref::Named(alloc::string::ToString::to_string(cap)),
    };
    Some((cap_ref, i + 1))
}

/// Returns true if and only if the given byte is allowed in a capture name
/// written in non-brace form.
fn is_valid_cap_letter(b: u8) -> bool {
    matches!(b, b'0'..=b'9' | b'a'..=b'z' | b'A'..=b'Z' | b'_')
}

#[cfg(test)]
mod tests {
    use alloc::{string::ToString, vec, vec::Vec};

    use super::*;

    fn parse_ok(template: &str, names: &[Option<&str>]) -> Vec<Segment> {
        resolve(parse(template.as_bytes()), names).unwrap()
    }

    #[test]
    fn parse_literal() {
        let want = vec![Segment::Lit(b"abc".to_vec())];
        assert_eq!(parse_ok("abc", &[]), want);
        assert_eq!(parse_ok("", &[]), vec![]);
    }

    #[test]
    fn parse_dollar_escape() {
        assert_eq!(parse_ok("$$", &[]), vec![Segment::Lit(b"$".to_vec())]);
        let want = vec![Segment::Lit(b"$$".to_vec())];
        assert_eq!(parse_ok("$$$$", &[]), want);
        let want = vec![Segment::Lit(b"a$b".to_vec())];
        assert_eq!(parse_ok("a$$b", &[]), want);
        // A trailing '$' and a '$' not starting a reference are literals.
        assert_eq!(parse_ok("$", &[]), vec![Segment::Lit(b"$".to_vec())]);
        let want = vec![Segment::Lit(b"$-".to_vec())];
        assert_eq!(parse_ok("$-", &[]), want);
    }

    #[test]
    fn parse_refs() {
        let names = &[None, Some("name"), None];
        let want = vec![
            Segment::Group(1),
            Segment::Lit(b"-".to_vec()),
            Segment::Group(2),
        ];
        assert_eq!(parse_ok("$name-$2", names), want);
        assert_eq!(parse_ok("${name}", names), vec![Segment::Group(1)]);
        assert_eq!(parse_ok("$0", names), vec![Segment::Group(0)]);
        let want = vec![Segment::Group(2), Segment::Lit(b"x".to_vec())];
        assert_eq!(parse_ok("${2}x", names), want);
    }

    #[test]
    fn resolve_unknown_group() {
        let err = resolve(parse(b"$missing"), &[None]).unwrap_err();
        let kind = ReplacementPlanErrorKind::UnknownGroup {
            reference: "missing".to_string(),
        };
        assert_eq!(err.kind(), &kind);
        let err = resolve(parse(b"$5"), &[None, None]).unwrap_err();
        let kind = ReplacementPlanErrorKind::UnknownGroup {
            reference: "5".to_string(),
        };
        assert_eq!(err.kind(), &kind);
    }
}
