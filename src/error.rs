use alloc::string::{String, ToString};

use regex_automata::meta;

/// An error that occurred during parsing or compiling a regular expression.
#[non_exhaustive]
#[derive(Clone, PartialEq)]
pub enum Error {
    /// A syntax error.
    Syntax(String),
    /// The compiled program exceeded the set size
    /// limit. The argument is the size limit imposed by
    /// [`RegexBuilder::size_limit`](crate::RegexBuilder::size_limit). Even
    /// when not configured explicitly, it defaults to a reasonable limit.
    ///
    /// If you're getting this error, it occurred because your regex has been
    /// compiled to an intermediate state that is too big. It is important to
    /// note that exceeding this limit does _not_ mean the regex is too big to
    /// _work_, but rather, the regex is big enough that it may wind up being
    /// surprisingly slow when used in a search. In other words, this error is
    /// meant to be a practical heuristic for avoiding a performance footgun,
    /// and especially so for the case where the regex pattern is coming from
    /// an untrusted source.
    ///
    /// There are generally two ways to move forward if you hit this error.
    /// The first is to find some way to use a smaller regex. The second is to
    /// increase the size limit via `RegexBuilder::size_limit`. However, if
    /// your regex pattern is not from a trusted source, then neither of these
    /// approaches may be appropriate. Instead, you'll have to determine just
    /// how big of a regex you want to allow.
    CompiledTooBig(usize),
}

impl Error {
    pub(crate) fn from_meta_build_error(err: meta::BuildError) -> Error {
        if let Some(size_limit) = err.size_limit() {
            Error::CompiledTooBig(size_limit)
        } else if let Some(ref err) = err.syntax_error() {
            Error::Syntax(err.to_string())
        } else {
            // This is a little suspect. Technically there are more ways for
            // a meta regex to fail to build other than "exceeded size limit"
            // and "syntax error." For example, if there are too many states
            // or even too many patterns. But in practice this is probably
            // good enough. The worst thing that happens is that Error::Syntax
            // represents an error that isn't technically a syntax error, but
            // the actual message will still be shown. So... it's not too bad.
            //
            // We really should have made the Error type in the regex crate
            // completely opaque. Rookie mistake.
            Error::Syntax(err.to_string())
        }
    }
}

#[cfg(feature = "std")]
impl std::error::Error for Error {
    // TODO: Remove this method entirely on the next breaking semver release.
    #[allow(deprecated)]
    fn description(&self) -> &str {
        match *self {
            Error::Syntax(ref err) => err,
            Error::CompiledTooBig(_) => "compiled program too big",
        }
    }
}

impl core::fmt::Display for Error {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match *self {
            Error::Syntax(ref err) => err.fmt(f),
            Error::CompiledTooBig(limit) => write!(
                f,
                "Compiled regex exceeds size limit of {limit} bytes.",
            ),
        }
    }
}

// We implement our own Debug implementation so that we show nicer syntax
// errors when people use `Regex::new(...).unwrap()`. It's a little weird,
// but the `Syntax` variant is already storing a `String` anyway, so we might
// as well format it nicely.
impl core::fmt::Debug for Error {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match *self {
            Error::Syntax(ref err) => {
                let hr: String = core::iter::repeat('~').take(79).collect();
                writeln!(f, "Syntax(")?;
                writeln!(f, "{hr}")?;
                writeln!(f, "{err}")?;
                writeln!(f, "{hr}")?;
                write!(f, ")")?;
                Ok(())
            }
            Error::CompiledTooBig(limit) => {
                f.debug_tuple("CompiledTooBig").field(&limit).finish()
            }
        }
    }
}

/// An error returned by bounded replacement routines when the output would
/// exceed a configured byte budget.
///
/// The byte budget counts the entire replacement result: every unmatched byte
/// copied from the haystack plus every byte produced by the replacement
/// callback. If the budget is exceeded, the routine returns this error
/// *before* producing any output, so no half-built string is ever returned.
///
/// See [`Regex::replace_all_bounded`](crate::Regex::replace_all_bounded) for
/// more details and examples.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BoundedReplaceError {
    budget: usize,
    needed: usize,
}

impl BoundedReplaceError {
    /// Creates a new bounded replacement error.
    pub(crate) fn new(budget: usize, needed: usize) -> BoundedReplaceError {
        BoundedReplaceError { budget, needed }
    }

    /// Returns the byte budget that was exceeded.
    pub fn budget(&self) -> usize {
        self.budget
    }

    /// Returns the number of output bytes that would have been used up to and
    /// including the match that caused the budget to be exceeded.
    ///
    /// This is a lower bound on the total size of the replacement result,
    /// since matches after the failing one have not yet been processed.
    pub fn needed(&self) -> usize {
        self.needed
    }
}

impl core::fmt::Display for BoundedReplaceError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(
            f,
            "bounded replacement exceeded byte budget: needed at least {} \
             bytes but the budget is {} bytes",
            self.needed, self.budget,
        )
    }
}

#[cfg(feature = "std")]
impl std::error::Error for BoundedReplaceError {}
