//! Streaming regex search over arbitrarily chunked UTF-8 input.
//!
//! This module provides [`StreamingFinder`], a stateful finder that accepts a
//! haystack as a sequence of byte chunks (via [`StreamingFinder::feed`]) and
//! yields the same sequence of non-overlapping matches that
//! [`Regex::find_iter`] would produce if the complete text were available all
//! at once. Matches are reported with byte offsets that are global to the
//! entire stream, not to any individual chunk.
//!
//! Because a match may span a chunk boundary (and because greedy matches may
//! grow when more input arrives), a match is only ever reported once it is
//! provably final. To make that possible, [`StreamingFinder::new`] computes
//! the maximum match width of the pattern and rejects patterns whose match
//! width cannot be bounded (for example, any pattern using unbounded
//! repetition such as `a*` or `a{2,}`). The amount of memory retained
//! internally is bounded by that maximum match width plus a small constant
//! amount of boundary context; the finder never accumulates the full history
//! of the input.

use alloc::string::String;
use alloc::vec::Vec;
use core::fmt;

use crate::Regex;

/// A single match produced by a [`StreamingFinder`].
///
/// The `start` and `end` offsets are byte offsets relative to the
/// beginning of the entire stream (i.e. all bytes ever fed to the finder),
/// exactly as if [`Regex::find_iter`] had been run over the concatenation
/// of every chunk. The matched text is copied out and owned by this value.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StreamMatch {
    /// The global byte offset at which the match starts.
    pub start: u64,
    /// The global byte offset at which the match ends.
    pub end: u64,
    /// The matched text, owned by this value.
    pub text: String,
}

/// An error returned by [`StreamingFinder`].
#[derive(Debug)]
#[non_exhaustive]
pub enum StreamError {
    /// The pattern failed to compile.
    Regex(crate::Error),
    /// The pattern has no bounded maximum match length.
    ///
    /// This is reported at construction time for patterns using unbounded
    /// repetition such as `a*`, `a+` or `a{2,}` (and also for patterns
    /// that can never match anything). Streaming search requires a provable
    /// upper bound on the width of any match so that it can know when a
    /// match is final and bound its internal buffer.
    Unbounded,
    /// A chunk (or the end of the stream) contained invalid UTF-8.
    ///
    /// The finder is poisoned after this error: all subsequent calls fail
    /// with [`StreamError::Poisoned`].
    InvalidUtf8,
    /// `feed` was called after `finish`, or `finish` was called twice.
    ///
    /// The finder is poisoned after this error: all subsequent calls fail
    /// with [`StreamError::Poisoned`].
    AlreadyFinished,
    /// The finder is in a failed state because of a previous error and
    /// refuses to do any more work.
    Poisoned,
}

impl fmt::Display for StreamError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            StreamError::Regex(ref e) => {
                write!(f, "regex compile error: {}", e)
            }
            StreamError::Unbounded => write!(
                f,
                "pattern has no bounded maximum match length; \
                 unbounded repetition is not supported by streaming search"
            ),
            StreamError::InvalidUtf8 => {
                write!(f, "stream contained invalid UTF-8")
            }
            StreamError::AlreadyFinished => {
                write!(f, "stream was already finished")
            }
            StreamError::Poisoned => {
                write!(f, "streaming finder is in a failed state")
            }
        }
    }
}

impl std::error::Error for StreamError {}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum State {
    Active,
    Finished,
    Failed,
}

/// A streaming version of [`Regex::find_iter`] for chunked byte input.
///
/// A `StreamingFinder` is created from a pattern with
/// [`StreamingFinder::new`], which fails if the pattern's maximum match
/// width cannot be bounded. Bytes are supplied in arbitrary chunks with
/// [`StreamingFinder::feed`]; a UTF-8 encoded codepoint may be split
/// across chunk boundaries. Each call returns the matches that have become
/// provably final, in stream order, with global byte offsets. Calling
/// [`StreamingFinder::finish`] declares the end of the input and returns
/// any remaining matches.
///
/// Concatenating the matches returned by every `feed` call and the final
/// `finish` call yields exactly the sequence that `Regex::find_iter` would
/// produce on the complete text.
///
/// # Errors and poisoning
///
/// Feeding invalid UTF-8, feeding after `finish`, or calling `finish`
/// twice is an error. Once any error has been returned, the finder is
/// poisoned and every subsequent call returns [`StreamError::Poisoned`].
///
/// # Example
///
/// ```
/// use regex::StreamingFinder;
///
/// let mut finder = StreamingFinder::new(r"ab{1,2}").unwrap();
/// // A greedy match must not be reported until it is provably final.
/// assert!(finder.feed(b"a").unwrap().is_empty());
/// assert!(finder.feed(b"b").unwrap().is_empty());
/// assert!(finder.feed(b"b").unwrap().is_empty());
/// let m = finder.finish().unwrap();
/// assert_eq!(m.len(), 1);
/// assert_eq!((m[0].start, m[0].end), (0, 3));
/// assert_eq!(m[0].text, "abb");
/// ```
#[derive(Debug)]
pub struct StreamingFinder {
    re: Regex,
    /// Maximum width of any match, in bytes.
    max_len: usize,
    /// Uncommitted bytes. `buf[..valid]` is valid UTF-8; the remainder is
    /// an incomplete trailing codepoint (fewer than 4 bytes).
    buf: Vec<u8>,
    /// Length of the valid UTF-8 prefix of `buf`.
    valid: usize,
    /// Global stream offset of `buf[0]`.
    base: u64,
    /// Next search start position, as an offset within `buf`.
    pos: usize,
    /// End of the most recently reported match, as an offset within `buf`.
    /// Mirrors `last_match_end` in `regex_automata::util::iter::Searcher`.
    last_match_end: Option<usize>,
    state: State,
}

impl StreamingFinder {
    /// Creates a new streaming finder for the given pattern.
    ///
    /// Returns [`StreamError::Unbounded`] if the pattern's maximum match
    /// width cannot be bounded (e.g. it uses `*`, `+` or `{n,}`
    /// repetition), and [`StreamError::Regex`] if the pattern does not
    /// compile.
    pub fn new(pattern: &str) -> Result<StreamingFinder, StreamError> {
        let re = Regex::new(pattern).map_err(StreamError::Regex)?;
        let hir =
            match regex_syntax::ParserBuilder::new().build().parse(pattern) {
                Ok(hir) => hir,
                // The pattern compiled above, so this should be unreachable,
                // but stay total just in case the parsers ever disagree.
                Err(_) => {
                    return Err(StreamError::Regex(
                        Regex::new(pattern)
                            .expect_err("pattern compiled moments ago"),
                    ));
                }
            };
        let max_len =
            hir.properties().maximum_len().ok_or(StreamError::Unbounded)?;
        Ok(StreamingFinder {
            re,
            max_len,
            buf: Vec::new(),
            valid: 0,
            base: 0,
            pos: 0,
            last_match_end: None,
            state: State::Active,
        })
    }

    /// Returns the maximum width (in bytes) of any match of the pattern.
    pub fn max_match_len(&self) -> usize {
        self.max_len
    }

    /// Returns the number of bytes currently retained internally.
    ///
    /// This is bounded by the pattern's maximum match width plus a small
    /// amount of boundary context; it does not grow with the total amount
    /// of input consumed.
    pub fn buffered_len(&self) -> usize {
        self.buf.len()
    }

    /// Feeds a chunk of bytes to the finder and returns every match that
    /// became provably final as a result.
    ///
    /// Chunks may split a UTF-8 encoded codepoint across boundaries.
    /// Matches are returned in stream order with global byte offsets.
    pub fn feed(
        &mut self,
        chunk: &[u8],
    ) -> Result<Vec<StreamMatch>, StreamError> {
        match self.state {
            State::Active => {}
            State::Finished => {
                self.state = State::Failed;
                return Err(StreamError::AlreadyFinished);
            }
            State::Failed => return Err(StreamError::Poisoned),
        }
        self.buf.extend_from_slice(chunk);
        match core::str::from_utf8(&self.buf[self.valid..]) {
            Ok(_) => self.valid = self.buf.len(),
            Err(ref e) => {
                self.valid += e.valid_up_to();
                if e.error_len().is_some() {
                    self.state = State::Failed;
                    return Err(StreamError::InvalidUtf8);
                }
            }
        }
        Ok(self.advance(false))
    }

    /// Declares the end of the input and returns the remaining matches.
    ///
    /// If the stream ends in the middle of a UTF-8 encoded codepoint, this
    /// fails with [`StreamError::InvalidUtf8`]. Calling `finish` twice (or
    /// calling `feed` after `finish`) fails with
    /// [`StreamError::AlreadyFinished`].
    pub fn finish(&mut self) -> Result<Vec<StreamMatch>, StreamError> {
        match self.state {
            State::Active => {}
            State::Finished => {
                self.state = State::Failed;
                return Err(StreamError::AlreadyFinished);
            }
            State::Failed => return Err(StreamError::Poisoned),
        }
        if self.valid != self.buf.len() {
            self.state = State::Failed;
            return Err(StreamError::InvalidUtf8);
        }
        self.state = State::Finished;
        Ok(self.advance(true))
    }
}

/// Private implementation details.
impl StreamingFinder {
    /// Reports every match that is provably final given the buffered
    /// bytes. When `eof` is true, all remaining matches are final.
    ///
    /// This mirrors `regex_automata::util::iter::Searcher::try_advance`
    /// exactly, so the emitted sequence is identical to `find_iter`.
    fn advance(&mut self, eof: bool) -> Vec<StreamMatch> {
        let mut out = Vec::new();
        let hay = core::str::from_utf8(&self.buf[..self.valid])
            .expect("valid UTF-8");
        let p = hay.len();
        loop {
            if self.pos > p {
                break;
            }
            let (s, e) = match self.re.find_at(hay, self.pos) {
                None => {
                    advance_over_dead(hay, self.max_len, p, &mut self.pos);
                    break;
                }
                Some(m) => (m.start(), m.end()),
            };
            // A match starting at s can still change (grow greedily, or
            // appear or disappear via look-around or end anchors) unless
            // we hold more than s + max_len bytes. Only then is it final.
            if !eof && s.saturating_add(self.max_len) >= p {
                advance_over_dead(hay, self.max_len, s, &mut self.pos);
                break;
            }
            if s == e && Some(e) == self.last_match_end {
                // An empty match that overlaps the previous match is
                // skipped; the search resumes one byte later. The skip
                // is final because the empty match itself is final.
                self.pos += 1;
                if self.pos > p {
                    break;
                }
                let (s2, e2) = match self.re.find_at(hay, self.pos) {
                    None => {
                        advance_over_dead(hay, self.max_len, p, &mut self.pos);
                        break;
                    }
                    Some(m) => (m.start(), m.end()),
                };
                if !eof && s2.saturating_add(self.max_len) >= p {
                    advance_over_dead(hay, self.max_len, s2, &mut self.pos);
                    break;
                }
                out.push(StreamMatch {
                    start: self.base + s2 as u64,
                    end: self.base + e2 as u64,
                    text: String::from(&hay[s2..e2]),
                });
                self.pos = e2;
                self.last_match_end = Some(e2);
            } else {
                out.push(StreamMatch {
                    start: self.base + s as u64,
                    end: self.base + e as u64,
                    text: String::from(&hay[s..e]),
                });
                self.pos = e;
                self.last_match_end = Some(e);
            }
        }
        self.trim();
        out
    }

    /// Drops buffered bytes that can no longer influence any future
    /// match, keeping exactly one codepoint of look-behind context before
    /// the next search position.
    fn trim(&mut self) {
        if self.pos == 0 {
            return;
        }
        let keep = {
            let hay = core::str::from_utf8(&self.buf[..self.valid])
                .expect("valid UTF-8");
            floor_char_boundary(hay, (self.pos - 1).min(self.valid))
        };
        if keep == 0 {
            return;
        }
        self.buf.drain(..keep);
        self.valid -= keep;
        self.pos -= keep;
        self.base += keep as u64;
        // A `last_match_end` that falls off the buffer can never compare
        // equal to a future match (matches only move forward), so it is
        // equivalent to drop it.
        self.last_match_end =
            self.last_match_end.and_then(|e| e.checked_sub(keep));
    }
}

/// Returns `i` rounded down to a UTF-8 codepoint boundary of `s`.
fn floor_char_boundary(s: &str, i: usize) -> usize {
    if i >= s.len() {
        return s.len();
    }
    let mut i = i;
    while !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

/// Advances `*pos` over positions that are provably dead, given that no
/// match final for the current buffer starts in `*pos..limit`. A match
/// starting strictly before `hay.len() - max_len` would lie entirely
/// (including its look-around context) inside the buffer, and
/// end-of-haystack anchors cannot reach it either, so it would already
/// have been found. Positions in `*pos..limit` additionally did not
/// match on the current buffer, so any late match there would need an
/// end-of-haystack anchor, which is also ruled out before
/// `hay.len() - max_len`.
fn advance_over_dead(
    hay: &str,
    max_len: usize,
    limit: usize,
    pos: &mut usize,
) {
    let dead = hay.len().saturating_sub(max_len).min(limit);
    if dead > *pos {
        *pos = (*pos).max(floor_char_boundary(hay, dead));
    }
}
