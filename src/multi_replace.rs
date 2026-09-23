@T@/*!
Multi-rule replacement over a single, shared original haystack.

This module provides an independent component for applying a set of
replacement [Rule]s to a haystack according to fixed competition rules:

* Every rule searches the same original haystack. Text produced by a
  replacement is never fed back into any rule, so replacement text that
  happens to match a pattern never triggers further matches.
* Candidate matches are ordered by:
  1. start byte offset, earliest first;
  2. [Rule::priority], highest first (at the same start);
  3. match byte length, longest first (same start and priority);
  4. rule registration order (as the final tiebreaker).
* Once a candidate is selected, any later candidate that overlaps it is
  reported as shadowed. At most one candidate is selected at any given
  start offset. Zero-width matches take part in the exact same ordering
  as non-empty matches.
* A zero-width candidate that falls strictly inside an already selected
  non-empty match is shadowed; one positioned exactly at the end of such
  a match is kept. Adjacent, non-overlapping matches never block each
  other.
* All offsets are byte offsets, and only boundaries reported by the
  regex engines are used for slicing, so UTF-8 boundaries are never
  crossed. Zero-width matches are enumerated exactly once and cannot
  cause an infinite loop.

Replacement templates use the standard regex capture semantics:
@DO@1, @DO@LB@nameRB@ for a named group, @DO@0 for the whole match, and
@DO@@DO@ for a literal dollar sign. Every template is validated when its
rule is added, i.e. before any haystack is processed. Validation is
strict: every dollar sign must either be escaped as @DO@@DO@ or reference
a capture group that exists in that rule's regex. An unknown group or an
unterminated brace makes the rule (and therefore the whole replacement
set) fail to build, so replacement can never run with a bad template.

# Example

@T@@T@@T@
use regex::multi_replace::{MultiReplaceBuilder, Rule, RuleId};

let replacer = MultiReplaceBuilder::new()
    .add_rule(Rule::new(RuleId::new(0), r"abc", "[@DO@0]", 1))?
    .add_rule(Rule::new(RuleId::new(1), r"bcde", "<@DO@0>", 9))?
    .build()?;

let out = replacer.replace("abcde")?;
// abc starts earlier than bcde, so the higher-priority bcde candidate
// is shadowed even though its priority is greater.
assert_eq!(out.result, "[abc]de");
# Ok::<(), Box<dyn std::error::Error>>(())
@T@@T@@T@
*/

use alloc::string::String;
use alloc::string::ToString;
use alloc::vec;
use alloc::vec::Vec;
use core::fmt;
use core::str;

use crate::Regex;

/// The identifier of a replacement [Rule].
///
/// Identifiers only need to be unique within a single replacement set.
/// They are reported back verbatim in [MatchReport]s.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub struct RuleId(usize);

impl RuleId {
    /// Creates a new rule identifier.
    pub fn new(id: usize) -> RuleId {
        RuleId(id)
    }

    /// Returns the identifier as a plain integer.
    pub fn as_usize(self) -> usize {
        self.0
    }
}

impl fmt::Display for RuleId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// A replacement rule: a regex, a capture-reference template, and the
/// priority used when multiple rules match at the same start offset.
///
/// A rule is an immutable description. It is compiled and validated
/// when added to a [MultiReplaceBuilder].
#[derive(Clone, Debug)]
pub struct Rule {
    id: RuleId,
    pattern: String,
    replacement: String,
    priority: i64,
}

impl Rule {
    /// Creates a new rule.
    ///
    /// `priority` follows "higher wins" semantics for matches that begin
    /// at the same byte offset. `template` uses the standard capture
    /// syntax (@DO@1, the brace form for named groups, @DO@0, @DO@@DO@);
    /// it is checked against `pattern` when the rule is added to a
    /// builder.
    pub fn new(
        id: RuleId,
        pattern: &str,
        template: &str,
        priority: i64,
    ) -> Rule {
        Rule {
            id,
            pattern: pattern.to_string(),
            replacement: template.to_string(),
            priority,
        }
    }

    /// Returns this rule's identifier.
    pub fn id(&self) -> RuleId {
        self.id
    }

    /// Returns this rule's regex pattern string.
    pub fn pattern(&self) -> &str {
        &self.pattern
    }

    /// Returns this rule's replacement template.
    pub fn replacement(&self) -> &str {
        &self.replacement
    }

    /// Returns this rule's priority.
    pub fn priority(&self) -> i64 {
        self.priority
    }
}

/// A configurable builder for a compiled [MultiReplacer].
#[derive(Clone, Debug, Default)]
pub struct MultiReplaceBuilder {
    rules: Vec<Rule>,
    max_candidates: Option<usize>,
    max_output_bytes: Option<usize>,
}

impl MultiReplaceBuilder {
    /// Creates a new builder with no rules and no limits.
    pub fn new() -> MultiReplaceBuilder {
        MultiReplaceBuilder::default()
    }

    /// Registers a rule.
    ///
    /// The rule's regex is compiled and its replacement template is
    /// validated immediately, so bad patterns or templates fail before
    /// any haystack is processed. Registration order is the final
    /// tiebreaker during match selection; rule identifiers must be
    /// unique within the builder.
    pub fn add_rule(
        &mut self,
        rule: Rule,
    ) -> Result<&mut Self, MultiReplaceError> {
        if self.rules.iter().any(|existing| existing.id() == rule.id()) {
            return Err(MultiReplaceError::DuplicateRuleId(rule.id()));
        }
        let re =
            Regex::new(&rule.pattern).map_err(|source| {
                MultiReplaceError::Build { rule: rule.id(), source }
            })?;
        validate_template(rule.id(), &rule.replacement, &re)?;
        self.rules.push(rule);
        Ok(self)
    }

    /// Sets the maximum number of candidate matches collected from a
    /// haystack.
    ///
    /// Candidates are counted across all rules, before overlap
    /// selection. If more matches exist, replacement fails with
    /// [MultiReplaceError::TooManyCandidates] and no partial result is
    /// produced.
    pub fn max_candidates(&mut self, limit: usize) -> &mut Self {
        self.max_candidates = Some(limit);
        self
    }

    /// Sets the maximum size, in bytes, of the produced string.
    ///
    /// If the fully assembled result would exceed this limit,
    /// replacement fails with [MultiReplaceError::OutputTooLarge] and
    /// no partial result is produced.
    pub fn max_output_bytes(&mut self, limit: usize) -> &mut Self {
        self.max_output_bytes = Some(limit);
        self
    }

    /// Compiles all registered rules into a [MultiReplacer].
    pub fn build(&self) -> Result<MultiReplacer, MultiReplaceError> {
        let mut compiled = Vec::with_capacity(self.rules.len());
        for rule in &self.rules {
            let re =
                Regex::new(&rule.pattern).map_err(|source| {
                    MultiReplaceError::Build { rule: rule.id(), source }
                })?;
            validate_template(rule.id(), &rule.replacement, &re)?;
            compiled.push(CompiledRule {
                id: rule.id(),
                re,
                template: rule.replacement.clone(),
                priority: rule.priority,
            });
        }
        Ok(MultiReplacer {
            rules: compiled,
            max_candidates: self.max_candidates,
            max_output_bytes: self.max_output_bytes,
        })
    }
}

#[derive(Clone, Debug)]
struct CompiledRule {
    id: RuleId,
    re: Regex,
    template: String,
    priority: i64,
}

/// A compiled, reusable set of replacement rules.
///
/// Construct one with [MultiReplaceBuilder]. It is cheap to reuse
/// across many haystacks.
#[derive(Clone, Debug)]
pub struct MultiReplacer {
    rules: Vec<CompiledRule>,
    max_candidates: Option<usize>,
    max_output_bytes: Option<usize>,
}

impl MultiReplacer {
    /// Returns the number of registered rules.
    pub fn rule_count(&self) -> usize {
        self.rules.len()
    }

    /// Applies all rules to `haystack` and returns the new string along
    /// with a report of every adopted or shadowed match.
    ///
    /// All rules see the same `haystack`; inserted replacement text is
    /// never re-matched. On error (for example when a configured limit
    /// is exceeded), no partial result is returned.
    pub fn replace(
        &self,
        haystack: &str,
    ) -> Result<Replacement, MultiReplaceError> {
        let mut candidates = self.collect_candidates(haystack)?;

        candidates.sort_by(|a, b| {
            // Start offset: earliest first.
            a.start
                .cmp(&b.start)
                // Priority: highest first.
                .then_with(|| b.priority.cmp(&a.priority))
                // Length: longest first.
                .then_with(|| b.len().cmp(&a.len()))
                // Registration order: earliest first.
                .then_with(|| a.order.cmp(&b.order))
        });

        let status = select(&candidates);

        let mut inserted_len: usize = 0;
        let mut adopted_len: usize = 0;
        for (idx, decision) in status.iter().enumerate() {
            if decision.is_some() {
                continue;
            }
            inserted_len =
                inserted_len.checked_add(candidates[idx].expanded.len()).ok_or(
                    MultiReplaceError::OutputTooLarge {
                        limit: self.max_output_bytes.unwrap_or(usize::MAX),
                        needed: usize::MAX,
                    },
                )?;
            adopted_len = adopted_len
                .checked_add(candidates[idx].len())
                .ok_or(MultiReplaceError::OutputTooLarge {
                    limit: self.max_output_bytes.unwrap_or(usize::MAX),
                    needed: usize::MAX,
                })?;
        }
        let needed = haystack
            .len()
            .checked_sub(adopted_len)
            .and_then(|rest| rest.checked_add(inserted_len))
            .ok_or(MultiReplaceError::OutputTooLarge {
                limit: self.max_output_bytes.unwrap_or(usize::MAX),
                needed: usize::MAX,
            })?;
        if let Some(limit) = self.max_output_bytes {
            if needed > limit {
                return Err(MultiReplaceError::OutputTooLarge { limit, needed });
            }
        }

        // The size check above runs before anything is assembled, so a
        // limit failure never leaves a partial result behind.
        let mut result = String::with_capacity(needed);
        let mut last_end = 0usize;
        for (idx, decision) in status.iter().enumerate() {
            if decision.is_some() {
                continue;
            }
            let cand = &candidates[idx];
            // start/end come straight from a regex match, which only
            // reports valid UTF-8 boundaries.
            result.push_str(&haystack[last_end..cand.start]);
            result.push_str(&cand.expanded);
            last_end = cand.end;
        }
        result.push_str(&haystack[last_end..]);

        let mut reports = Vec::with_capacity(candidates.len());
        for (idx, decision) in status.iter().enumerate() {
            let cand = &candidates[idx];
            let status = match *decision {
                None => MatchStatus::Adopted,
                Some(blocker) => MatchStatus::Shadowed {
                    blocked_by: candidates[blocker].id,
                },
            };
            reports.push(MatchReport {
                rule: cand.id,
                start: cand.start,
                end: cand.end,
                status,
            });
        }
        Ok(Replacement { result, reports })
    }

    fn collect_candidates(
        &self,
        haystack: &str,
    ) -> Result<Vec<Candidate>, MultiReplaceError> {
        let mut candidates = Vec::new();
        for (order, rule) in self.rules.iter().enumerate() {
            // captures_iter yields zero-width matches at advancing
            // positions just like the standard regex iterators, so this
            // loop always terminates.
            for caps in rule.re.captures_iter(haystack) {
                if let Some(limit) = self.max_candidates {
                    if candidates.len() >= limit {
                        return Err(MultiReplaceError::TooManyCandidates {
                            limit,
                        });
                    }
                }
                let whole = caps
                    .get(0)
                    .expect("a reported capture match has group 0");
                let mut expanded = String::new();
                caps.expand(&rule.template, &mut expanded);
                candidates.push(Candidate {
                    start: whole.start(),
                    end: whole.end(),
                    order,
                    id: rule.id,
                    priority: rule.priority,
                    expanded,
                });
            }
        }
        Ok(candidates)
    }
}
