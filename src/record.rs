/*!
Priority-ordered record parsing built on top of [`Regex`].

This module provides [`RecordParser`], a small library-level utility for
turning lines of text into typed records. The caller supplies an ordered
list of rules. Each rule pairs a regular expression with a type conversion
for each of its capture groups. Supported conversions are unsigned 64-bit
integers, booleans and raw strings.

Rules are tried in priority order and a rule only applies when its overall
match spans the entire line. The first rule that fully matches a line is
the only rule used for that line. In particular, if the regex matches but
converting one of its capture groups fails, the error is reported (along
with the rule and the byte range of the captured text) and later rules are
*not* tried. A later "fallback" rule can therefore never mask a
conversion failure from an earlier rule.

Capture groups that are optional and do not participate in the match are
distinguished from groups that participate but capture the empty string:
the former produce a [`Field`] whose value is `None`, while the latter
produce a value (for example, an empty string for [`FieldType::Str`]).

# Example

```
use regex::record::{FieldType, FieldValue, RecordParser};

let parser = RecordParser::builder()
    .rule(r"^id=(\d+) ok=(\w+)$", &[FieldType::UInt, FieldType::Bool])
    .build()
    .unwrap();

let record = parser.parse_line("id=12 ok=true").unwrap();
assert_eq!(record.get(0).unwrap().value(), Some(&FieldValue::UInt(12)));
assert_eq!(record.get(1).unwrap().value(), Some(&FieldValue::Bool(true)));
```
*/

use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::fmt;
use core::ops::Range;

use crate::{Error, Regex};

/// The type of conversion applied to a capture group in a rule.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum FieldType {
    /// An unsigned 64-bit integer, parsed with `u64::from_str`.
    UInt,
    /// A boolean. Only `true` and `false` are accepted.
    Bool,
    /// A raw string. The captured text is used as-is and never fails.
    Str,
}

impl fmt::Display for FieldType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            FieldType::UInt => write!(f, "u64"),
            FieldType::Bool => write!(f, "bool"),
            FieldType::Str => write!(f, "string"),
        }
    }
}

/// A typed field value produced by a successful conversion.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum FieldValue {
    /// An unsigned 64-bit integer.
    UInt(u64),
    /// A boolean.
    Bool(bool),
    /// A raw string.
    Str(String),
}

/// A single parsed field of a [`Record`].
///
/// A field corresponds to one capture group of the rule that matched the
/// line. When the group is optional and did not participate in the match,
/// both the value and the span are `None`. When the group participated
/// but captured the empty string, the value is `Some` (e.g. an empty
/// string for [`FieldType::Str`]) and the span is an empty range.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Field {
    value: Option<FieldValue>,
    span: Option<Range<usize>>,
}

impl Field {
    /// Returns the converted value, or `None` when the capture group did
    /// not participate in the match.
    pub fn value(&self) -> Option<&FieldValue> {
        self.value.as_ref()
    }

    /// Returns the byte range of the captured text within the original
    /// line, or `None` when the capture group did not participate in the
    /// match.
    pub fn span(&self) -> Option<Range<usize>> {
        self.span.clone()
    }

    /// Returns true when the capture group did not participate in the
    /// match. This is distinct from a group that captured the empty
    /// string, which yields `Some` value with an empty span.
    pub fn is_missing(&self) -> bool {
        self.value.is_none()
    }
}

/// A record parsed from a single line.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Record {
    rule: usize,
    fields: Vec<Field>,
}

impl Record {
    /// Returns the index of the rule that produced this record. Indices
    /// follow the order in which rules were added to the builder.
    pub fn rule(&self) -> usize {
        self.rule
    }

    /// Returns all fields of this record, in capture group order.
    pub fn fields(&self) -> &[Field] {
        &self.fields
    }

    /// Returns the field at the given index, or `None` when the index is
    /// out of bounds.
    pub fn get(&self, index: usize) -> Option<&Field> {
        self.fields.get(index)
    }

    /// Returns the number of fields in this record.
    pub fn len(&self) -> usize {
        self.fields.len()
    }

    /// Returns true when this record has no fields.
    pub fn is_empty(&self) -> bool {
        self.fields.is_empty()
    }

    /// Returns an iterator over the fields of this record.
    pub fn iter(&self) -> core::slice::Iter<'_, Field> {
        self.fields.iter()
    }
}

/// An error that occurred while parsing a single line.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum LineError {
    /// No rule fully matched the line.
    NoMatch,
    /// A rule fully matched the line, but converting one of its capture
    /// groups to the requested type failed. Later rules are not tried;
    /// this error is final for the line.
    Convert(ConvertError),
}

impl LineError {
    /// Returns the conversion error when this is a [`LineError::Convert`],
    /// or `None` otherwise.
    pub fn convert_error(&self) -> Option<&ConvertError> {
        match *self {
            LineError::NoMatch => None,
            LineError::Convert(ref err) => Some(err),
        }
    }
}

impl fmt::Display for LineError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            LineError::NoMatch => write!(f, "no rule matched the line"),
            LineError::Convert(ref err) => err.fmt(f),
        }
    }
}

#[cfg(feature = "std")]
impl std::error::Error for LineError {}

/// A field conversion failure for a line that fully matched a rule.
///
/// This reports which rule and which capture group failed, the byte range
/// of the captured text within the original line and the captured text
/// itself.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ConvertError {
    rule: usize,
    field: usize,
    span: Range<usize>,
    text: String,
    expected: FieldType,
}

impl ConvertError {
    /// Returns the index of the rule whose conversion failed.
    pub fn rule(&self) -> usize {
        self.rule
    }

    /// Returns the index of the field (capture group) whose conversion
    /// failed, in the order the field types were declared for the rule.
    pub fn field(&self) -> usize {
        self.field
    }

    /// Returns the byte range of the captured text within the original
    /// line.
    pub fn span(&self) -> Range<usize> {
        self.span.clone()
    }

    /// Returns the captured text that failed to convert.
    pub fn text(&self) -> &str {
        &self.text
    }

    /// Returns the type the captured text failed to convert to.
    pub fn expected(&self) -> FieldType {
        self.expected
    }
}

impl fmt::Display for ConvertError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "rule {}: failed to convert field {} ({}..{}) {:?} to {}",
            self.rule,
            self.field,
            self.span.start,
            self.span.end,
            self.text,
            self.expected,
        )
    }
}

#[cfg(feature = "std")]
impl std::error::Error for ConvertError {}

/// An error that occurred while building a [`RecordParser`].
#[derive(Clone, Debug)]
pub enum BuildError {
    /// A regex of a rule failed to compile.
    Regex(Error),
    /// A rule declares more field types than its regex has capture groups.
    FieldCount {
        /// The index of the offending rule.
        rule: usize,
        /// The number of capture groups in the regex (excluding group 0).
        groups: usize,
        /// The number of field types declared for the rule.
        fields: usize,
    },
}

impl fmt::Display for BuildError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            BuildError::Regex(ref err) => err.fmt(f),
            BuildError::FieldCount { rule, groups, fields } => write!(
                f,
                "rule {}: {} field types declared but the regex only has \
                 {} capture groups",
                rule, fields, groups,
            ),
        }
    }
}

#[cfg(feature = "std")]
impl std::error::Error for BuildError {}

#[derive(Clone, Debug)]
struct Rule {
    regex: Regex,
    fields: Vec<FieldType>,
}

/// A parser that converts lines into typed [`Record`]s using an ordered
/// list of rules.
///
/// See the [module documentation](self) for details and an example.
#[derive(Clone, Debug)]
pub struct RecordParser {
    rules: Vec<Rule>,
}

impl RecordParser {
    /// Returns a new builder for a [`RecordParser`].
    pub fn builder() -> RecordParserBuilder {
        RecordParserBuilder::new()
    }

    /// Parses a single line into a [`Record`].
    ///
    /// Rules are tried in priority order. A rule applies only when its
    /// overall match spans the entire line. The first such rule is used
    /// to produce the record; if any of its field conversions fail, a
    /// [`LineError::Convert`] is returned and no later rule is tried.
    /// When no rule fully matches, [`LineError::NoMatch`] is returned.
    pub fn parse_line(&self, line: &str) -> Result<Record, LineError> {
        for (rule_index, rule) in self.rules.iter().enumerate() {
            let caps = match rule.regex.captures(line) {
                Some(caps) => caps,
                None => continue,
            };
            let whole = caps.get(0).expect("group 0 always participates");
            if whole.start() != 0 || whole.end() != line.len() {
                // The regex matched, but not the entire line. This rule
                // does not apply; keep looking.
                continue;
            }
            // This rule fully matches: commit to it. Conversion failures
            // are reported and never fall through to later rules.
            let mut fields = Vec::with_capacity(rule.fields.len());
            for (field_index, &expected) in rule.fields.iter().enumerate() {
                let group = caps.get(field_index + 1);
                let m = match group {
                    None => {
                        fields.push(Field { value: None, span: None });
                        continue;
                    }
                    Some(m) => m,
                };
                let text = m.as_str();
                let value = match convert(expected, text) {
                    Some(value) => value,
                    None => {
                        return Err(LineError::Convert(ConvertError {
                            rule: rule_index,
                            field: field_index,
                            span: m.start()..m.end(),
                            text: text.to_string(),
                            expected,
                        }));
                    }
                };
                fields.push(Field {
                    value: Some(value),
                    span: Some(m.start()..m.end()),
                });
            }
            return Ok(Record { rule: rule_index, fields });
        }
        Err(LineError::NoMatch)
    }

    /// Returns an iterator that parses each line of the given text.
    ///
    /// The iterator yields the 0-based line number along with the parse
    /// result for that line. Parsing continues line by line regardless of
    /// errors on earlier lines.
    pub fn parse_lines<'p, 't>(&'p self, text: &'t str) -> ParseLines<'p, 't> {
        ParseLines { parser: self, lines: text.lines(), number: 0 }
    }
}

fn convert(expected: FieldType, text: &str) -> Option<FieldValue> {
    match expected {
        FieldType::UInt => text.parse::<u64>().ok().map(FieldValue::UInt),
        FieldType::Bool => match text {
            "true" => Some(FieldValue::Bool(true)),
            "false" => Some(FieldValue::Bool(false)),
            _ => None,
        },
        FieldType::Str => Some(FieldValue::Str(text.to_string())),
    }
}

/// An iterator over the parse results of each line in a text.
///
/// This is created by [`RecordParser::parse_lines`]. Each item is the
/// 0-based line number and the parse result for that line.
#[derive(Debug)]
pub struct ParseLines<'p, 't> {
    parser: &'p RecordParser,
    lines: core::str::Lines<'t>,
    number: usize,
}

impl<'p, 't> Iterator for ParseLines<'p, 't> {
    type Item = (usize, Result<Record, LineError>);

    fn next(&mut self) -> Option<Self::Item> {
        let line = self.lines.next()?;
        let number = self.number;
        self.number += 1;
        Some((number, self.parser.parse_line(line)))
    }
}

/// A builder for a [`RecordParser`].
///
/// Rules are added in priority order with [`RecordParserBuilder::rule`].
#[derive(Clone, Debug, Default)]
pub struct RecordParserBuilder {
    rules: Vec<(String, Vec<FieldType>)>,
}

impl RecordParserBuilder {
    /// Returns a new, empty builder.
    pub fn new() -> RecordParserBuilder {
        RecordParserBuilder::default()
    }

    /// Adds a rule to the parser.
    ///
    /// `pattern` is a regular expression that must match the entire line
    /// for the rule to apply. `fields` declares the conversion applied
    /// to each capture group, in order: `fields[0]` applies to capture
    /// group 1, `fields[1]` to group 2, and so on.
    ///
    /// Rules are tried in the order they are added; the first rule whose
    /// overall match spans the entire line is the only rule used for that
    /// line.
    pub fn rule(
        &mut self,
        pattern: &str,
        fields: &[FieldType],
    ) -> &mut RecordParserBuilder {
        self.rules.push((pattern.to_string(), fields.to_vec()));
        self
    }

    /// Builds the [`RecordParser`].
    ///
    /// This returns an error when the regex of a rule fails to compile,
    /// or when a rule declares more field types than its regex has
    /// capture groups.
    pub fn build(&self) -> Result<RecordParser, BuildError> {
        let mut rules = Vec::with_capacity(self.rules.len());
        for (rule_index, (pattern, fields)) in self.rules.iter().enumerate() {
            let regex = Regex::new(pattern).map_err(BuildError::Regex)?;
            let groups = regex.captures_len() - 1;
            if fields.len() > groups {
                return Err(BuildError::FieldCount {
                    rule: rule_index,
                    groups,
                    fields: fields.len(),
                });
            }
            rules.push(Rule { regex, fields: fields.clone() });
        }
        Ok(RecordParser { rules })
    }
}
