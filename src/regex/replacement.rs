/*!
Precompiled replacement templates.

This module provides the shared parsing logic for [`ReplacementPlan`], which
is a replacement template that has been precompiled against a specific regex's
capture group layout.

The template format supported here is identical to the one documented on
[`Regex::replace`](crate::Regex::replace): `$name` and `$index` refer to
capture groups, `${name}` and `${index}` provide braced variants, and `$$`
writes a literal `$`. Unlike the free-form interpolation used by the
`Replacer` trait, however, a plan is validated at compile time: references to
capture groups that do not exist in the regex are reported as errors before
any replacement is performed.
*/

use alloc::vec::Vec;

use crate::error::Error;

/// A single chunk of a compiled replacement template.
///
/// A replacement template is compiled into a sequence of chunks. Literal
/// chunks are emitted verbatim, while group chunks are replaced with the
/// substring matched by the corresponding capture group (or the empty string
/// if the group did not participate in the match).
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum Chunk {
    /// A literal sequence of bytes to emit verbatim.
    ///
    /// For the top-level (`&str`) regex, this is always valid UTF-8. For the
    /// bytes regex, it may be arbitrary bytes.
    Literal(Vec<u8>),
    /// A reference to the capture group at the given index.
    Group(usize),
}

/// A parsed reference to a capture group in a replacement template.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Ref<'a> {
    /// A named reference, e.g., `$name` or `${name}`.
    Named(&'a str),
    /// A numbered reference, e.g., `$1` or `${1}`.
    Number(usize),
}

/// Parses a possible reference to a capture group name in the given text,
/// starting at the beginning of `replacement`.
///
/// If no such valid reference could be found, `None` is returned.
///
/// This is a faithful copy of the `find_cap_ref` routine in `regex-automata`'s
/// `util::interpolate` module. It is duplicated here so that a plan can
/// precompile the template without depending on the internal interpolation
/// routines. The two implementations must stay in sync.
fn find_cap_ref(replacement: &[u8]) -> Option<(Ref<'_>, usize)> {
    let mut i = 0;
    let rep: &[u8] = replacement;
    if rep.len() <= 1 || rep[0] != b'$' {
        return None;
    }
    i += 1;
    if rep[i] == b'{' {
        return find_cap_ref_braced(rep, i + 1);
    }
    let mut cap_end = i;
    while rep.get(cap_end).copied().map_or(false, is_valid_cap_letter) {
        cap_end += 1;
    }
    if cap_end == i {
        return None;
    }
    // We just verified that the range 0..cap_end is valid ASCII, so it must
    // therefore be valid UTF-8. If we really cared, we could avoid this UTF-8
    // check via an unchecked conversion or by parsing the number straight from
    // &[u8].
    let cap = core::str::from_utf8(&rep[i..cap_end])
        .expect("valid UTF-8 capture name");
    Some((
        match cap.parse::<usize>() {
            Ok(i) => Ref::Number(i),
            Err(_) => Ref::Named(cap),
        },
        cap_end,
    ))
}

/// Looks for a braced reference, e.g., `${foo1}`. This assumes that an opening
/// brace has been found at `i-1` in `rep`. This then looks for a closing
/// brace and returns the capture reference within the brace.
fn find_cap_ref_braced(rep: &[u8], mut i: usize) -> Option<(Ref<'_>, usize)> {
    assert_eq!(b'{', rep[i.checked_sub(1).unwrap()]);
    let start = i;
    while rep.get(i).map_or(false, |&b| b != b'}') {
        i += 1;
    }
    if !rep.get(i).map_or(false, |&b| b == b'}') {
        return None;
    }
    // When looking at braced names, we don't put any restrictions on the name,
    // so it's possible it could be invalid UTF-8. But a capture group name
    // can never be invalid UTF-8, so if we have invalid UTF-8, then we can
    // safely return None.
    let cap = match core::str::from_utf8(&rep[start..i]) {
        Err(_) => return None,
        Ok(cap) => cap,
    };
    Some((
        match cap.parse::<usize>() {
            Ok(i) => Ref::Number(i),
            Err(_) => Ref::Named(cap),
        },
        i + 1,
    ))
}

/// Returns true if and only if the given byte is allowed in a capture name
/// written in non-brace form.
fn is_valid_cap_letter(b: u8) -> bool {
    matches!(b, b'0'..=b'9' | b'a'..=b'z' | b'A'..=b'Z' | b'_')
}

/// Compiles a replacement template into a sequence of chunks.
///
/// `group_len` is the total number of capture groups in the regex (including
/// the implicit group at index 0). `name_to_index` maps a capture group name
/// to its index.
///
/// Returns an error if the template references a capture group (by index or
/// name) that does not exist in the regex. This validation happens entirely
/// at compile time, before any replacement is performed.
pub(crate) fn compile(
    template: &[u8],
    group_len: usize,
    mut name_to_index: impl FnMut(&str) -> Option<usize>,
) -> Result<Vec<Chunk>, Error> {
    let mut chunks = Vec::new();
    let mut rest = template;
    while !rest.is_empty() {
        match crate::find_byte::find_byte(b'$', rest) {
            None => {
                chunks.push(Chunk::Literal(rest.to_vec()));
                break;
            }
            Some(i) => {
                if i > 0 {
                    chunks.push(Chunk::Literal(rest[..i].to_vec()));
                }
                rest = &rest[i..];
            }
        }
        // Handle escaping of '$'.
        if rest.get(1).map_or(false, |&b| b == b'$') {
            chunks.push(Chunk::Literal(alloc::vec![b'$']));
            rest = &rest[2..];
            continue;
        }
        debug_assert!(!rest.is_empty());
        let (cap_ref, end) = match find_cap_ref(rest) {
            Some(cap_ref) => cap_ref,
            None => {
                // A lone `$` (not followed by a valid reference) is treated
                // as a literal `$`, matching the free-form interpolation
                // behavior.
                chunks.push(Chunk::Literal(alloc::vec![b'$']));
                rest = &rest[1..];
                continue;
            }
        };
        rest = &rest[end..];
        match cap_ref {
            Ref::Number(i) => {
                if i >= group_len {
                    return Err(Error::Replacement(alloc::format!(
                        "replacement template references capture group index {i}, \
                         but the regex only has {group_len} capture group(s)"
                    )));
                }
                chunks.push(Chunk::Group(i));
            }
            Ref::Named(name) => match name_to_index(name) {
                Some(i) => chunks.push(Chunk::Group(i)),
                None => {
                    return Err(Error::Replacement(alloc::format!(
                        "replacement template references capture group name {name:?}, \
                         but the regex has no capture group with that name"
                    )));
                }
            },
        }
    }
    Ok(chunks)
}
