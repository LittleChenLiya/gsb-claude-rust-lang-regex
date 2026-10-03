/*!
Tests for `regex::bytes::CapturesSnapshot`, the explicitly owned snapshot of
`regex::bytes::Captures`.

Covered here:

* taking a snapshot from a reusable haystack buffer (with a two byte prefix
  and non-UTF-8 named groups) and reading it after the buffer is gone,
* distinguishing a missing optional group from a group that matched the
  empty string,
* consistency between name based and index based queries,
* versioned archive round trips that preserve original (non zero based)
  ranges without re-executing any regex,
* rejection of tampered or otherwise inconsistent archives,
* group count and boundary cases.

Run only these tests with:

    cargo test --test integration snapshot::
*/

use regex::bytes::{CapturesSnapshot, Regex, SnapshotError};

/// Computes the FNV-1a 64-bit hash of `data`.
///
/// This mirrors the (deliberately simple, non-cryptographic) checksum used
/// by the snapshot archive format, so that tests can both detect and repair
/// checksums when crafting tampered archives.
fn fnv1a64(data: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf29ce484222325;
    for &b in data {
        hash ^= u64::from(b);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

/// A builder for hand crafted snapshot archives.
///
/// This exists so tests can construct archives that are structurally invalid
/// in specific ways (but still carry a valid checksum), which is something
/// `CapturesSnapshot::to_archive` can never produce.
struct ArchiveBuilder {
    body: Vec<u8>,
}

impl ArchiveBuilder {
    /// Starts an archive with the standard header: magic, version 1, the
    /// given group count and the given full match bytes.
    fn new(group_count: u32, full: &[u8]) -> ArchiveBuilder {
        let mut body = Vec::new();
        body.extend_from_slice(b"RXSNAP");
        body.extend_from_slice(&1u32.to_le_bytes());
        body.extend_from_slice(&group_count.to_le_bytes());
        body.extend_from_slice(&(full.len() as u32).to_le_bytes());
        body.extend_from_slice(full);
        ArchiveBuilder { body }
    }

    /// Appends a non-participating group entry.
    fn group_none(&mut self) -> &mut ArchiveBuilder {
        self.body.push(0);
        self
    }

    /// Appends a participating group entry with the given original range
    /// and bytes.
    fn group(
        &mut self,
        start: u64,
        end: u64,
        bytes: &[u8],
    ) -> &mut ArchiveBuilder {
        self.body.push(1);
        self.body.extend_from_slice(&start.to_le_bytes());
        self.body.extend_from_slice(&end.to_le_bytes());
        self.body.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
        self.body.extend_from_slice(bytes);
        self
    }

    /// Appends the name section and the trailing checksum, returning the
    /// complete archive.
    fn finish(&self, names: &[(u32, &[u8])]) -> Vec<u8> {
        let mut body = self.body.clone();
        body.extend_from_slice(&(names.len() as u32).to_le_bytes());
        for &(index, name) in names.iter() {
            body.extend_from_slice(&index.to_le_bytes());
            body.extend_from_slice(&(name.len() as u32).to_le_bytes());
            body.extend_from_slice(name);
        }
        let checksum = fnv1a64(&body);
        body.extend_from_slice(&checksum.to_le_bytes());
        body
    }
}

/// Returns a snapshot of a match on non-UTF-8 input with a two byte prefix,
/// taken in a scope so that both the haystack and the `Captures` are dropped
/// before the snapshot is returned.
fn prefixed_non_utf8_snapshot() -> CapturesSnapshot {
    let re =
        Regex::new(r"(?-u)(?<head>[a-z]\xFF+[a-z])(?<num>[0-9]+)").unwrap();
    let snap = {
        // Two byte prefix, then a match containing the non-UTF-8 byte 0xFF.
        let haystack = b"\xFF\xFEa\xFFb123tail".to_vec();
        let caps = re.captures(&haystack).unwrap();
        caps.snapshot()
    }; // `haystack` (the reusable buffer) and `caps` are dropped here.
    snap
}

#[test]
fn non_utf8_named_groups_survive_buffer_drop() {
    let snap = prefixed_non_utf8_snapshot();

    // The original buffer is gone, but the snapshot is still readable and
    // still reports the original (non zero based) ranges and raw bytes.
    println!(
        "full match: range {:?} bytes {:?}",
        snap.full_match_span(),
        snap.full_match_bytes()
    );
    for i in 0..snap.group_count() {
        println!(
            "group {i} (name {:?}): range {:?} bytes {:?}",
            snap.name(i),
            snap.span(i),
            snap.bytes(i),
        );
    }

    assert_eq!(snap.group_count(), 3);
    assert_eq!(snap.full_match_span(), 2..8);
    assert_eq!(snap.full_match_bytes(), &b"a\xFFb123"[..]);
    assert_eq!(snap.span(1), Some(2..5));
    assert_eq!(snap.bytes(1), Some(&b"a\xFFb"[..]));
    assert_eq!(snap.span(2), Some(5..8));
    assert_eq!(snap.bytes(2), Some(&b"123"[..]));
    // Non-UTF-8 bytes are preserved as bytes, never validated as UTF-8.
    assert!(core::str::from_utf8(snap.bytes(1).unwrap()).is_err());
}

#[test]
fn optional_missing_group_differs_from_empty_group() {
    let re = Regex::new(r"(?-u)x(?<opt>z)?(?<empty>)y").unwrap();
    let caps = re.captures(b"xy").unwrap();
    let snap = caps.snapshot();

    assert_eq!(snap.group_count(), 3);
    // The optional group did not participate: no range, no bytes.
    assert_eq!(snap.span_by_name("opt"), None);
    assert_eq!(snap.bytes_by_name("opt"), None);
    // The empty group participated but matched the empty string: an empty
    // range and empty (but present) bytes. This is distinct from `None`.
    assert_eq!(snap.span_by_name("empty"), Some(1..1));
    assert_eq!(snap.bytes_by_name("empty"), Some(&b""[..]));
    assert_ne!(snap.span_by_name("opt"), snap.span_by_name("empty"));
}

#[test]
fn name_and_index_queries_agree() {
    let snap = prefixed_non_utf8_snapshot();
    for i in 0..snap.group_count() {
        let Some(name) = snap.name(i) else { continue };
        // The name based query must return exactly the same range and the
        // same bytes as the index based query for the same group.
        assert_eq!(snap.name_index(name), Some(i));
        assert_eq!(snap.span_by_name(name), snap.span(i));
        assert_eq!(snap.bytes_by_name(name), snap.bytes(i));
    }
    // Unknown names and out-of-bounds indices agree on `None` too.
    assert_eq!(snap.name_index("nope"), None);
    assert_eq!(snap.span_by_name("nope"), None);
    assert_eq!(snap.bytes_by_name("nope"), None);
    assert_eq!(snap.span(snap.group_count()), None);
    assert_eq!(snap.bytes(snap.group_count()), None);
}

#[test]
fn archive_roundtrip_preserves_original_ranges() {
    let snap = prefixed_non_utf8_snapshot();
    let archive = snap.to_archive();
    let restored = CapturesSnapshot::from_archive(&archive).unwrap();

    assert_eq!(restored, snap);
    // Ranges are restored exactly as archived: they refer to the original
    // input and are never reset to a 0 based slice of the saved bytes.
    assert_eq!(restored.full_match_span(), 2..8);
    assert_eq!(restored.span_by_name("head"), Some(2..5));
    assert_eq!(restored.span_by_name("num"), Some(5..8));
    assert_eq!(restored.bytes_by_name("head"), Some(&b"a\xFFb"[..]));
}

#[test]
fn archive_format_is_pinned() {
    // A hand built archive for a known snapshot must be byte-for-byte
    // identical to what `to_archive` produces. This pins the archive
    // format (and validates the test's own archive builder).
    let snap = prefixed_non_utf8_snapshot();
    let mut b = ArchiveBuilder::new(3, b"a\xFFb123");
    b.group(2, 8, b"a\xFFb123").group(2, 5, b"a\xFFb").group(5, 8, b"123");
    let expected = b.finish(&[(1, b"head"), (2, b"num")]);
    assert_eq!(snap.to_archive(), expected);
}

#[test]
fn archive_restores_without_rerunning_regex() {
    // Restore from an archive produced in a completely separate scope. No
    // `Regex` value exists at this point, so restoration cannot possibly
    // re-execute one.
    let archive = prefixed_non_utf8_snapshot().to_archive();
    let restored = CapturesSnapshot::from_archive(&archive).unwrap();
    assert_eq!(restored.group_count(), 3);
    assert_eq!(restored.name(1), Some("head"));
    assert_eq!(restored.name(2), Some("num"));
    assert_eq!(restored.name(0), None);
    assert_eq!(restored.full_match_bytes(), &b"a\xFFb123"[..]);
}

#[test]
fn group_count_matches_captures_len() {
    let re = Regex::new(r"(?-u)(a)(?<b>b)?(c)(?<d>d)?").unwrap();
    let caps = re.captures(b"xxacyy").unwrap();
    let snap = caps.snapshot();
    assert_eq!(snap.group_count(), caps.len());
    assert_eq!(snap.group_count(), 5);
    assert_eq!(snap.span(2), None);
    assert_eq!(snap.span(4), None);
    assert_eq!(snap.span(1), Some(2..3));
    assert_eq!(snap.span(3), Some(3..4));
}

#[test]
fn empty_overall_match_roundtrips() {
    // An empty overall match is a boundary case: group 0 has an empty
    // range and empty bytes, yet it participated.
    let re = Regex::new(r"(?-u)(?<e>a*)").unwrap();
    let caps = re.captures(b"bbb").unwrap();
    let snap = caps.snapshot();
    assert_eq!(snap.full_match_span(), 0..0);
    assert_eq!(snap.full_match_bytes(), &b""[..]);
    assert_eq!(snap.span_by_name("e"), Some(0..0));
    assert_eq!(snap.bytes_by_name("e"), Some(&b""[..]));

    let restored = CapturesSnapshot::from_archive(&snap.to_archive()).unwrap();
    assert_eq!(restored, snap);
    assert_eq!(restored.full_match_span(), 0..0);
}

#[test]
fn group_at_end_of_input_roundtrips() {
    let re = Regex::new(r"(?-u)(?<tail>[0-9]+)$").unwrap();
    let haystack = b"ab123";
    let caps = re.captures(haystack).unwrap();
    let snap = caps.snapshot();
    // The group ends exactly at the end of the input.
    assert_eq!(snap.span_by_name("tail"), Some(2..haystack.len()));
    let restored = CapturesSnapshot::from_archive(&snap.to_archive()).unwrap();
    assert_eq!(restored.span_by_name("tail"), Some(2..5));
    assert_eq!(restored.bytes_by_name("tail"), Some(&b"123"[..]));
}

#[test]
fn archive_rejects_bad_magic() {
    let snap = prefixed_non_utf8_snapshot();
    let mut archive = snap.to_archive();
    // Flip a magic byte and repair the checksum so only the magic check
    // can fire.
    archive[0] ^= 0xFF;
    let checksum = fnv1a64(&archive[..archive.len() - 8]);
    let n = archive.len();
    archive[n - 8..].copy_from_slice(&checksum.to_le_bytes());
    assert_eq!(
        CapturesSnapshot::from_archive(&archive),
        Err(SnapshotError::BadMagic),
    );
}

#[test]
fn archive_rejects_unsupported_version() {
    let snap = prefixed_non_utf8_snapshot();
    let mut archive = snap.to_archive();
    archive[6..10].copy_from_slice(&2u32.to_le_bytes());
    let checksum = fnv1a64(&archive[..archive.len() - 8]);
    let n = archive.len();
    archive[n - 8..].copy_from_slice(&checksum.to_le_bytes());
    assert_eq!(
        CapturesSnapshot::from_archive(&archive),
        Err(SnapshotError::UnsupportedVersion(2)),
    );
}

#[test]
fn archive_rejects_truncated() {
    let snap = prefixed_non_utf8_snapshot();
    let archive = snap.to_archive();
    // Every strict prefix of a valid archive must be rejected.
    for len in 0..archive.len() {
        assert!(
            CapturesSnapshot::from_archive(&archive[..len]).is_err(),
            "prefix of len {len} should be rejected",
        );
    }
}

#[test]
fn archive_rejects_trailing_bytes() {
    let snap = prefixed_non_utf8_snapshot();
    let mut archive = snap.to_archive();
    archive.push(0);
    assert_eq!(
        CapturesSnapshot::from_archive(&archive),
        Err(SnapshotError::ChecksumMismatch),
    );
}

#[test]
fn archive_rejects_tampered_saved_bytes() {
    let snap = prefixed_non_utf8_snapshot();
    // Flip one bit in every position of a valid archive. Every single
    // tampered archive must be rejected, no matter where the flip lands.
    let archive = snap.to_archive();
    for i in 0..archive.len() {
        let mut tampered = archive.clone();
        tampered[i] ^= 0x01;
        assert!(
            CapturesSnapshot::from_archive(&tampered).is_err(),
            "archive with bit flipped at offset {i} should be rejected",
        );
    }
}

#[test]
fn archive_rejects_overall_bytes_mismatch() {
    // The overall match group's saved bytes differ from the archived full
    // match bytes (with a valid checksum, so the structural check fires).
    let mut b = ArchiveBuilder::new(2, b"abc");
    b.group(2, 5, b"abd").group(2, 3, b"a");
    let archive = b.finish(&[]);
    assert!(matches!(
        CapturesSnapshot::from_archive(&archive),
        Err(SnapshotError::Range(_)),
    ));
}

#[test]
fn archive_rejects_range_length_mismatch() {
    // A group's original range length does not match its saved byte count.
    let mut b = ArchiveBuilder::new(2, b"abc");
    b.group(2, 5, b"abc").group(2, 4, b"a");
    let archive = b.finish(&[]);
    assert!(matches!(
        CapturesSnapshot::from_archive(&archive),
        Err(SnapshotError::Range(_)),
    ));
}

#[test]
fn archive_rejects_inverted_range() {
    let mut b = ArchiveBuilder::new(2, b"abc");
    b.group(2, 5, b"abc").group(4, 3, b"");
    let archive = b.finish(&[]);
    assert!(matches!(
        CapturesSnapshot::from_archive(&archive),
        Err(SnapshotError::Range(_)),
    ));
}

#[test]
fn archive_rejects_group_outside_overall_match() {
    // A group range that lies outside the overall match range. In
    // particular, ranges are validated against the archived overall match
    // range, not silently accepted or re-based.
    let mut b = ArchiveBuilder::new(2, b"abc");
    b.group(2, 5, b"abc").group(0, 3, b"abc");
    let archive = b.finish(&[]);
    assert!(matches!(
        CapturesSnapshot::from_archive(&archive),
        Err(SnapshotError::Range(_)),
    ));
}

#[test]
fn archive_rejects_zero_groups() {
    let b = ArchiveBuilder::new(0, b"");
    let archive = b.finish(&[]);
    assert!(matches!(
        CapturesSnapshot::from_archive(&archive),
        Err(SnapshotError::Layout(_)),
    ));
}

#[test]
fn archive_rejects_missing_overall_group() {
    let mut b = ArchiveBuilder::new(2, b"abc");
    b.group_none().group(2, 3, b"a");
    let archive = b.finish(&[]);
    assert!(matches!(
        CapturesSnapshot::from_archive(&archive),
        Err(SnapshotError::Layout(_)),
    ));
}

#[test]
fn archive_rejects_unknown_group_tag() {
    let snap = prefixed_non_utf8_snapshot();
    let archive = snap.to_archive();
    // The first group tag byte sits right after the header:
    // 6 magic + 4 version + 4 group count + 4 full len + 6 full bytes.
    let tag_offset = 6 + 4 + 4 + 4 + 6;
    assert_eq!(archive[tag_offset], 1);
    let mut tampered = archive;
    tampered[tag_offset] = 7;
    let checksum = fnv1a64(&tampered[..tampered.len() - 8]);
    let n = tampered.len();
    tampered[n - 8..].copy_from_slice(&checksum.to_le_bytes());
    assert!(matches!(
        CapturesSnapshot::from_archive(&tampered),
        Err(SnapshotError::Layout(_)),
    ));
}

#[test]
fn archive_rejects_invalid_names() {
    // Name assigned to the overall match group (index 0).
    let mut b = ArchiveBuilder::new(2, b"abc");
    b.group(2, 5, b"abc").group(2, 3, b"a");
    let archive = b.finish(&[(0, b"x")]);
    assert!(matches!(
        CapturesSnapshot::from_archive(&archive),
        Err(SnapshotError::Layout(_)),
    ));

    // Name assigned to an out-of-bounds group index.
    let mut b = ArchiveBuilder::new(2, b"abc");
    b.group(2, 5, b"abc").group(2, 3, b"a");
    let archive = b.finish(&[(2, b"x")]);
    assert!(matches!(
        CapturesSnapshot::from_archive(&archive),
        Err(SnapshotError::Layout(_)),
    ));

    // The same group assigned two names.
    let mut b = ArchiveBuilder::new(2, b"abc");
    b.group(2, 5, b"abc").group(2, 3, b"a");
    let archive = b.finish(&[(1, b"x"), (1, b"y")]);
    assert!(matches!(
        CapturesSnapshot::from_archive(&archive),
        Err(SnapshotError::Layout(_)),
    ));

    // An empty name.
    let mut b = ArchiveBuilder::new(2, b"abc");
    b.group(2, 5, b"abc").group(2, 3, b"a");
    let archive = b.finish(&[(1, b"")]);
    assert!(matches!(
        CapturesSnapshot::from_archive(&archive),
        Err(SnapshotError::InvalidName(_)),
    ));

    // A name that is not valid UTF-8.
    let mut b = ArchiveBuilder::new(2, b"abc");
    b.group(2, 5, b"abc").group(2, 3, b"a");
    let archive = b.finish(&[(1, b"\xFF")]);
    assert!(matches!(
        CapturesSnapshot::from_archive(&archive),
        Err(SnapshotError::InvalidName(_)),
    ));
}

#[test]
fn archive_rejects_absurd_counts_without_hanging() {
    // A corrupt group count must be rejected quickly instead of causing a
    // huge allocation or a long loop.
    let mut body = Vec::new();
    body.extend_from_slice(b"RXSNAP");
    body.extend_from_slice(&1u32.to_le_bytes());
    body.extend_from_slice(&u32::MAX.to_le_bytes());
    let checksum = fnv1a64(&body);
    body.extend_from_slice(&checksum.to_le_bytes());
    assert!(CapturesSnapshot::from_archive(&body).is_err());
}

#[test]
fn snapshot_error_display_is_readable() {
    let err = SnapshotError::UnsupportedVersion(99);
    let msg = err.to_string();
    assert!(msg.contains("99"), "error message: {msg:?}");
    assert!(msg.contains('1'), "error message: {msg:?}");
}
