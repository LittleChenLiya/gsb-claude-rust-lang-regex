/*!
Multi-rule replacement over a shared original haystack.

This module provides an independent component, MultiReplace, that applies a
set of replacement *rules* to a haystack. Unlike chaining several calls to
the Regex::replace_all method, every rule searches the same original text:
replacement text emitted by one rule is never fed back into any rule
(including itself), so replacement text cannot match again.

# Rule selection

All rules are first run independently over the complete original haystack to
collect *candidate* matches. Candidates are then totally ordered by:

1. Match start, earliest first (left to right).
2. Priority, highest first.
3. Match length, longest first.
4. Rule registration order, earliest first.

Candidates are scanned in that order and greedily accepted. An already
accepted interval blocks every later candidate that overlaps it, and at most
one candidate with any given start position can be accepted. Zero-width
matches participate in the exact same ordering as non-empty matches. A
zero-width candidate is shadowed when it falls strictly inside an accepted
non-empty interval, but a zero-width candidate exactly at the end of an
accepted non-empty interval is retained.

All reported byte ranges are byte offsets into the *original* haystack and
always lie on UTF-8 boundaries (this is a Unicode/str component).

# Limits

Both the number of candidates enumerated across all rules and the byte size
of the produced output can be capped. If either cap is exceeded, processing
fails before any result is returned; a partial result is never produced.
*/

use alloc::{
    string::{String, ToString},
    vec,
    vec::Vec,
};

use crate::Regex;

/// An error that can occur when building or executing a MultiReplace.
///
/// Neither this error nor any of its payloads ever contains a copy of the
/// processed haystack, so it is safe to surface even for sensitive input.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MultiReplaceError {
    /// A rule regex failed to compile.
    InvalidRegex {
        /// Zero-based rule id (registration order).
        rule: usize,
        /// Human readable compiler diagnostic.
        message: String,
    },
    /// A rule replacement template was invalid for its regex.
    ///
    /// A template is invalid when it references a capture group index that
    /// does not exist, a capture group name that does not exist, or when it
    /// contains an unclosed braced reference (a dollar sign immediately
    /// followed by an opening brace without a matching closing brace). All
    /// templates are validated at build time, before any haystack is
    /// processed.
    InvalidTemplate {
        /// Zero-based rule id (registration order).
        rule: usize,
        /// Human readable description of the problem.
        message: String,
    },
    /// The number of candidate matches enumerated across all rules exceeded
    /// the configured MultiReplaceBuilder::max_candidates cap.
    TooManyCandidates {
        /// The configured candidate cap.
        limit: usize,
        /// The number of candidates reached before giving up.
        found: usize,
    },
    /// The produced replacement would exceed the configured
    /// MultiReplaceBuilder::max_output_bytes cap. No partial output is
    /// returned.
    OutputTooLarge {
        /// The configured output byte cap.
        limit: usize,
    },
}

impl core::fmt::Display for MultiReplaceError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            MultiReplaceError::InvalidRegex { rule, message } => write!(
                f,
                "multi-replace rule {rule}: invalid regex: {message}"
            ),
            MultiReplaceError::InvalidTemplate { rule, message } => write!(
                f,
                "multi-replace rule {rule}: invalid replacement \
                 template: {message}"
            ),
            MultiReplaceError::TooManyCandidates { limit, found } => write!(
                f,
                "multi-replace candidate limit exceeded: found at least \
                 {found} candidates but the limit is {limit}"
            ),
            MultiReplaceError::OutputTooLarge { limit } => write!(
                f,
                "multi-replace output byte limit of {limit} exceeded"
            ),
        }
    }
}

#[cfg(feature = "std")]
impl std::error::Error for MultiReplaceError {}

/// A single replacement rule: a regex, an integer priority and a replacement
/// template.
///
/// The template uses the same capture interpolation semantics as the expand
/// method on Captures: dollar references such as $1 and $name, including the
/// braced form, refer to capture groups, and a doubled dollar sign is a
/// literal dollar sign.
///
/// Rules are registered with MultiReplaceBuilder::rule (or
/// MultiReplaceBuilder::add). The registration order is the zero-based rule
/// id reported in MatchReport and serves as the final tie breaker when two
/// candidates share a start position, priority and length.
#[derive(Clone, Debug)]
pub struct ReplacementRule {
    pattern: String,
    priority: i64,
    template: String,
}

impl ReplacementRule {
    /// Create a new rule from its regex source, priority and replacement
    /// template.
    ///
    /// The pattern is not compiled and the template is not validated until
    /// MultiReplaceBuilder::build is called.
    pub fn new(
        pattern: impl Into<String>,
        priority: i64,
        template: impl Into<String>,
    ) -> ReplacementRule {
        ReplacementRule {
            pattern: pattern.into(),
            priority,
            template: template.into(),
        }
    }

    /// Returns the rule regex pattern source.
    pub fn pattern(&self) -> &str {
        &self.pattern
    }

    /// Returns the rule priority; higher values win at a shared start.
    pub fn priority(&self) -> i64 {
        self.priority
    }

    /// Returns the rule capture replacement template.
    pub fn template(&self) -> &str {
        &self.template
    }
}

/// A builder for compiling a MultiReplace from a set of ReplacementRules.
///
/// Build it incrementally with MultiReplaceBuilder::new and
/// MultiReplaceBuilder::rule, or in one shot with MultiReplace::new.
///
/// Building compiles every regex and validates every replacement template
/// against its regex capture groups, so an invalid template is reported
/// before any input is ever processed.
///
/// # Example
///
/// ```
/// use regex::multi_replace::{MultiReplace, ReplacementRule};
///
/// let mr = MultiReplace::new([
///     ReplacementRule::new("dog", 0, "[redacted dog]"),
///     ReplacementRule::new("cat", 0, "[redacted cat]"),
/// ]).unwrap();
/// let out = mr.replace_all("a dog and a cat").unwrap();
/// assert_eq!(out.text, "a [redacted dog] and a [redacted cat]");
/// ```
#[derive(Clone, Debug, Default)]
pub struct MultiReplaceBuilder {
    rules: Vec<ReplacementRule>,
    max_candidates: Option<usize>,
    max_output_bytes: Option<usize>,
}

impl MultiReplaceBuilder {
    /// Create a new empty builder.
    pub fn new() -> MultiReplaceBuilder {
        MultiReplaceBuilder::default()
    }

    /// Register a rule and return its zero-based rule id (its registration
    /// order).
    pub fn rule(&mut self, rule: ReplacementRule) -> usize {
        let id = self.rules.len();
        self.rules.push(rule);
        id
    }

    /// Register a rule from its regex, priority and replacement template and
    /// return its zero-based rule id.
    ///
    /// ```
    /// use regex::multi_replace::MultiReplaceBuilder;
    ///
    /// let mut builder = MultiReplaceBuilder::new();
    /// builder.add(r"\d+", 1, "[num]");
    /// let mr = builder.build().unwrap();
    /// let out = mr.replace_all("abc 42 def 7").unwrap();
    /// assert_eq!(out.text, "abc [num] def [num]");
    /// ```
    pub fn add(
        &mut self,
        pattern: impl Into<String>,
        priority: i64,
        template: impl Into<String>,
    ) -> usize {
        self.rule(ReplacementRule::new(pattern, priority, template))
    }

    /// Cap the total number of candidate matches enumerated across all rules.
    ///
    /// If more candidates exist, replacing returns
    /// MultiReplaceError::TooManyCandidates and no output is produced. By
    /// default the number of candidates is unlimited.
    pub fn max_candidates(
        &mut self,
        limit: usize,
    ) -> &mut MultiReplaceBuilder {
        self.max_candidates = Some(limit);
        self
    }

    /// Cap the byte length of the produced replacement string.
    ///
    /// If the output would exceed the given limit, replacing returns
    /// MultiReplaceError::OutputTooLarge and no output is produced. By
    /// default the output size is unlimited.
    pub fn max_output_bytes(
        &mut self,
        limit: usize,
    ) -> &mut MultiReplaceBuilder {
        self.max_output_bytes = Some(limit);
        self
    }

    /// Compile all registered regexes and validate all replacement templates.
    pub fn build(&self) -> Result<MultiReplace, MultiReplaceError> {
        let mut compiled = Vec::with_capacity(self.rules.len());
        for (id, rule) in self.rules.iter().enumerate() {
            let regex = Regex::new(&rule.pattern).map_err(|_err| {
                MultiReplaceError::InvalidRegex {
                    rule: id,
                    message: "the regex pattern failed to compile".to_string(),
                }
            })?;
            validate_template(id, &regex, &rule.template)?;
            compiled.push(CompiledRule {
                regex,
                priority: rule.priority,
                template: rule.template.clone(),
            });
        }
        Ok(MultiReplace {
            rules: compiled,
            max_candidates: self.max_candidates,
            max_output_bytes: self.max_output_bytes,
        })
    }
}

#[derive(Clone, Debug)]
struct CompiledRule {
    regex: Regex,
    priority: i64,
    template: String,
}

/// A compiled set of independent replacement rules that all operate on the
/// original haystack passed to each call.
///
/// Construct one with MultiReplace::new or a MultiReplaceBuilder. It is
/// meant to be compiled once and reused across many inputs. See the module
/// documentation for the precise candidate ordering and overlap rules.
#[derive(Clone, Debug)]
pub struct MultiReplace {
    rules: Vec<CompiledRule>,
    max_candidates: Option<usize>,
    max_output_bytes: Option<usize>,
}

impl MultiReplace {
    /// Compile a MultiReplace directly from an iterable of rules.
    ///
    /// This is a convenience wrapper around MultiReplaceBuilder.
    pub fn new<I>(rules: I) -> Result<MultiReplace, MultiReplaceError>
    where
        I: IntoIterator<Item = ReplacementRule>,
    {
        let mut builder = MultiReplaceBuilder::new();
        for rule in rules {
            builder.rule(rule);
        }
        builder.build()
    }

    /// Returns the number of registered rules.
    pub fn len(&self) -> usize {
        self.rules.len()
    }

    /// Returns true if no rules were registered.
    pub fn is_empty(&self) -> bool {
        self.rules.is_empty()
    }

    /// Apply every rule to the haystack and return the new text together
    /// with a report of every candidate match, both adopted and shadowed.
    ///
    /// Every rule searches the same original haystack; text inserted by a
    /// replacement is never searched again. All ranges in Replacement::
    /// reports are byte offsets into the original haystack. Neither the
    /// reports nor error values contain copies of the haystack.
    pub fn replace_all(
        &self,
        haystack: &str,
    ) -> Result<Replacement, MultiReplaceError> {
        let mut candidates: Vec<Candidate> = Vec::new();
        for (rule_id, rule) in self.rules.iter().enumerate() {
            for match_ in rule.regex.find_iter(haystack) {
                if let Some(limit) = self.max_candidates {
                    if candidates.len() >= limit {
                        return Err(MultiReplaceError::TooManyCandidates {
                            limit,
                            found: limit + 1,
                        });
                    }
                }
                candidates.push(Candidate {
                    start: match_.start(),
                    end: match_.end(),
                    priority: rule.priority,
                    rule: rule_id,
                });
            }
        }

        candidates.sort_by(|a, b| {
            a.start
                .cmp(&b.start)
                .then_with(|| b.priority.cmp(&a.priority))
                .then_with(|| (b.end - b.start).cmp(&(a.end - a.start)))
                .then_with(|| a.rule.cmp(&b.rule))
        });

        // Greedy selection in canonical order. The accepted list stays
        // sorted by start because the scan itself runs left to right.
        let mut accepted: Vec<Candidate> = Vec::new();
        let mut adopted = vec![false; candidates.len()];
        for (idx, candidate) in candidates.iter().enumerate() {
            let blocked =
                accepted.iter().any(|chosen| overlaps(chosen, candidate));
            if !blocked {
                accepted.push(*candidate);
                adopted[idx] = true;
            }
        }

        let mut out = String::new();
        let mut cursor = 0usize;
        let mut reports = Vec::with_capacity(candidates.len());
        for (idx, candidate) in candidates.iter().enumerate() {
            if adopted[idx] {
                out.push_str(&haystack[cursor..candidate.start]);
                let caps = self.rules[candidate.rule]
                    .regex
                    .captures_at(haystack, candidate.start)
                    .expect("previously found match");
                debug_assert_eq!(
                    (caps.get_match().start(), caps.get_match().end()),
                    (candidate.start, candidate.end)
                );
                caps.expand(&self.rules[candidate.rule].template, &mut out);
                if let Some(limit) = self.max_output_bytes {
                    if out.len() > limit {
                        return Err(MultiReplaceError::OutputTooLarge {
                            limit,
                        });
                    }
                }
                cursor = candidate.end;
            }
            reports.push(MatchReport {
                rule: candidate.rule,
                range: candidate.start..candidate.end,
                adopted: adopted[idx],
            });
        }
        out.push_str(&haystack[cursor..]);
        if let Some(limit) = self.max_output_bytes {
            if out.len() > limit {
                return Err(MultiReplaceError::OutputTooLarge { limit });
            }
        }
        Ok(Replacement { text: out, reports })
    }
}

#[derive(Clone, Copy, Debug)]
struct Candidate {
    start: usize,
    end: usize,
    priority: i64,
    rule: usize,
}

/// Returns true if accepting the second candidate is forbidden because the
/// first, already accepted candidate blocks it.
///
/// Two intervals conflict when they share a start (at most one accepted
/// match per position) or when their half-open byte ranges overlap. Adjacent
/// intervals (the end of the first equals the start of the second) never
/// conflict. Consequently a zero-width candidate strictly inside an accepted
/// non-empty interval is blocked, while a zero-width candidate exactly at
/// its trailing end is retained.
fn overlaps(a: &Candidate, b: &Candidate) -> bool {
    a.start == b.start || (a.start < b.end && b.start < a.end)
}

/// The result of a successful MultiReplace::replace_all.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Replacement {
    /// The text produced by substituting every adopted match.
    pub text: String,
    /// One report per candidate match, in canonical selection order: start,
    /// then priority, then length, then rule id.
    pub reports: Vec<MatchReport>,
}

/// A report of one candidate match and whether it was adopted.
///
/// Only adopted matches contribute to Replacement::text; shadowed matches
/// are reported for diagnostics but leave the haystack untouched.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MatchReport {
    /// Zero-based rule id, i.e. the rule registration order.
    pub rule: usize,
    /// Byte range of the match in the original haystack.
    pub range: core::ops::Range<usize>,
    /// True if the match was adopted (and substituted); false if it was
    /// shadowed by an earlier-accepted candidate.
    pub adopted: bool,
}

/// Validate every capture reference in a replacement template against the
/// capture groups of its compiled regex.
///
/// This mirrors the accepted surface syntax used by Captures::expand:
///
/// - Two adjacent dollar signs are an escaped literal dollar sign.
/// - A dollar followed by [0-9A-Za-z_]+ is a numeric index or group name.
/// - A dollar-brace reference up to the closing brace is a numeric index or
///   group name; the braced name may contain arbitrary non-brace bytes.
/// - A lone dollar sign with no following reference (including a braced
///   reference that is never closed) is a template error.
/// - A numeric index that is out of range or a name that does not identify a
///   capture group is a template error.
fn validate_template(
    rule: usize,
    regex: &Regex,
    template: &str,
) -> Result<(), MultiReplaceError> {
    let groups = regex.captures_len();
    let bytes = template.as_bytes();
    let mut i = 0usize;
    while i < bytes.len() {
        if bytes[i] != b'$' {
            i += 1;
            continue;
        }
        if bytes.get(i + 1) == Some(&b'$') {
            i += 2;
            continue;
        }
        let mut j = i + 1;
        if j >= bytes.len() {
            return Err(template_error(
                rule,
                "trailing dollar sign with no capture reference",
            ));
        }
        if bytes[j] == b'{' {
            let start = j + 1;
            let mut end = start;
            while end < bytes.len() && bytes[end] != b'}' {
                end += 1;
            }
            if end >= bytes.len() {
                return Err(template_error(
                    rule,
                    "unclosed braced capture reference",
                ));
            }
            let name =
                core::str::from_utf8(&bytes[start..end]).map_err(|_| {
                    template_error(
                        rule,
                        "braced capture reference is not valid UTF-8",
                    )
                })?;
            check_reference(rule, regex, groups, name)?;
            i = end + 1;
            continue;
        }
        let start = j;
        while j < bytes.len() && is_ref_byte(bytes[j]) {
            j += 1;
        }
        if j == start {
            return Err(template_error(
                rule,
                "dollar sign is not followed by a capture reference",
            ));
        }
        let name = core::str::from_utf8(&bytes[start..j])
            .expect("reference bytes are ASCII alphanumeric or underscore");
        check_reference(rule, regex, groups, name)?;
        i = j;
    }
    Ok(())
}

/// Build an InvalidTemplate error without embedding any haystack content.
fn template_error(rule: usize, message: &str) -> MultiReplaceError {
    MultiReplaceError::InvalidTemplate { rule, message: message.to_string() }
}

/// Check that a single capture reference (numeric or named) resolves.
fn check_reference(
    rule: usize,
    regex: &Regex,
    groups: usize,
    reference: &str,
) -> Result<(), MultiReplaceError> {
    if let Ok(index) = reference.parse::<usize>() {
        if index >= groups {
            return Err(MultiReplaceError::InvalidTemplate {
                rule,
                message: alloc::format!(
                    "capture group index {index} does not exist: the regex \
                     defines {groups} groups including group 0"
                ),
            });
        }
        return Ok(());
    }
    if regex.capture_names().any(|name| name == Some(reference)) {
        return Ok(());
    }
    Err(MultiReplaceError::InvalidTemplate {
        rule,
        message: alloc::format!(
            "there is no capture group named {reference:?}"
        ),
    })
}

/// Returns true for bytes allowed in an unbraced capture reference, matching
/// the interpolation grammar ([0-9A-Za-z_]).
fn is_ref_byte(byte: u8) -> bool {
    matches!(byte, b'0'..=b'9' | b'a'..=b'z' | b'A'..=b'Z' | b'_')
}
