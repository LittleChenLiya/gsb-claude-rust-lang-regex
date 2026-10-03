/*!
An owned snapshot of a [`bytes::Captures`](crate::bytes::Captures) value.

This module provides [`CapturesSnapshot`], an explicitly owned ("`'static`")
copy of everything a `bytes::Captures` knows: the overall match bytes, the
original haystack range of every capture group, the raw bytes of every group
and the capture group name index. Because the snapshot owns its data, the
haystack (and any reusable buffer it came from) may be reused or dropped
immediately after the snapshot is taken.

Snapshots can be written to a versioned archive with
[`CapturesSnapshot::to_archive`] and restored with
[`CapturesSnapshot::from_archive`]. Restoring validates the archive and never
re-executes a regex.
*/

use alloc::{
    string::{String, ToString},
    vec::Vec,
};
use core::ops::Range;

use regex_automata::PatternID;

use crate::regex::bytes::Captures;

/// The magic prefix of every snapshot archive.
const ARCHIVE_MAGIC: &[u8; 6] = b"RXSNAP";

/// The only archive version this implementation can read and write.
const ARCHIVE_VERSION: u32 = 1;

/// An owned snapshot of a [`bytes::Captures`](crate::bytes::Captures) value.
///
/// A snapshot copies, out of a `Captures` value, everything needed to inspect
/// a match after the haystack is gone:
///
/// * the bytes of the overall match (capture group `0`),
/// * the original haystack range (`start..end` byte offsets) of every
///   capture group,
/// * the raw bytes of every capture group (non-UTF-8 bytes are preserved
///   as-is, no UTF-8 validation is ever performed),
/// * the capture group name index.
///
/// A group that did not participate in the match is represented distinctly
/// from a group that matched the empty string: the former has no range and
/// no bytes (`None`), while the latter has an empty range (e.g. `5..5`) and
/// empty bytes (`Some(&[])`).
///
/// Since a snapshot owns all of its data, it does not borrow the haystack.
/// The haystack may be a reusable buffer that is cleared or reused for the
/// next read immediately after [`Captures::snapshot`] returns.
///
/// # Archiving
///
/// [`CapturesSnapshot::to_archive`] serializes a snapshot to a self-contained
/// `Vec<u8>` with an explicit format version, and
/// [`CapturesSnapshot::from_archive`] restores it. Restoring only validates
/// the archive; it never re-runs a regex. The original haystack ranges are
/// preserved exactly (they are never re-based to `0`), so a restored snapshot
/// still refers to offsets in the original input.
///
/// The archive format is versioned (`RXSNAP`, version `1`) and is intended
/// for short-term persistence and transport, not as a stable cross-version
/// interchange format. Archives are validated on restore: the group layout,
/// the original ranges and the saved bytes must all be mutually consistent,
/// and a trailing checksum must match. Invalid archives are rejected with a
/// [`SnapshotError`] before any snapshot is produced.
///
/// # Example
///
/// ```
/// use regex::bytes::Regex;
///
/// let re = Regex::new(r"(?-u)(?<word>[a-z]\xFF[a-z])").unwrap();
/// // Take a snapshot, then drop both the haystack and the `Captures`.
/// let snap = {
///     let haystack = b"\x00\x01a\xFFb".to_vec(); // 2-byte prefix
///     let caps = re.captures(&haystack).unwrap();
///     caps.snapshot()
/// }; // `haystack` and `caps` are dropped here.
///
/// // The snapshot is still fully readable, with original ranges intact.
/// assert_eq!(snap.full_match_span(), 2..5);
/// assert_eq!(snap.full_match_bytes(), &b"a\xFFb"[..]);
/// assert_eq!(snap.span_by_name("word"), Some(2..5));
/// assert_eq!(snap.bytes_by_name("word"), Some(&b"a\xFFb"[..]));
///
/// // Archive it and restore it later, without any regex.
/// let archive = snap.to_archive();
/// let restored = regex::bytes::CapturesSnapshot::from_archive(&archive)?;
/// assert_eq!(restored, snap);
/// # Ok::<(), regex::bytes::SnapshotError>(())
/// ```
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CapturesSnapshot {
    /// The bytes of the overall match (group 0).
    full: Vec<u8>,
    /// One entry per capture group, including group 0. `None` means the
    /// group did not participate in the match.
    groups: Vec<Option<GroupSnapshot>>,
    /// `names[i]` is the name of group `i`, if it has one. Group 0 is
    /// always `None`.
    names: Vec<Option<String>>,
}

/// The saved state of a single participating capture group.
#[derive(Clone, Debug, PartialEq, Eq)]
struct GroupSnapshot {
    /// The original haystack byte offset at which this group starts.
    start: usize,
    /// The original haystack byte offset at which this group ends.
    end: usize,
    /// The raw bytes this group matched. May be empty (an empty match),
    /// which is distinct from the group not participating at all.
    bytes: Vec<u8>,
}

/// An error that occurred while restoring a [`CapturesSnapshot`] from an
/// archive.
///
/// Every variant corresponds to a consistency check that
/// [`CapturesSnapshot::from_archive`] performs before producing a snapshot.
/// When this error is returned, no snapshot is produced and no regex is
/// executed.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SnapshotError {
    /// The archive does not start with the expected magic bytes.
    BadMagic,
    /// The archive uses an unsupported format version. The unsupported
    /// version number is included.
    UnsupportedVersion(u32),
    /// The archive ended before all declared data could be read.
    Truncated,
    /// The archive has unexpected bytes after its logical end.
    TrailingBytes,
    /// The archive's trailing checksum does not match its contents.
    ChecksumMismatch,
    /// The capture group layout is inconsistent (for example, zero groups,
    /// a missing overall match group, or a name assigned to an invalid or
    /// duplicate group index).
    Layout(String),
    /// A group's original range is inconsistent with its saved bytes or
    /// with the overall match (for example, `start > end`, a range length
    /// that differs from the saved byte count, saved bytes for the overall
    /// match that differ from the archived full match, or a group range
    /// outside the overall match range).
    Range(String),
    /// A capture group name is invalid (empty or not UTF-8).
    InvalidName(String),
}

impl CapturesSnapshot {
    /// Creates a snapshot from a [`bytes::Captures`](crate::bytes::Captures)
    /// value, copying out all group ranges, group bytes and names.
    pub(crate) fn new(caps: &Captures<'_>) -> CapturesSnapshot {
        let len = caps.len();
        let mut groups = Vec::with_capacity(len);
        for i in 0..len {
            groups.push(caps.get(i).map(|m| GroupSnapshot {
                start: m.start(),
                end: m.end(),
                bytes: m.as_bytes().to_vec(),
            }));
        }
        let names = caps
            .caps
            .group_info()
            .pattern_names(PatternID::ZERO)
            .map(|name| name.map(ToString::to_string))
            .collect();
        let full =
            groups[0].as_ref().map_or_else(Vec::new, |g| g.bytes.clone());
        CapturesSnapshot { full, groups, names }
    }

    /// Returns the total number of capture groups, including group 0 (the
    /// overall match) and groups that did not participate in the match.
    ///
    /// This is always equivalent to [`Captures::len`] of the `Captures`
    /// value this snapshot was taken from.
    #[inline]
    pub fn group_count(&self) -> usize {
        self.groups.len()
    }

    /// Returns the bytes of the overall match (capture group 0).
    #[inline]
    pub fn full_match_bytes(&self) -> &[u8] {
        &self.full
    }

    /// Returns the original haystack range of the overall match (capture
    /// group 0).
    ///
    /// The range refers to byte offsets in the original input, exactly as
    /// reported when the match occurred. It is never re-based to `0`.
    #[inline]
    pub fn full_match_span(&self) -> Range<usize> {
        let g = self.groups[0].as_ref().expect("group 0 is always present");
        g.start..g.end
    }

    /// Returns the original haystack range of the capture group at index
    /// `i`, or `None` if `i` is out of bounds or the group did not
    /// participate in the match.
    ///
    /// A group that matched the empty string returns an empty range (e.g.
    /// `5..5`), which is distinct from a non-participating group (`None`).
    #[inline]
    pub fn span(&self, i: usize) -> Option<Range<usize>> {
        let g = self.groups.get(i)?.as_ref()?;
        Some(g.start..g.end)
    }

    /// Returns the raw bytes matched by the capture group at index `i`, or
    /// `None` if `i` is out of bounds or the group did not participate in
    /// the match.
    ///
    /// A group that matched the empty string returns `Some(&[])`, which is
    /// distinct from a non-participating group (`None`). Bytes are returned
    /// exactly as they appeared in the input; they are not required to be
    /// valid UTF-8.
    #[inline]
    pub fn bytes(&self, i: usize) -> Option<&[u8]> {
        let g = self.groups.get(i)?.as_ref()?;
        Some(&g.bytes)
    }

    /// Returns the name of the capture group at index `i`, or `None` if
    /// `i` is out of bounds or the group has no name. Group 0 never has a
    /// name.
    #[inline]
    pub fn name(&self, i: usize) -> Option<&str> {
        self.names.get(i)?.as_deref()
    }

    /// Returns the index of the capture group named `name`, or `None` if
    /// there is no such group.
    ///
    /// If multiple groups share the same name, the index of the first one
    /// is returned, matching the behavior of
    /// [`Captures::name`](crate::bytes::Captures::name).
    #[inline]
    pub fn name_index(&self, name: &str) -> Option<usize> {
        self.names.iter().position(|n| n.as_deref() == Some(name))
    }

    /// Returns the original haystack range of the capture group named
    /// `name`, or `None` if there is no such group or it did not
    /// participate in the match.
    ///
    /// This is always consistent with [`CapturesSnapshot::span`]: if
    /// `snap.name_index(name)` is `Some(i)`, then
    /// `snap.span_by_name(name) == snap.span(i)`.
    #[inline]
    pub fn span_by_name(&self, name: &str) -> Option<Range<usize>> {
        self.span(self.name_index(name)?)
    }

    /// Returns the raw bytes matched by the capture group named `name`, or
    /// `None` if there is no such group or it did not participate in the
    /// match.
    ///
    /// This is always consistent with [`CapturesSnapshot::bytes`]: if
    /// `snap.name_index(name)` is `Some(i)`, then
    /// `snap.bytes_by_name(name) == snap.bytes(i)`.
    #[inline]
    pub fn bytes_by_name(&self, name: &str) -> Option<&[u8]> {
        self.bytes(self.name_index(name)?)
    }

    /// Serializes this snapshot to a versioned, self-contained archive.
    ///
    /// The archive can be restored with
    /// [`CapturesSnapshot::from_archive`]. See the [type level
    /// documentation](CapturesSnapshot#archiving) for the guarantees of the
    /// archive format.
    pub fn to_archive(&self) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(ARCHIVE_MAGIC);
        push_u32(&mut out, ARCHIVE_VERSION);
        push_u32(&mut out, self.groups.len() as u32);
        push_u32(&mut out, self.full.len() as u32);
        out.extend_from_slice(&self.full);
        for group in self.groups.iter() {
            match *group {
                None => out.push(0),
                Some(ref g) => {
                    out.push(1);
                    push_u64(&mut out, g.start as u64);
                    push_u64(&mut out, g.end as u64);
                    push_u32(&mut out, g.bytes.len() as u32);
                    out.extend_from_slice(&g.bytes);
                }
            }
        }
        let name_count =
            self.names.iter().filter(|n| n.is_some()).count() as u32;
        push_u32(&mut out, name_count);
        for (i, name) in self.names.iter().enumerate() {
            if let Some(ref name) = *name {
                push_u32(&mut out, i as u32);
                push_u32(&mut out, name.len() as u32);
                out.extend_from_slice(name.as_bytes());
            }
        }
        let checksum = fnv1a64(&out);
        push_u64(&mut out, checksum);
        out
    }

    /// Restores a snapshot previously written by
    /// [`CapturesSnapshot::to_archive`].
    ///
    /// This validates the archive before producing a snapshot:
    ///
    /// * the magic bytes and format version must be supported,
    /// * the group layout must be well formed (at least one group, the
    ///   overall match group must be present, names must refer to existing
    ///   groups at most once and never to group 0),
    /// * every group's original range must be consistent with its saved
    ///   bytes (`start <= end`, range length equals byte count) and must
    ///   lie within the overall match range,
    /// * the overall match group's saved bytes must equal the archived
    ///   full match bytes,
    /// * the trailing checksum must match and no trailing bytes may
    ///   follow it.
    ///
    /// If any check fails, an error is returned and no snapshot is
    /// produced. Ranges are restored exactly as archived (they are never
    /// re-based to `0`), and no regex is executed at any point.
    pub fn from_archive(
        archive: &[u8],
    ) -> Result<CapturesSnapshot, SnapshotError> {
        // The checksum covers everything before the final 8 bytes.
        if archive.len() < 8 {
            return Err(SnapshotError::Truncated);
        }
        let (body, checksum_bytes) = archive.split_at(archive.len() - 8);
        let want_checksum =
            u64::from_le_bytes(checksum_bytes.try_into().unwrap());
        if fnv1a64(body) != want_checksum {
            return Err(SnapshotError::ChecksumMismatch);
        }
        let mut r = Reader { rest: body };
        if r.take(ARCHIVE_MAGIC.len())? != ARCHIVE_MAGIC {
            return Err(SnapshotError::BadMagic);
        }
        let version = r.u32()?;
        if version != ARCHIVE_VERSION {
            return Err(SnapshotError::UnsupportedVersion(version));
        }
        let group_count = r.u32()? as usize;
        if group_count == 0 {
            return Err(SnapshotError::Layout(
                "archive declares zero capture groups".to_string(),
            ));
        }
        // Every group needs at least one tag byte, so a group count bigger
        // than the remaining input can never be satisfied. This also keeps
        // a corrupt count from causing a huge allocation.
        if group_count > r.rest.len() {
            return Err(SnapshotError::Truncated);
        }
        let full_len = r.u32()? as usize;
        let full = r.take(full_len)?.to_vec();
        let mut groups = Vec::with_capacity(group_count);
        for i in 0..group_count {
            let tag = r.u8()?;
            let group = match tag {
                0 => None,
                1 => {
                    let start = u64_to_usize(r.u64()?)?;
                    let end = u64_to_usize(r.u64()?)?;
                    let len = r.u32()? as usize;
                    if start > end {
                        return Err(SnapshotError::Range(alloc::format!(
                            "group {i} has start {start} after end {end}",
                        )));
                    }
                    if end - start != len {
                        return Err(SnapshotError::Range(alloc::format!(
                            "group {i} range {start}..{end} is {} bytes \
                             but {len} bytes were archived",
                            end - start,
                        )));
                    }
                    let bytes = r.take(len)?.to_vec();
                    Some(GroupSnapshot { start, end, bytes })
                }
                _ => {
                    return Err(SnapshotError::Layout(alloc::format!(
                        "group {i} has unknown participation tag {tag}",
                    )));
                }
            };
            groups.push(group);
        }
        // The overall match group must be present, and its archived bytes
        // must equal the archived full match bytes.
        let overall = groups[0].as_ref().ok_or_else(|| {
            SnapshotError::Layout(
                "overall match group (index 0) is not participating"
                    .to_string(),
            )
        })?;
        if overall.bytes != full {
            return Err(SnapshotError::Range(
                "overall match group bytes differ from archived full \
                 match bytes"
                    .to_string(),
            ));
        }
        let overall_span = overall.start..overall.end;
        // Every participating group must lie within the overall match.
        for (i, group) in groups.iter().enumerate() {
            if let Some(ref g) = *group {
                if g.start < overall_span.start || g.end > overall_span.end {
                    return Err(SnapshotError::Range(alloc::format!(
                        "group {i} range {}..{} lies outside the overall \
                         match range {}..{}",
                        g.start,
                        g.end,
                        overall_span.start,
                        overall_span.end,
                    )));
                }
            }
        }
        let name_count = r.u32()? as usize;
        // Every name entry needs at least 8 bytes of header, so a name
        // count bigger than the remaining input can never be satisfied.
        if name_count > r.rest.len() {
            return Err(SnapshotError::Truncated);
        }
        let mut names = alloc::vec![None; group_count];
        for _ in 0..name_count {
            let index = r.u32()? as usize;
            let name_len = r.u32()? as usize;
            let name_bytes = r.take(name_len)?;
            let name = core::str::from_utf8(name_bytes).map_err(|_| {
                SnapshotError::InvalidName(
                    "capture group name is not valid UTF-8".to_string(),
                )
            })?;
            if name.is_empty() {
                return Err(SnapshotError::InvalidName(
                    "capture group name is empty".to_string(),
                ));
            }
            if index >= group_count {
                return Err(SnapshotError::Layout(alloc::format!(
                    "name {name:?} assigned to out-of-bounds group index \
                     {index} (group count is {group_count})",
                )));
            }
            if index == 0 {
                return Err(SnapshotError::Layout(
                    "name assigned to the overall match group (index 0)"
                        .to_string(),
                ));
            }
            if names[index].is_some() {
                return Err(SnapshotError::Layout(alloc::format!(
                    "group {index} has been assigned more than one name",
                )));
            }
            names[index] = Some(name.to_string());
        }
        if !r.rest.is_empty() {
            return Err(SnapshotError::TrailingBytes);
        }
        Ok(CapturesSnapshot { full, groups, names })
    }
}

#[cfg(feature = "std")]
impl std::error::Error for SnapshotError {}

impl core::fmt::Display for SnapshotError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match *self {
            SnapshotError::BadMagic => {
                write!(f, "invalid snapshot archive magic bytes")
            }
            SnapshotError::UnsupportedVersion(version) => write!(
                f,
                "unsupported snapshot archive version {version} \
                 (supported: {ARCHIVE_VERSION})",
            ),
            SnapshotError::Truncated => {
                write!(f, "snapshot archive is truncated")
            }
            SnapshotError::TrailingBytes => {
                write!(f, "snapshot archive has trailing bytes")
            }
            SnapshotError::ChecksumMismatch => {
                write!(f, "snapshot archive checksum mismatch")
            }
            SnapshotError::Layout(ref msg) => {
                write!(f, "invalid snapshot archive group layout: {msg}")
            }
            SnapshotError::Range(ref msg) => {
                write!(f, "invalid snapshot archive range: {msg}")
            }
            SnapshotError::InvalidName(ref msg) => {
                write!(f, "invalid snapshot archive group name: {msg}")
            }
        }
    }
}

/// A cursor over the unvalidated bytes of an archive.
struct Reader<'a> {
    rest: &'a [u8],
}

impl<'a> Reader<'a> {
    /// Takes the next `n` bytes, failing if fewer remain.
    fn take(&mut self, n: usize) -> Result<&'a [u8], SnapshotError> {
        if self.rest.len() < n {
            return Err(SnapshotError::Truncated);
        }
        let (head, tail) = self.rest.split_at(n);
        self.rest = tail;
        Ok(head)
    }

    /// Reads a single byte.
    fn u8(&mut self) -> Result<u8, SnapshotError> {
        Ok(self.take(1)?[0])
    }

    /// Reads a little-endian `u32`.
    fn u32(&mut self) -> Result<u32, SnapshotError> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }

    /// Reads a little-endian `u64`.
    fn u64(&mut self) -> Result<u64, SnapshotError> {
        Ok(u64::from_le_bytes(self.take(8)?.try_into().unwrap()))
    }
}

/// Converts an archived `u64` offset to a `usize`.
fn u64_to_usize(n: u64) -> Result<usize, SnapshotError> {
    usize::try_from(n).map_err(|_| {
        SnapshotError::Range(alloc::format!(
            "archived offset {n} does not fit in a usize"
        ))
    })
}

/// Appends a little-endian `u32` to `out`.
fn push_u32(out: &mut Vec<u8>, n: u32) {
    out.extend_from_slice(&n.to_le_bytes());
}

/// Appends a little-endian `u64` to `out`.
fn push_u64(out: &mut Vec<u8>, n: u64) {
    out.extend_from_slice(&n.to_le_bytes());
}

/// Computes the FNV-1a 64-bit hash of `data`.
///
/// This is used as the archive checksum. It is not cryptographic; it exists
/// to detect corruption and tampering before any structural validation is
/// attempted.
fn fnv1a64(data: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf29ce484222325;
    for &b in data {
        hash ^= u64::from(b);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}
