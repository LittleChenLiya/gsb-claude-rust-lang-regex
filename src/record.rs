/*!
A library-level record parser built on top of [`Regex`].

A [`RecordParser`] is constructed from a prioritized list of
[`RecordRule`]s. Each rule pairs a regex (which must match the entire
line) with a list of [`FieldSpec`]s that describe how to convert capture
groups into typed values. When parsing a line, rules are tried in order
and the first rule whose regex matches the entire line is used. If that
rule matches but one of its field conversions fails, a
[`ParseError::Convert`] is returned that identifies the rule and the
byte range of the offending capture. Lower priority rules are never
tried in that case, so a broad "fallback" rule cannot silently mask a
malformed field.

Capture groups that did not participate in the match (e.g. the group
inside an unmatched `(?:...)?`) are reported as
[`FieldValue::Missing`], which is distinct from a group that
participated but matched the empty string.

# Example

```
use regex::record::{
    FieldKind, FieldSpec, FieldValue, RecordParser, RecordRule,
};

let parser = RecordParser::new(vec![RecordRule::new(
    r"id=(\d+) ok=(\w+)",
    vec![
        FieldSpec::group(1, FieldKind::U64),
        FieldSpec::group(2, FieldKind::Bool),
    ],
)])?;
let record = parser.parse_line("id=12 ok=true")?;
assert_eq!(
    record.fields,
    vec![FieldValue::U64(12), FieldValue::Bool(true)]
);
# Ok::<(), Box<dyn std::error::Error>>(())
```
*/

use alloc::{
    format,
    string::{String, ToString},
    vec::Vec,
};

use crate::Regex;

/// The type a capture group should be converted to.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FieldKind {
    /// A 64-bit unsigned integer, parsed with `u64::from_str`.
    U64,
    /// A 64-bit signed integer, parsed with `i64::from_str`.
    I64,
    /// A boolean. Only `true` and `false` are accepted.
    Bool,
    /// The raw captured string, with no conversion applied.
    Str,
}

impl FieldKind {
    fn name(&self) -> &'static str {
        match *self {
            FieldKind::U64 => "u64",
            FieldKind::I64 => "i64",
            FieldKind::Bool => "bool",
            FieldKind::Str => "string",
        }
    }
}

/// Which capture group a [`FieldSpec`] refers to.
#[derive(Clone, Debug, Eq, PartialEq)]
enum GroupRef {
    /// A capture group index, where `0` is the entire match.
    Index(usize),
    /// A named capture group.
    Name(String),
}

/// A mapping from a capture group to a typed field.
///
/// Field specs are evaluated in order for whichever rule matches a line,
/// and each one produces one entry in [`Record::fields`].
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FieldSpec {
    group: GroupRef,
    kind: FieldKind,
}

impl FieldSpec {
    /// Create a spec that reads the capture group at `index` and
    /// converts it according to `kind`.
    ///
    /// Note that index `0` refers to the entire match.
    pub fn group(index: usize, kind: FieldKind) -> FieldSpec {
        FieldSpec { group: GroupRef::Index(index), kind }
    }

    /// Create a spec that reads the named capture group `name` and
    /// converts it according to `kind`.
    pub fn named(name: &str, kind: FieldKind) -> FieldSpec {
        FieldSpec { group: GroupRef::Name(name.to_string()), kind }
    }

    /// Returns the type this field is converted to.
    pub fn kind(&self) -> FieldKind {
        self.kind
    }
}

/// A single parsing rule: a regex that must match the entire line, plus
/// the field conversions applied to its capture groups.
///
/// Rules are tried in the order they were given to
/// [`RecordParser::new`], and only the first rule whose regex matches
/// the entire line is used for that line.
#[derive(Clone, Debug)]
pub struct RecordRule {
    pattern: String,
    fields: Vec<FieldSpec>,
}

impl RecordRule {
    /// Create a rule from a regex pattern and its field conversions.
    ///
    /// The pattern is implicitly anchored: it must match the entire line
    /// for the rule to apply.
    pub fn new(pattern: &str, fields: Vec<FieldSpec>) -> RecordRule {
        RecordRule { pattern: pattern.to_string(), fields }
    }

    /// Returns the pattern this rule was created with.
    pub fn pattern(&self) -> &str {
        &self.pattern
    }

    /// Returns the field conversions applied when this rule matches.
    pub fn fields(&self) -> &[FieldSpec] {
        &self.fields
    }
}

/// A compiled rule, with named groups resolved to indices.
#[derive(Clone, Debug)]
struct CompiledRule {
    regex: Regex,
    fields: Vec<CompiledField>,
}

#[derive(Clone, Debug)]
struct CompiledField {
    group: usize,
    kind: FieldKind,
}

/// A parser that converts lines into typed records using a prioritized
/// list of rules.
///
/// See the [module documentation](self) for details and an example.
#[derive(Clone, Debug)]
pub struct RecordParser {
    rules: Vec<CompiledRule>,
}

impl RecordParser {
    /// Compile a prioritized list of rules into a parser.
    ///
    /// This returns an error if any rule's pattern fails to compile or
    /// refers to a capture group that does not exist.
    pub fn new(rules: Vec<RecordRule>) -> Result<RecordParser, Error> {
        let mut compiled = Vec::with_capacity(rules.len());
        for (i, rule) in rules.iter().enumerate() {
            // Anchor the pattern so that it must match the entire line.
            // The non-capturing group keeps the caller's group indices
            // intact.
            let pattern = format!("^(?:{})$", rule.pattern);
            let regex = Regex::new(&pattern).map_err(|err| {
                Error::Compile { rule: i, pattern: rule.pattern.clone(), err }
            })?;
            let mut fields = Vec::with_capacity(rule.fields.len());
            for spec in rule.fields.iter() {
                let group = match spec.group {
                    GroupRef::Index(index) => {
                        if index >= regex.captures_len() {
                            return Err(Error::UnknownGroup {
                                rule: i,
                                group: index.to_string(),
                            });
                        }
                        index
                    }
                    GroupRef::Name(ref name) => {
                        let index = regex
                            .capture_names()
                            .enumerate()
                            .find(|&(_, n)| n == Some(name.as_str()))
                            .map(|(index, _)| index);
                        match index {
                            Some(index) => index,
                            None => {
                                return Err(Error::UnknownGroup {
                                    rule: i,
                                    group: name.clone(),
                                });
                            }
                        }
                    }
                };
                fields.push(CompiledField { group, kind: spec.kind });
            }
            compiled.push(CompiledRule { regex, fields });
        }
        Ok(RecordParser { rules: compiled })
    }

    /// Parse a single line into a record.
    ///
    /// Rules are tried in priority order and the first rule whose regex
    /// matches the entire line is used. If that rule's field conversions
    /// fail, a [`ParseError::Convert`] error is returned and no other
    /// rule is tried. If no rule matches the line at all, a
    /// [`ParseError::NoMatch`] error is returned.
    pub fn parse_line(&self, line: &str) -> Result<Record, ParseError> {
        for (rule_index, rule) in self.rules.iter().enumerate() {
            let caps = match rule.regex.captures(line) {
                Some(caps) => caps,
                None => continue,
            };
            let mut fields = Vec::with_capacity(rule.fields.len());
            for spec in rule.fields.iter() {
                let group = match caps.get(spec.group) {
                    None => {
                        // The group did not participate in the match.
                        // This is distinct from a group that matched the
                        // empty string.
                        fields.push(FieldValue::Missing);
                        continue;
                    }
                    Some(group) => group,
                };
                let raw = &line[group.range()];
                let value = match convert(spec.kind, raw) {
                    Some(value) => value,
                    None => {
                        return Err(ParseError::Convert(ConvertError {
                            rule: rule_index,
                            group: spec.group,
                            kind: spec.kind,
                            span: Span {
                                start: group.start(),
                                end: group.end(),
                            },
                            text: raw.to_string(),
                        }));
                    }
                };
                fields.push(value);
            }
            return Ok(Record { rule: rule_index, fields });
        }
        Err(ParseError::NoMatch(NoMatchError { line: line.to_string() }))
    }

    /// Parse many lines, continuing past lines that fail to parse.
    ///
    /// The returned vector has one entry per input line, in order. A
    /// parse error on one line does not stop later lines from being
    /// parsed.
    pub fn parse_lines<'a, I>(
        &self,
        lines: I,
    ) -> Vec<Result<Record, ParseError>>
    where
        I: IntoIterator<Item = &'a str>,
    {
        lines.into_iter().map(|line| self.parse_line(line)).collect()
    }
}

/// Convert a captured string to a typed value. Returns `None` when the
/// conversion fails.
fn convert(kind: FieldKind, raw: &str) -> Option<FieldValue> {
    match kind {
        FieldKind::U64 => raw.parse::<u64>().ok().map(FieldValue::U64),
        FieldKind::I64 => raw.parse::<i64>().ok().map(FieldValue::I64),
        FieldKind::Bool => match raw {
            "true" => Some(FieldValue::Bool(true)),
            "false" => Some(FieldValue::Bool(false)),
            _ => None,
        },
        FieldKind::Str => Some(FieldValue::Str(raw.to_string())),
    }
}

/// A parsed record: the index of the rule that matched and the typed
/// field values produced by its field specs, in order.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Record {
    /// The index (into the rules given to [`RecordParser::new`]) of the
    /// rule that matched the line.
    pub rule: usize,
    /// The typed field values, one per [`FieldSpec`] of the matched
    /// rule.
    pub fields: Vec<FieldValue>,
}

/// A typed field value produced by parsing a line.
#[non_exhaustive]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum FieldValue {
    /// A 64-bit unsigned integer.
    U64(u64),
    /// A 64-bit signed integer.
    I64(i64),
    /// A boolean.
    Bool(bool),
    /// A raw captured string. A group that participated in the match but
    /// matched the empty string yields `Str("")`, which is distinct
    /// from `Missing`.
    Str(String),
    /// The capture group did not participate in the match (e.g. it was
    /// inside an optional part of the pattern that went unused).
    Missing,
}

/// A byte range in the parsed line.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Span {
    /// The byte offset of the start of the range (inclusive).
    pub start: usize,
    /// The byte offset of the end of the range (exclusive).
    pub end: usize,
}

/// An error that occurred while parsing a line.
#[non_exhaustive]
#[derive(Clone, Debug)]
pub enum ParseError {
    /// No rule matched the line.
    NoMatch(NoMatchError),
    /// A rule matched the line, but one of its capture groups could not
    /// be converted to the requested type. Lower priority rules are not
    /// tried when this happens.
    Convert(ConvertError),
}

impl core::fmt::Display for ParseError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match *self {
            ParseError::NoMatch(ref err) => err.fmt(f),
            ParseError::Convert(ref err) => err.fmt(f),
        }
    }
}

impl std::error::Error for ParseError {}

/// An error indicating that no rule matched a line.
#[derive(Clone, Debug)]
pub struct NoMatchError {
    line: String,
}

impl NoMatchError {
    /// Returns the line that no rule matched.
    pub fn line(&self) -> &str {
        &self.line
    }
}

impl core::fmt::Display for NoMatchError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "no rule matched line: {:?}", self.line)
    }
}

impl std::error::Error for NoMatchError {}

/// An error indicating that a capture group matched by a rule could not
/// be converted to the requested type.
#[derive(Clone, Debug)]
pub struct ConvertError {
    rule: usize,
    group: usize,
    kind: FieldKind,
    span: Span,
    text: String,
}

impl ConvertError {
    /// The index (into the rules given to [`RecordParser::new`]) of the
    /// rule that matched the line.
    pub fn rule(&self) -> usize {
        self.rule
    }

    /// The index of the capture group whose conversion failed.
    pub fn group(&self) -> usize {
        self.group
    }

    /// The type the capture group failed to convert to.
    pub fn kind(&self) -> FieldKind {
        self.kind
    }

    /// The byte range of the capture group in the original line.
    pub fn span(&self) -> Span {
        self.span
    }

    /// The captured text that failed to convert.
    pub fn text(&self) -> &str {
        &self.text
    }
}

impl core::fmt::Display for ConvertError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(
            f,
            "rule {}: capture group {} (bytes {}..{}) could not be \
             converted to {}: {:?}",
            self.rule,
            self.group,
            self.span.start,
            self.span.end,
            self.kind.name(),
            self.text,
        )
    }
}

impl std::error::Error for ConvertError {}

/// An error that occurred while building a [`RecordParser`].
#[non_exhaustive]
#[derive(Clone, Debug)]
pub enum Error {
    /// A rule's pattern failed to compile.
    Compile {
        /// The index of the offending rule.
        rule: usize,
        /// The pattern that failed to compile.
        pattern: String,
        /// The underlying regex compile error.
        err: crate::Error,
    },
    /// A rule refers to a capture group that does not exist.
    UnknownGroup {
        /// The index of the offending rule.
        rule: usize,
        /// The group index or name that does not exist.
        group: String,
    },
}

impl core::fmt::Display for Error {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match *self {
            Error::Compile { rule, ref pattern, ref err } => write!(
                f,
                "rule {}: pattern {:?} failed to compile: {}",
                rule, pattern, err,
            ),
            Error::UnknownGroup { rule, ref group } => {
                write!(f, "rule {}: no such capture group: {}", rule, group)
            }
        }
    }
}

impl std::error::Error for Error {}
