/*!
Streaming regex search over chunked byte input.

This module provides [StreamFinder], a way to run a [Regex] over input
that arrives in arbitrary byte chunks (for example, from a network
socket or a pipe) instead of as one contiguous string. The matches
reported are guaranteed to be exactly the same as those returned by
[Regex::find_iter] on the complete UTF-8 text, including non-overlapping
semantics, greedy match lengths, empty matches, anchors and word
boundaries.

The pattern must have a provably bounded match width. Patterns with
unbounded repetition such as "a+", "a*" or "a{2,}" are rejected at
construction time with [StreamError::UnboundedPattern]. The memory used
by a [StreamFinder] is bounded by the pattern's maximum match width
plus the size of the chunks handed to [StreamFinder::feed]; the finder
never accumulates the full history of the input.
*/

use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::fmt;

use crate::regex::string::Regex;

/// A streaming regex finder over chunked UTF-8 byte input.
///
/// A finder accepts input via [StreamFinder::feed] in arbitrary byte
/// chunks. Multi-byte UTF-8 sequences may be split across chunks. Each
/// call returns the matches that are fully determined by the input seen
/// so far. A match is only reported once enough input exists to prove
/// that no more input could change it: a greedy match that could still
/// be extended, or a match whose end touches the end of the buffered
/// input, is held back. [StreamFinder::finish] declares the input
/// complete and yields all remaining matches.
///
/// The sequence of matches reported across all calls to feed and
/// finish is identical to what [Regex::find_iter] returns when given
/// the complete text at once. Offsets in each [StreamMatch] are
/// absolute byte offsets into the complete stream.
///
/// # Errors and state
///
/// * Feeding bytes that can never be valid UTF-8 (or finishing with an
///   incomplete UTF-8 sequence) returns [StreamError::InvalidUtf8].
/// * Calling feed after finish, or calling finish twice, returns
///   [StreamError::AlreadyFinished].
/// * Any error puts the finder into a poisoned state: every subsequent
///   call to feed or finish returns [StreamError::Poisoned].
///
/// # Example
///
/// ~~~
/// use regex::StreamFinder;
///
/// let mut finder = StreamFinder::new(r"ab{1,2}")?;
/// // The match is not reported until it is fully determined: after
/// // seeing "ab", more input could still extend it to "abb".
/// assert!(finder.feed(b"a")?.is_empty());
/// assert!(finder.feed(b"b")?.is_empty());
/// assert!(finder.feed(b"b")?.is_empty());
/// let matches = finder.finish()?;
/// assert_eq!(matches.len(), 1);
/// assert_eq!(matches[0].range(), 0..3);
/// assert_eq!(matches[0].as_str(), "abb");
/// # Ok::<(), regex::StreamError>(())
/// ~~~
#[derive(Debug)]
pub struct StreamFinder {
    /// The compiled regex, built with the default Regex configuration.
    re: Regex,
    /// Maximum match width in bytes. Proven finite at construction.
    max_width: usize,
    /// Unconsumed input. Starts with (at most) one UTF-8 encoded char
    /// of look-behind context immediately preceding the search
    /// position, followed by not-yet-consumed input. Never grows beyond
    /// roughly max_width + 8 + (last chunk size) bytes.
    buf: Vec<u8>,
    /// Absolute stream offset of buf[0].
    buf_start: u64,
    /// Length of the prefix of buf that is known to be valid UTF-8.
    /// buf[..valid_len] always starts on a char boundary.
    valid_len: usize,
    /// Absolute stream offset at which the next search starts. Never
    /// decreases. It is always >= buf_start, and if it is > 0 then buf
    /// contains the full char immediately preceding it.
    search_at: u64,
    /// Absolute end offset of the most recently reported match. Used
    /// to replicate the empty-match handling of find_iter.
    last_match_end: Option<u64>,
    /// Lifecycle state.
    state: State,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    Active,
    Finished,
    Poisoned,
}

/// A single match reported by a [StreamFinder].
///
/// The offsets are absolute byte offsets into the complete input
/// stream. The matched text is owned by this value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StreamMatch {
    start: u64,
    end: u64,
    text: String,
}

impl StreamMatch {
    /// Returns the absolute byte offset at which the match starts.
    #[inline]
    pub fn start(&self) -> u64 {
        self.start
    }

    /// Returns the absolute byte offset at which the match ends.
    #[inline]
    pub fn end(&self) -> u64 {
        self.end
    }

    /// Returns the absolute byte range of the match.
    #[inline]
    pub fn range(&self) -> core::ops::Range<u64> {
        self.start..self.end
    }

    /// Returns the matched text.
    #[inline]
    pub fn as_str(&self) -> &str {
        &self.text
    }
}

/// An error that can occur when constructing or driving a
/// [StreamFinder].
#[derive(Debug)]
pub enum StreamError {
    /// The pattern failed to compile.
    Compile(crate::Error),
    /// The pattern has no finite upper bound on its match width (for
    /// example, it uses unbounded repetition). Such patterns are not
    /// supported because they would require an unbounded buffer.
    UnboundedPattern,
    /// The fed bytes are not valid UTF-8 (or the input ended in the
    /// middle of a UTF-8 encoded char).
    InvalidUtf8,
    /// feed was called after finish, or finish was called twice.
    AlreadyFinished,
    /// A previous call returned an error. Once any error occurs, the
    /// finder refuses all further use.
    Poisoned,
}

impl fmt::Display for StreamError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            StreamError::Compile(ref err) => {
                write!(f, "error compiling regex for streaming: {}", err)
            }
            StreamError::UnboundedPattern => write!(
                f,
                "pattern has no bounded maximum match width \
                 (unbounded repetition is not supported)"
            ),
            StreamError::InvalidUtf8 => {
                write!(f, "stream is not valid UTF-8")
            }
            StreamError::AlreadyFinished => {
                write!(f, "stream finder was already finished")
            }
            StreamError::Poisoned => {
                write!(f, "stream finder is poisoned by a previous error")
            }
        }
    }
}

#[cfg(feature = "std")]
impl std::error::Error for StreamError {}

impl StreamFinder {
    /// Creates a new streaming finder for the given pattern.
    ///
    /// The pattern is compiled with the default [Regex] configuration
    /// and analyzed for its maximum match width. If no finite bound on
    /// the width of a match can be proven (for example because of
    /// unbounded repetition), [StreamError::UnboundedPattern] is
    /// returned.
    pub fn new(pattern: &str) -> Result<StreamFinder, StreamError> {
        let re = Regex::new(pattern).map_err(StreamError::Compile)?;
        let hir = regex_syntax::parse(pattern).map_err(|err| {
            StreamError::Compile(crate::Error::Syntax(err.to_string()))
        })?;
        let max_width = match hir.properties().maximum_len() {
            Some(len) => len,
            None => return Err(StreamError::UnboundedPattern),
        };
        Ok(StreamFinder {
            re,
            max_width,
            buf: Vec::new(),
            buf_start: 0,
            valid_len: 0,
            search_at: 0,
            last_match_end: None,
            state: State::Active,
        })
    }

    /// Returns the original pattern string.
    #[inline]
    pub fn pattern(&self) -> &str {
        self.re.as_str()
    }

    /// Feeds the next chunk of input bytes.
    ///
    /// The chunk may be of any size and may split a multi-byte UTF-8
    /// encoded char. Returns the matches that became fully determined
    /// by this chunk. Matches are returned in order and never overlap,
    /// and concatenating the results of all feed calls and the final
    /// finish call yields exactly what [Regex::find_iter] returns on
    /// the complete text.
    pub fn feed(
        &mut self,
        chunk: &[u8],
    ) -> Result<Vec<StreamMatch>, StreamError> {
        match self.state {
            State::Active => {}
            State::Finished => {
                self.state = State::Poisoned;
                return Err(StreamError::AlreadyFinished);
            }
            State::Poisoned => return Err(StreamError::Poisoned),
        }
        self.buf.extend_from_slice(chunk);
        match core::str::from_utf8(&self.buf[self.valid_len..]) {
            Ok(_) => self.valid_len = self.buf.len(),
            Err(err) => match err.error_len() {
                // A byte sequence that can never be completed into
                // valid UTF-8. The stream is corrupt.
                Some(_) => {
                    self.state = State::Poisoned;
                    return Err(StreamError::InvalidUtf8);
                }
                // The buffer ends with an incomplete UTF-8 sequence.
                // It may be completed by a later chunk.
                None => self.valid_len += err.valid_up_to(),
            },
        }
        Ok(self.drain_matches(false))
    }

    /// Declares the input complete and returns all remaining matches.
    ///
    /// If the buffered input ends in the middle of a UTF-8 encoded
    /// char, [StreamError::InvalidUtf8] is returned.
    pub fn finish(&mut self) -> Result<Vec<StreamMatch>, StreamError> {
        match self.state {
            State::Active => {}
            State::Finished => {
                self.state = State::Poisoned;
                return Err(StreamError::AlreadyFinished);
            }
            State::Poisoned => return Err(StreamError::Poisoned),
        }
        if self.valid_len != self.buf.len() {
            self.state = State::Poisoned;
            return Err(StreamError::InvalidUtf8);
        }
        self.state = State::Finished;
        Ok(self.drain_matches(true))
    }

    /// Returns the number of bytes currently retained in the internal
    /// buffer.
    ///
    /// This is a diagnostic accessor. The retained bytes are bounded
    /// by the pattern's maximum match width (plus one look-behind char
    /// and an incomplete trailing UTF-8 sequence) and the most
    /// recently fed chunk; the finder never accumulates the full
    /// input history.
    #[inline]
    pub fn buffered_len(&self) -> usize {
        self.buf.len()
    }

    /// Reports every match that is fully determined by the buffered
    /// input. When finishing is true, the buffered input is treated as
    /// the complete haystack.
    fn drain_matches(&mut self, finishing: bool) -> Vec<StreamMatch> {
        let mut matches = Vec::new();
        loop {
            let valid_end = self.buf_start + self.valid_len as u64;
            if self.search_at > valid_end {
                break;
            }
            let local_start = (self.search_at - self.buf_start) as usize;
            let found = {
                let hay = core::str::from_utf8(&self.buf[..self.valid_len])
                    .expect("buffer prefix is valid UTF-8");
                let mut m = self.re.find_at(hay, local_start);
                // Replicate the find_iter handling of an empty match
                // that starts where the previous match ended: retry
                // the search one byte later.
                if let Some(mm) = m {
                    if mm.start() == mm.end()
                        && Some(self.buf_start + mm.end() as u64)
                            == self.last_match_end
                    {
                        m = if local_start < hay.len() {
                            self.re.find_at(hay, local_start + 1)
                        } else {
                            None
                        };
                    }
                }
                m.map(|mm| (mm.start(), mm.end()))
            };
            let (start, end) = match found {
                None => break,
                Some(span) => span,
            };
            // A match is only final once every byte the search could
            // have inspected is known: the search may explore up to
            // max_width bytes past the match start, and look-around
            // at the end of that window (a trailing $ anchor or a
            // word boundary, say) needs one more byte. Otherwise more
            // input could still change or invalidate the match.
            let determined = finishing
                || match start.checked_add(self.max_width) {
                    Some(need) => need < self.valid_len,
                    None => false,
                };
            if !determined {
                break;
            }
            let text = core::str::from_utf8(&self.buf[start..end])
                .expect("match is valid UTF-8")
                .to_string();
            let gstart = self.buf_start + start as u64;
            let gend = self.buf_start + end as u64;
            matches.push(StreamMatch { start: gstart, end: gend, text });
            self.search_at = gend;
            self.last_match_end = Some(gend);
        }
        // Drop everything that can no longer influence future matches.
        // A match starting before valid_end - max_width would lie
        // entirely within the known input and would already have been
        // found, so only the tail of the buffer (plus one look-behind
        // char) can still matter.
        let keep_from = if finishing {
            self.search_at
        } else {
            let valid_end = self.buf_start + self.valid_len as u64;
            let floor = valid_end.saturating_sub(self.max_width as u64);
            core::cmp::max(self.search_at, floor)
        };
        self.compact(keep_from);
        matches
    }

    /// Advances the search position to keep_from (never backward) and
    /// drops all buffered bytes before it, retaining only the single
    /// UTF-8 encoded char immediately preceding it for look-behind
    /// context (line anchors and word boundaries).
    fn compact(&mut self, keep_from: u64) {
        let new_search = core::cmp::max(self.search_at, keep_from);
        if new_search == self.buf_start {
            self.search_at = new_search;
            return;
        }
        let rel = (new_search - self.buf_start) as usize;
        // rel <= valid_len and buf[..valid_len] is valid UTF-8, so
        // scanning back over continuation bytes finds the start of
        // the char containing the byte at rel - 1.
        let mut ctx = rel - 1;
        while ctx > 0 && self.buf[ctx] & 0xC0 == 0x80 {
            ctx -= 1;
        }
        if ctx > 0 {
            self.buf.drain(..ctx);
            self.buf_start += ctx as u64;
            self.valid_len -= ctx;
        }
        self.search_at = new_search;
    }
}
