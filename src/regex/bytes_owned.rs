//! Owned snapshots of byte-string capture groups.
//!
//! This module provides [`OwnedCaptures`], an owned snapshot of a
//! [`Captures`](crate::bytes::Captures) value. Unlike `Captures`, which
//! borrows the haystack it matched against, `OwnedCaptures` stores a copy of
//! every matched byte together with the *original* byte range of each capture
//! group in the haystack. This makes it suitable for use cases where the
//! haystack buffer is reused or dropped after matching: the captures remain
//! readable indefinitely and never borrow the caller's buffer.
//!
//! An `OwnedCaptures` can also be [archived](OwnedCaptures::archive) into a
//! versioned, self-contained byte string and [restored](OwnedCaptures::restore)
//! later. Restoration validates the archive's magic, version, group layout,
//! group names, ranges and the consistency between ranges and the saved
//! bytes. It never re-runs the regex and never resets ranges to start-at-zero
//! sliced ranges.
//!
//! # Example
//!
//! ```
//! use regex::bytes::{OwnedCaptures, Regex};
//!
//! let re = Regex::new(r"(?<key>[a-z]+)=(?<val>[a-z]+)").unwrap();
//! let mut buf = b"\x00\x01name=value".to_vec();
//! let caps = re.captures(&buf).unwrap();
//! let owned = caps.into_owned();
//!
//! // The original buffer can be dropped (or reused); the snapshot stays
//! // readable.
//! drop(buf);
//! assert_eq!(owned.name("val").unwrap().as_bytes(), b"value");
//! assert_eq!(owned.name("val").unwrap().range(), 7..12);
//!
//! // Archive and restore without re-running the regex.
//! let archive = owned.archive();
//! let restored = OwnedCaptures::restore(&archive).unwrap();
//! assert_eq!(restored, owned);
//! ```

use alloc::{string::String, vec::Vec};
use core::ops::Range;

use regex_automata::PatternID;

use super::bytes::Captures;

/// The version of the archive format produced by
/// [`OwnedCaptures::archive`] and accepted by [`OwnedCaptures::restore`].
///
/// Future revisions of the archive format will bump this number. Archives
/// carrying an unknown version are rejected.
pub const ARCHIVE_VERSION: u16 = 1;

/// The magic number identifying a captures archive.
const ARCHIVE_MAGIC: &[u8; 4] = b"RgCp";

/// The reserved flags field of an archive. Currently always zero.
const ARCHIVE_FLAGS: u16 = 0;

/// The number of bytes in the fixed-size archive header.
const ARCHIVE_HEADER_LEN: usize = 4 + 2 + 2 + 8 + 8;

/// An owned snapshot of the capture groups from a single regex match.
///
/// This is the owned counterpart of [`Captures`](crate::bytes::Captures). It
/// stores:
///
/// * the bytes matched by every capture group (including the full match at
///   group index `0`), preserved byte-for-byte even when they are not valid
///   UTF-8;
/// * the *original* byte range of each group in the haystack, where `None`
///   means the group did not participate in the match and `Some(range)` means
///   it did (an empty `range` distinguishes a participating group that matched
///   the empty string from a missing group);
/// * the mapping from capture group names to their group indices.
///
/// Because an `OwnedCaptures` does not borrow the haystack, it can be kept
/// after the haystack buffer has been reused or dropped. It can also be
/// [archived](OwnedCaptures::archive) to bytes and
/// [restored](OwnedCaptures::restore) later.
///
/// Values of this type are created by calling
/// [`Captures::into_owned`](crate::bytes::Captures::into_owned).
///
/// # Example
///
/// ```
/// use regex::bytes::Regex;
///
/// let re = Regex::new(r"(?<key>[a-z]+)=(?<val>[a-z]+)").unwrap();
/// let mut buf = b"\x00\x01name=value".to_vec();
/// let caps = re.captures(&buf).unwrap();
/// let owned = caps.into_owned();
///
/// drop(buf); // the buffer can be released
/// assert_eq!(owned.name("key").unwrap().as_bytes(), b"name");
/// assert_eq!(owned.name("val").unwrap().range(), 7..12);
/// ```
#[derive(Clone, PartialEq, Eq)]
pub struct OwnedCaptures {
    /// Per-group data. `None` means the group did not participate in the
    /// match. Index `0` is always present and is the full match.
    groups: Vec<Option<OwnedGroup>>,
    /// The capture group names, paired with the index of the group they name.
    names: Vec<(String, usize)>,
}

/// The data for a single participating capture group.
#[derive(Clone, PartialEq, Eq)]
struct OwnedGroup {
    /// The original byte range of the group in the haystack. This is never
    /// reset to a start-at-zero sliced range.
    range: Range<usize>,
    /// The bytes the group matched.
    bytes: Vec<u8>,
}

impl OwnedCaptures {
    /// Creates an owned snapshot from a borrowed [`Captures`].
    pub(crate) fn from_captures(caps: &Captures<'_>) -> OwnedCaptures {
        let groups = caps
            .iter()
            .map(|group| {
                group.map(|m| OwnedGroup {
                    range: m.range(),
                    bytes: m.as_bytes().to_vec(),
                })
            })
            .collect();
        let names = caps
            .caps
            .group_info()
            .pattern_names(PatternID::ZERO)
            .enumerate()
            .filter_map(|(i, name)| name.map(|n| (String::from(n), i)))
            .collect();
        OwnedCaptures { groups, names }
    }

    /// Returns the match associated with the capture group at index `i`. If
    /// `i` does not correspond to a capture group, or if the capture group did
    /// not participate in the match, then `None` is returned.
    ///
    /// When `i == 0`, this is guaranteed to return a non-`None` value.
    ///
    /// # Example
    ///
    /// ```
    /// use regex::bytes::Regex;
    ///
    /// let re = Regex::new(r"[a-z]+(?:([0-9]+)|([A-Z]+))").unwrap();
    /// let owned = re.captures(b"abc123").unwrap().into_owned();
    ///
    /// assert_eq!(owned.get(1).unwrap().as_bytes(), b"123");
    /// assert_eq!(owned.get(2), None); // group did not participate
    /// ```
    #[inline]
    pub fn get(&self, i: usize) -> Option<OwnedMatch<'_>> {
        self.groups.get(i).and_then(|g| g.as_ref()).map(|g| OwnedMatch {
            start: g.range.start,
            end: g.range.end,
            bytes: &g.bytes,
        })
    }

    /// Returns the overall match for the capture.
    ///
    /// This returns the match for index `0`. That is, it is equivalent to
    /// `owned.get(0).unwrap()`.
    ///
    /// # Example
    ///
    /// ```
    /// use regex::bytes::Regex;
    ///
    /// let re = Regex::new(r"[a-z]+([0-9]+)").unwrap();
    /// let owned = re.captures(b"   abc123-def").unwrap().into_owned();
    ///
    /// assert_eq!(owned.get_match().as_bytes(), b"abc123");
    /// ```
    #[inline]
    pub fn get_match(&self) -> OwnedMatch<'_> {
        self.get(0).expect("capture group 0 is always present in a match")
    }

    /// Returns the match associated with the capture group named `name`. If
    /// `name` isn't a valid capture group or it refers to a group that didn't
    /// match, then `None` is returned.
    ///
    /// # Example
    ///
    /// ```
    /// use regex::bytes::Regex;
    ///
    /// let re = Regex::new(
    ///     r"[a-z]+(?:(?<numbers>[0-9]+)|(?<letters>[A-Z]+))",
    /// ).unwrap();
    /// let owned = re.captures(b"abc123").unwrap().into_owned();
    ///
    /// assert_eq!(owned.name("numbers").unwrap().as_bytes(), b"123");
    /// assert_eq!(owned.name("letters"), None);
    /// ```
    #[inline]
    pub fn name(&self, name: &str) -> Option<OwnedMatch<'_>> {
        self.names
            .iter()
            .find(|(n, _)| n == name)
            .and_then(|(_, i)| self.get(*i))
    }

    /// Returns the total number of capture groups. This includes both matching
    /// and non-matching groups.
    ///
    /// The length returned is always equivalent to the number of elements
    /// yielded by [`OwnedCaptures::iter`]. Consequently, the length is always
    /// greater than zero since every `OwnedCaptures` value always includes the
    /// match for the entire regex.
    ///
    /// # Example
    ///
    /// ```
    /// use regex::bytes::Regex;
    ///
    /// let re = Regex::new(r"(\w)(\d)?(\w)").unwrap();
    /// let owned = re.captures(b"AZ").unwrap().into_owned();
    /// assert_eq!(owned.len(), 4);
    /// ```
    #[inline]
    pub fn len(&self) -> usize {
        self.groups.len()
    }

    /// Returns an iterator over all capture groups. This includes both matching
    /// and non-matching groups.
    ///
    /// The iterator always yields at least one matching group: the first group
    /// (at index `0`) with no name. Subsequent groups are returned in the
    /// order of their opening parenthesis in the regex.
    ///
    /// The elements yielded have type `Option<OwnedMatch<'_>>`, where a
    /// non-`None` value is present if the capture group matches.
    ///
    /// # Example
    ///
    /// ```
    /// use regex::bytes::Regex;
    ///
    /// let re = Regex::new(r"(\w)(\d)?(\w)").unwrap();
    /// let owned = re.captures(b"AZ").unwrap().into_owned();
    ///
    /// let mut it = owned.iter();
    /// assert_eq!(it.next().unwrap().map(|m| m.as_bytes()), Some(&b"AZ"[..]));
    /// assert_eq!(it.next().unwrap().map(|m| m.as_bytes()), Some(&b"A"[..]));
    /// assert_eq!(it.next().unwrap().map(|m| m.as_bytes()), None);
    /// assert_eq!(it.next().unwrap().map(|m| m.as_bytes()), Some(&b"Z"[..]));
    /// assert_eq!(it.next(), None);
    /// ```
    #[inline]
    pub fn iter(&self) -> OwnedCaptureMatches<'_> {
        OwnedCaptureMatches { caps: self, indices: 0..self.groups.len() }
    }

    /// Serializes this snapshot into a versioned, self-contained archive.
    ///
    /// The archive contains the full match bytes, every group's original byte
    /// range and the named group index. It can be passed to
    /// [`OwnedCaptures::restore`] to recover an equivalent `OwnedCaptures`
    /// without re-running the regex.
    ///
    /// The archive format is versioned by [`ARCHIVE_VERSION`].
    ///
    /// # Example
    ///
    /// ```
    /// use regex::bytes::{OwnedCaptures, Regex};
    ///
    /// let re = Regex::new(r"(?<v>\d+)").unwrap();
    /// let owned = re.captures(b"abc123").unwrap().into_owned();
    /// let archive = owned.archive();
    ///
    /// let restored = OwnedCaptures::restore(&archive).unwrap();
    /// assert_eq!(restored, owned);
    /// ```
    pub fn archive(&self) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(ARCHIVE_MAGIC);
        out.extend_from_slice(&ARCHIVE_VERSION.to_le_bytes());
        out.extend_from_slice(&ARCHIVE_FLAGS.to_le_bytes());
        out.extend_from_slice(&(self.groups.len() as u64).to_le_bytes());
        out.extend_from_slice(&(self.names.len() as u64).to_le_bytes());
        for group in &self.groups {
            match group {
                None => out.push(0),
                Some(group) => {
                    out.push(1);
                    out.extend_from_slice(&(group.range.start as u64).to_le_bytes());
                    out.extend_from_slice(&(group.range.end as u64).to_le_bytes());
                    out.extend_from_slice(&(group.bytes.len() as u64).to_le_bytes());
                    out.extend_from_slice(&group.bytes);
                }
            }
        }
        for (name, index) in &self.names {
            out.extend_from_slice(&(name.len() as u64).to_le_bytes());
            out.extend_from_slice(name.as_bytes());
            out.extend_from_slice(&(*index as u64).to_le_bytes());
        }
        out
    }

    /// Restores a snapshot from an archive produced by
    /// [`OwnedCaptures::archive`].
    ///
    /// Restoration validates the archive before accepting it:
    ///
    /// * the magic and [version](ARCHIVE_VERSION) must match;
    /// * the group layout must be well formed (at least one group, group `0`
    ///   present, no more names than groups);
    /// * every group range must be valid (`start <= end`), contained within
    ///   the full match range and consistent with the saved byte length;
    /// * every name must be valid UTF-8, non-empty, unique and refer to a
    ///   valid non-zero group index;
    /// * there must be no trailing or truncated data.
    ///
    /// A malformed or tampered archive is rejected with an [`ArchiveError`].
    /// Restoration never re-runs the regex and never resets group ranges to
    /// start-at-zero sliced ranges; the original ranges are recovered exactly.
    ///
    /// # Example
    ///
    /// ```
    /// use regex::bytes::{OwnedCaptures, Regex};
    ///
    /// let re = Regex::new(r"(?<v>\d+)").unwrap();
    /// let owned = re.captures(b"abc123").unwrap().into_owned();
    /// let archive = owned.archive();
    ///
    /// let restored = OwnedCaptures::restore(&archive).unwrap();
    /// assert_eq!(restored.get(1).unwrap().range(), 3..6); // original range
    ///
    /// // A tampered archive is rejected.
    /// let mut tampered = archive.clone();
    /// tampered[0] ^= 0xFF;
    /// assert!(OwnedCaptures::restore(&tampered).is_err());
    /// ```
    pub fn restore(archive: &[u8]) -> Result<OwnedCaptures, ArchiveError> {
        if archive.len() < ARCHIVE_HEADER_LEN {
            return Err(ArchiveError::new(ArchiveErrorKind::Truncated));
        }
        let mut cursor = Cursor { buf: archive, pos: 0 };

        let magic = cursor.take(4)?;
        if magic != ARCHIVE_MAGIC {
            return Err(ArchiveError::new(ArchiveErrorKind::BadMagic));
        }
        let version = cursor.u16()?;
        if version != ARCHIVE_VERSION {
            return Err(ArchiveError::new(ArchiveErrorKind::UnsupportedVersion(
                version,
            )));
        }
        let flags = cursor.u16()?;
        if flags != ARCHIVE_FLAGS {
            return Err(ArchiveError::new(ArchiveErrorKind::BadFlags(flags)));
        }
        let group_count = cursor.u64()?;
        let name_count = cursor.u64()?;
        let group_count = to_usize(group_count, ArchiveErrorKind::InvalidGroupCount)?;
        let name_count = to_usize(name_count, ArchiveErrorKind::InvalidName)?;
        if group_count == 0 || group_count > archive.len() {
            return Err(ArchiveError::new(ArchiveErrorKind::InvalidGroupCount));
        }
        if name_count > group_count {
            return Err(ArchiveError::new(ArchiveErrorKind::InvalidName));
        }

        let mut groups = Vec::with_capacity(group_count);
        for _ in 0..group_count {
            match cursor.u8()? {
                0 => groups.push(None),
                1 => {
                    let start = to_usize(cursor.u64()?, ArchiveErrorKind::InvalidRange)?;
                    let end = to_usize(cursor.u64()?, ArchiveErrorKind::InvalidRange)?;
                    let len = to_usize(cursor.u64()?, ArchiveErrorKind::BytesLengthMismatch)?;
                    if start > end || end - start != len {
                        return Err(ArchiveError::new(
                            ArchiveErrorKind::BytesLengthMismatch,
                        ));
                    }
                    let bytes = cursor.take(len)?.to_vec();
                    groups.push(Some(OwnedGroup { range: start..end, bytes }));
                }
                _ => return Err(ArchiveError::new(ArchiveErrorKind::InvalidGroupLayout)),
            }
        }

        // Group 0 is the full match and must always be present.
        let full = groups[0]
            .as_ref()
            .ok_or_else(|| ArchiveError::new(ArchiveErrorKind::InvalidGroupLayout))?;
        // Every group range must lie within the full match range.
        for group in groups.iter().flatten() {
            if group.range.start < full.range.start || group.range.end > full.range.end {
                return Err(ArchiveError::new(ArchiveErrorKind::InvalidRange));
            }
        }

        let mut names = Vec::with_capacity(name_count);
        for _ in 0..name_count {
            let name_len = to_usize(cursor.u64()?, ArchiveErrorKind::InvalidName)?;
            let name_bytes = cursor.take(name_len)?;
            let name = core::str::from_utf8(name_bytes)
                .map_err(|_| ArchiveError::new(ArchiveErrorKind::InvalidName))?;
            if name.is_empty() {
                return Err(ArchiveError::new(ArchiveErrorKind::InvalidName));
            }
            let index = to_usize(cursor.u64()?, ArchiveErrorKind::InvalidNameIndex)?;
            if index == 0 || index >= group_count {
                return Err(ArchiveError::new(ArchiveErrorKind::InvalidNameIndex));
            }
            if names.iter().any(|(n, _): &(String, usize)| n == name) {
                return Err(ArchiveError::new(ArchiveErrorKind::DuplicateName));
            }
            if names.iter().any(|(_, i)| *i == index) {
                return Err(ArchiveError::new(ArchiveErrorKind::DuplicateName));
            }
            names.push((String::from(name), index));
        }

        if cursor.remaining() != 0 {
            return Err(ArchiveError::new(ArchiveErrorKind::TrailingBytes));
        }

        Ok(OwnedCaptures { groups, names })
    }
}

impl core::fmt::Debug for OwnedCaptures {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        use regex_automata::util::escape::DebugHaystack;

        struct CapturesDebugMap<'a>(&'a OwnedCaptures);

        impl<'a> core::fmt::Debug for CapturesDebugMap<'a> {
            fn fmt(&self, f: &mut core::fmt::Formatter) -> core::fmt::Result {
                let mut map = f.debug_map();
                for (i, group) in self.0.groups.iter().enumerate() {
                    match group {
                        None => map.entry(&i, &None::<()>),
                        Some(group) => map.entry(
                            &(i, group.range.start..group.range.end),
                            &DebugHaystack(&group.bytes),
                        ),
                    };
                }
                map.finish()
            }
        }

        f.debug_tuple("OwnedCaptures")
            .field(&CapturesDebugMap(self))
            .finish()
    }
}

/// A single match from an [`OwnedCaptures`].
///
/// This is the owned-snapshot counterpart of
/// [`Match`](crate::bytes::Match). It provides access to the original byte
/// range of the match in the haystack and to the matched bytes themselves.
/// The lifetime parameter `'a` refers to the lifetime of the
/// [`OwnedCaptures`] that produced this match.
#[derive(Copy, Clone, Eq, PartialEq)]
pub struct OwnedMatch<'a> {
    start: usize,
    end: usize,
    bytes: &'a [u8],
}

impl<'a> OwnedMatch<'a> {
    /// Returns the byte offset of the start of the match in the haystack.
    ///
    /// It is guaranteed that `OwnedMatch::start() <= OwnedMatch::end()`.
    #[inline]
    pub fn start(&self) -> usize {
        self.start
    }

    /// Returns the byte offset of the end of the match in the haystack.
    ///
    /// It is guaranteed that `OwnedMatch::start() <= OwnedMatch::end()`.
    #[inline]
    pub fn end(&self) -> usize {
        self.end
    }

    /// Returns true if and only if this match has a length of zero.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.start == self.end
    }

    /// Returns the length, in bytes, of this match.
    #[inline]
    pub fn len(&self) -> usize {
        self.bytes.len()
    }

    /// Returns the range over the starting and ending byte offsets of the
    /// match in the haystack.
    #[inline]
    pub fn range(&self) -> Range<usize> {
        self.start..self.end
    }

    /// Returns the bytes that matched. These are preserved byte-for-byte even
    /// when they are not valid UTF-8.
    #[inline]
    pub fn as_bytes(&self) -> &'a [u8] {
        self.bytes
    }
}

impl<'a> core::fmt::Debug for OwnedMatch<'a> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        use regex_automata::util::escape::DebugHaystack;

        let mut fmt = f.debug_struct("OwnedMatch");
        fmt.field("start", &self.start)
            .field("end", &self.end)
            .field("bytes", &DebugHaystack(self.bytes));
        fmt.finish()
    }
}

/// An iterator over all group matches in an [`OwnedCaptures`] value.
///
/// This iterator is created by [`OwnedCaptures::iter`] and yields values of
/// type `Option<OwnedMatch<'c>>`, where `'c` is the lifetime of the
/// `OwnedCaptures` that created the iterator. A `None` value is yielded for
/// groups that did not participate in the match.
#[derive(Clone, Debug)]
pub struct OwnedCaptureMatches<'c> {
    caps: &'c OwnedCaptures,
    indices: Range<usize>,
}

impl<'c> Iterator for OwnedCaptureMatches<'c> {
    type Item = Option<OwnedMatch<'c>>;

    #[inline]
    fn next(&mut self) -> Option<Option<OwnedMatch<'c>>> {
        let i = self.indices.next()?;
        Some(self.caps.get(i))
    }

    #[inline]
    fn size_hint(&self) -> (usize, Option<usize>) {
        self.indices.size_hint()
    }
}

impl<'c> ExactSizeIterator for OwnedCaptureMatches<'c> {}

impl<'c> core::iter::FusedIterator for OwnedCaptureMatches<'c> {}

impl core::ops::Index<usize> for OwnedCaptures {
    type Output = [u8];

    fn index(&self, i: usize) -> &[u8] {
        self.get(i)
            .map(|m| m.as_bytes())
            .unwrap_or_else(|| panic!("no group at index '{i}'"))
    }
}

impl core::ops::Index<&str> for OwnedCaptures {
    type Output = [u8];

    fn index(&self, name: &str) -> &[u8] {
        self.name(name)
            .map(|m| m.as_bytes())
            .unwrap_or_else(|| panic!("no group named '{name}'"))
    }
}

impl<'h> Captures<'h> {
    /// Creates an owned snapshot of these captures.
    ///
    /// The returned [`OwnedCaptures`] stores a copy of every matched byte
    /// (including non-UTF-8 bytes), the original byte range of each capture
    /// group and the named group index. It does not borrow the haystack, so it
    /// remains readable after the haystack buffer is reused or dropped.
    ///
    /// This is useful when captures must outlive the buffer they were matched
    /// in, for example when reading into a reused buffer:
    ///
    /// ```
    /// use regex::bytes::Regex;
    ///
    /// let re = Regex::new(r"(?<v>\d+)").unwrap();
    /// let mut buf = Vec::new();
    ///
    /// buf.extend_from_slice(b"item 123");
    /// let first = re.captures(&buf).unwrap().into_owned();
    ///
    /// buf.clear();
    /// buf.extend_from_slice(b"item 456"); // reuse the buffer
    /// let second = re.captures(&buf).unwrap().into_owned();
    ///
    /// // Both snapshots remain readable.
    /// assert_eq!(&first["v"], b"123");
    /// assert_eq!(&second["v"], b"456");
    /// ```
    #[inline]
    pub fn into_owned(self) -> OwnedCaptures {
        OwnedCaptures::from_captures(&self)
    }
}

/// An error returned when restoring a [`OwnedCaptures`] from an archive fails.
///
/// Restoration rejects malformed, truncated and tampered archives.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ArchiveError {
    kind: ArchiveErrorKind,
}

impl ArchiveError {
    fn new(kind: ArchiveErrorKind) -> ArchiveError {
        ArchiveError { kind }
    }
}

impl core::fmt::Display for ArchiveError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self.kind {
            ArchiveErrorKind::Truncated => {
                write!(f, "captures archive is truncated")
            }
            ArchiveErrorKind::BadMagic => {
                write!(f, "captures archive has invalid magic bytes")
            }
            ArchiveErrorKind::UnsupportedVersion(version) => {
                write!(
                    f,
                    "captures archive version {version} is not supported by this build \
                     (supported version is {ARCHIVE_VERSION})"
                )
            }
            ArchiveErrorKind::BadFlags(flags) => {
                write!(f, "captures archive has unsupported flags: {flags:#06x}")
            }
            ArchiveErrorKind::InvalidGroupCount => {
                write!(f, "captures archive has an invalid group count")
            }
            ArchiveErrorKind::InvalidGroupLayout => {
                write!(f, "captures archive has an invalid group layout")
            }
            ArchiveErrorKind::InvalidRange => {
                write!(f, "captures archive contains an invalid group range")
            }
            ArchiveErrorKind::InvalidName => {
                write!(f, "captures archive contains an invalid capture group name")
            }
            ArchiveErrorKind::InvalidNameIndex => {
                write!(
                    f,
                    "captures archive refers to a capture group index that does not exist"
                )
            }
            ArchiveErrorKind::DuplicateName => {
                write!(f, "captures archive contains a duplicate capture group name")
            }
            ArchiveErrorKind::BytesLengthMismatch => {
                write!(
                    f,
                    "captures archive saved bytes do not match the saved group ranges"
                )
            }
            ArchiveErrorKind::TrailingBytes => {
                write!(f, "captures archive has trailing bytes")
            }
        }
    }
}

#[cfg(feature = "std")]
impl std::error::Error for ArchiveError {}

/// The kind of an [`ArchiveError`].
#[derive(Clone, Debug, PartialEq, Eq)]
enum ArchiveErrorKind {
    /// The archive is shorter than its header or a record claims more bytes
    /// than remain.
    Truncated,
    /// The magic bytes do not match [`ARCHIVE_MAGIC`].
    BadMagic,
    /// The archive version is not [`ARCHIVE_VERSION`].
    UnsupportedVersion(u16),
    /// The reserved flags field is not [`ARCHIVE_FLAGS`].
    BadFlags(u16),
    /// The group count is zero, too large to fit in a `usize` or larger than
    /// the archive itself.
    InvalidGroupCount,
    /// The group layout is malformed (for example, group `0` is missing).
    InvalidGroupLayout,
    /// A group range is invalid (`start > end`, out of bounds or not contained
    /// in the full match).
    InvalidRange,
    /// A capture group name is empty, not valid UTF-8 or otherwise malformed.
    InvalidName,
    /// A name refers to an invalid (zero or out of bounds) group index.
    InvalidNameIndex,
    /// A capture group name or index is duplicated.
    DuplicateName,
    /// The saved byte length of a group does not match its saved range.
    BytesLengthMismatch,
    /// The archive has bytes left over after all records were read.
    TrailingBytes,
}

/// A minimal little-endian byte cursor over an archive.
struct Cursor<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Cursor<'a> {
    /// Returns the number of bytes not yet consumed.
    fn remaining(&self) -> usize {
        self.buf.len() - self.pos
    }

    /// Consumes exactly `n` bytes, returning them.
    fn take(&mut self, n: usize) -> Result<&'a [u8], ArchiveError> {
        if n > self.remaining() {
            return Err(ArchiveError::new(ArchiveErrorKind::Truncated));
        }
        let start = self.pos;
        self.pos += n;
        Ok(&self.buf[start..self.pos])
    }

    /// Consumes a single byte.
    fn u8(&mut self) -> Result<u8, ArchiveError> {
        Ok(self.take(1)?[0])
    }

    /// Consumes a little-endian `u16`.
    fn u16(&mut self) -> Result<u16, ArchiveError> {
        Ok(u16::from_le_bytes(self.take(2)?.try_into().unwrap()))
    }

    /// Consumes a little-endian `u64`.
    fn u64(&mut self) -> Result<u64, ArchiveError> {
        Ok(u64::from_le_bytes(self.take(8)?.try_into().unwrap()))
    }
}

/// Converts a `u64` archive field to a `usize`, mapping overflow to the given
/// error kind.
fn to_usize(
    value: u64,
    err: ArchiveErrorKind,
) -> Result<usize, ArchiveError> {
    usize::try_from(value).map_err(|_| ArchiveError::new(err))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bytes::Regex;

    #[test]
    fn owned_captures_prefixed_non_utf8_named_groups() {
        // Prefix two bytes before matching named groups on non-UTF-8 input.
        let mut buf = Vec::new();
        buf.extend_from_slice(b"\x00\x01");
        buf.extend_from_slice(b"name=foo\xFFbar\x02");

        let re = Regex::new(r"(?-u)(?<key>[a-z]+)=(?<val>[a-z]+\xFF[a-z]+)")
            .unwrap();
        let caps = re.captures(&buf).unwrap();
        let owned = caps.into_owned();

        // Print the original ranges and byte values.
        for (i, group) in owned.iter().enumerate() {
            if let Some(m) = group {
                std::println!(
                    "group {i}: range {:?} bytes {:?}",
                    m.range(),
                    m.as_bytes()
                );
            }
        }

        assert_eq!(owned.len(), 3);
        let key = owned.name("key").unwrap();
        let val = owned.name("val").unwrap();
        assert_eq!(key.range(), 2..6);
        assert_eq!(key.as_bytes(), b"name");
        assert_eq!(val.range(), 7..14);
        assert_eq!(val.as_bytes(), b"foo\xFFbar");

        // Name query and index query report the same range.
        assert_eq!(
            owned.name("val").unwrap().range(),
            owned.get(2).unwrap().range()
        );
        assert_eq!(
            owned.name("key").unwrap().range(),
            owned.get(1).unwrap().range()
        );

        // After the original buffer is released, the snapshot is still
        // readable.
        drop(buf);
        assert_eq!(owned.name("val").unwrap().as_bytes(), b"foo\xFFbar");
        assert_eq!(&owned[2], b"foo\xFFbar");
    }

    #[test]
    fn owned_captures_optional_missing_vs_empty_group() {
        let re = Regex::new(r"(?<missing>a)?(?<empty>b*)").unwrap();
        let owned = re.captures(b"x").unwrap().into_owned();

        // The optional group did not participate at all.
        assert!(owned.name("missing").is_none());
        assert!(owned.get(1).is_none());
        assert_eq!(owned.iter().nth(1).unwrap(), None);

        // The other group participated but matched the empty string.
        let m = owned.name("empty").unwrap();
        assert!(m.is_empty());
        assert_eq!(m.range(), 0..0);
        assert_eq!(m.as_bytes(), b"");

        // The iterator distinguishes "missing" from "matched empty".
        let mut it = owned.iter();
        assert!(it.next().unwrap().is_some()); // full match
        assert!(it.next().unwrap().is_none()); // missing group
        let empty = it.next().unwrap().unwrap();
        assert!(empty.is_empty());
        assert_eq!(empty.range(), 0..0);
        assert_eq!(it.next(), None);
    }

    #[test]
    fn owned_captures_archive_roundtrip_keeps_original_ranges() {
        let buf = b"\x00\x01name=value".to_vec();
        let re = Regex::new(r"(?<key>[a-z]+)=(?<val>[a-z]+)").unwrap();
        let owned = re.captures(&buf).unwrap().into_owned();

        let archive = owned.archive();
        let restored = OwnedCaptures::restore(&archive).unwrap();

        assert_eq!(restored, owned);
        // Ranges are the original ranges, not reset to start-at-zero slices.
        assert_eq!(restored.get(1).unwrap().range(), 2..6);
        assert_eq!(restored.get(2).unwrap().range(), 7..12);
        assert_eq!(restored.name("key").unwrap().range(), 2..6);
        assert_eq!(restored.name("val").unwrap().range(), 7..12);
        assert_eq!(restored.name("key").unwrap().as_bytes(), b"name");
        assert_eq!(restored.name("val").unwrap().as_bytes(), b"value");

        // Still readable after the buffer is released.
        drop(buf);
        assert_eq!(restored.name("val").unwrap().as_bytes(), b"value");
    }

    #[test]
    fn owned_captures_archive_rejects_tampering() {
        let re = Regex::new(r"(?<key>[a-z]+)=(?<val>[a-z]+)").unwrap();
        let owned = re.captures(b"name=value").unwrap().into_owned();
        let archive = owned.archive();

        // Bad magic.
        let mut tampered = archive.clone();
        tampered[0] ^= 0xFF;
        assert!(OwnedCaptures::restore(&tampered).is_err());

        // Bad version.
        let mut tampered = archive.clone();
        tampered[4] = 99;
        assert!(OwnedCaptures::restore(&tampered).is_err());

        // Bad flags.
        let mut tampered = archive.clone();
        tampered[6] = 1;
        assert!(OwnedCaptures::restore(&tampered).is_err());

        // Truncation at every prefix length is rejected.
        for len in 0..archive.len() {
            assert!(
                OwnedCaptures::restore(&archive[..len]).is_err(),
                "prefix of length {len} should be rejected"
            );
        }
        // Only the exact, complete archive is accepted.
        assert!(OwnedCaptures::restore(&archive).is_ok());

        // Trailing bytes are rejected.
        let mut tampered = archive.clone();
        tampered.push(0);
        assert!(OwnedCaptures::restore(&tampered).is_err());

        // A tampered group range is rejected. Group 0 is the full match; its
        // record is 1 (present) + 8 (start) + 8 (end) + 8 (len) + match bytes.
        let g0_record = 1 + 8 + 8 + 8 + owned.get_match().len();
        let g1_start = ARCHIVE_HEADER_LEN + g0_record + 1; // +1 for present
        let mut tampered = archive.clone();
        tampered[g1_start] ^= 0x01;
        assert!(OwnedCaptures::restore(&tampered).is_err());

        // A tampered group byte length is rejected.
        let g1_len = g1_start + 16; // after present + start + end
        let mut tampered = archive.clone();
        tampered[g1_len] ^= 0x01;
        assert!(OwnedCaptures::restore(&tampered).is_err());

        // A tampered name byte (invalid UTF-8) is rejected. The names section
        // follows the group records.
        let groups_end = ARCHIVE_HEADER_LEN
            + g0_record
            + 1
            + 8
            + 8
            + 8
            + owned.get(1).unwrap().len();
        let mut tampered = archive.clone();
        tampered[groups_end + 8] ^= 0xFF; // first byte of the first name
        assert!(OwnedCaptures::restore(&tampered).is_err());
    }

    #[test]
    fn owned_captures_count_and_boundaries() {
        let re = Regex::new(r"(\w)(\d)?(\w)").unwrap();
        let owned = re.captures(b"AZ").unwrap().into_owned();

        assert_eq!(owned.len(), 4);
        assert_eq!(owned.iter().count(), 4);
        assert_eq!(owned.iter().flatten().count(), 3);

        assert!(owned.get(0).is_some());
        assert!(owned.get(3).is_some());
        assert!(owned.get(4).is_none());
        assert!(owned.get(100).is_none());
        assert!(owned.name("nope").is_none());

        // A participating group that matched the empty string is distinct
        // from a missing group.
        let re = Regex::new(r"(?<e>x?)").unwrap();
        let owned = re.captures(b"zzz").unwrap().into_owned();
        let m = owned.name("e").unwrap();
        assert!(m.is_empty());
        assert_eq!(m.range(), 0..0);
        assert_eq!(m.len(), 0);
        assert_eq!(m.as_bytes(), b"");
    }

    #[test]
    fn owned_captures_reused_buffer_keeps_both_snapshots() {
        let re = Regex::new(r"(?<v>\d+)").unwrap();
        let mut buf = Vec::new();

        buf.extend_from_slice(b"item 123");
        let first = re.captures(&buf).unwrap().into_owned();

        buf.clear();
        buf.extend_from_slice(b"item 456");
        let second = re.captures(&buf).unwrap().into_owned();

        assert_eq!(&first["v"], b"123");
        assert_eq!(&second["v"], b"456");
        assert_eq!(first.name("v").unwrap().range(), 5..8);
        assert_eq!(second.name("v").unwrap().range(), 5..8);
    }

    #[test]
    fn owned_captures_debug_includes_ranges_and_bytes() {
        let re = Regex::new(r"(?<v>\d+)").unwrap();
        let owned = re.captures(b"abc\xFF123").unwrap().into_owned();
        let debug = alloc::format!("{owned:?}");
        assert!(debug.contains("4..7"), "debug was: {debug}");
        assert!(debug.contains("123"), "debug was: {debug}");
        // Non-UTF-8 bytes are escaped rather than causing a panic.
        let _ = alloc::format!("{:?}", owned.get_match());
    }
}
