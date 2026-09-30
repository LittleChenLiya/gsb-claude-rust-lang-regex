use regex::record::{
    BuildError, FieldType, FieldValue, LineError, RecordParser,
};

fn kv_parser() -> RecordParser {
    RecordParser::builder()
        .rule(r"^id=(\d+) ok=(\w+)$", &[FieldType::UInt, FieldType::Bool])
        .build()
        .unwrap()
}

#[test]
fn typed_fields() {
    let parser = kv_parser();
    let line = "id=12 ok=true";
    let record = parser.parse_line(line).unwrap();
    assert_eq!(record.rule(), 0);
    assert_eq!(record.len(), 2);

    let id = record.get(0).unwrap();
    assert_eq!(id.value(), Some(&FieldValue::UInt(12)));
    let id_span = id.span().unwrap();
    assert_eq!(&line[id_span.clone()], "12");
    assert_eq!(id_span, 3..5);

    let ok = record.get(1).unwrap();
    assert_eq!(ok.value(), Some(&FieldValue::Bool(true)));
    let ok_span = ok.span().unwrap();
    assert_eq!(&line[ok_span.clone()], "true");
    assert_eq!(ok_span, 9..13);
}

#[test]
fn u64_boundary() {
    let parser = kv_parser();
    let line = "id=18446744073709551615 ok=false";
    let record = parser.parse_line(line).unwrap();
    assert_eq!(
        record.get(0).unwrap().value(),
        Some(&FieldValue::UInt(u64::MAX))
    );
    assert_eq!(record.get(1).unwrap().value(), Some(&FieldValue::Bool(false)));

    let line = "id=18446744073709551616 ok=false";
    let err = parser.parse_line(line).unwrap_err();
    let err = err.convert_error().unwrap();
    assert_eq!(err.expected(), FieldType::UInt);
    assert_eq!(&line[err.span()], "18446744073709551616");
}

#[test]
fn overflow_is_convert_error_not_masked_by_fallback() {
    let parser = RecordParser::builder()
        .rule(r"^id=(\d+) ok=(\w+)$", &[FieldType::UInt, FieldType::Bool])
        .rule(r"^(.*)$", &[FieldType::Str])
        .build()
        .unwrap();
    let line = "id=99999999999999999999999999 ok=true";
    let err = parser.parse_line(line).unwrap_err();
    match err {
        LineError::NoMatch => panic!("expected convert error, got no match"),
        LineError::Convert(ref err) => {
            assert_eq!(err.rule(), 0);
            assert_eq!(err.field(), 0);
            assert_eq!(err.expected(), FieldType::UInt);
            assert_eq!(err.text(), "99999999999999999999999999");
            // The reported range must line up with the original line.
            assert_eq!(&line[err.span()], "99999999999999999999999999");
            assert_eq!(err.span(), 3..29);
        }
    }
}

#[test]
fn fallback_catches_otherwise_unmatched_lines() {
    let parser = RecordParser::builder()
        .rule(r"^id=(\d+)$", &[FieldType::UInt])
        .rule(r"^(.*)$", &[FieldType::Str])
        .build()
        .unwrap();
    let record = parser.parse_line("hello").unwrap();
    assert_eq!(record.rule(), 1);
    assert_eq!(
        record.get(0).unwrap().value(),
        Some(&FieldValue::Str("hello".to_string())),
    );
}

#[test]
fn unicode_prefix_spans() {
    let parser = RecordParser::builder()
        .rule(r"^.+ id=(\d+)$", &[FieldType::UInt])
        .build()
        .unwrap();
    let line = "\u{524d}\u{7f00} id=7";
    let record = parser.parse_line(line).unwrap();
    let id = record.get(0).unwrap();
    assert_eq!(id.value(), Some(&FieldValue::UInt(7)));
    let span = id.span().unwrap();
    assert_eq!(&line[span.clone()], "7");
    assert_eq!(span, line.len() - 1..line.len());
    assert_eq!(span, line.find("7").unwrap()..line.len());
}

#[test]
fn optional_group_missing_vs_empty_capture() {
    let parser = RecordParser::builder()
        .rule(r"^a=(\d+)? b=(.*)$", &[FieldType::UInt, FieldType::Str])
        .build()
        .unwrap();
    let line = "a= b=";
    let record = parser.parse_line(line).unwrap();

    // The optional group did not participate: missing, not empty.
    let a = record.get(0).unwrap();
    assert!(a.is_missing());
    assert_eq!(a.value(), None);
    assert_eq!(a.span(), None);

    // The second group participated but captured the empty string.
    let b = record.get(1).unwrap();
    assert!(!b.is_missing());
    assert_eq!(b.value(), Some(&FieldValue::Str(String::new())));
    let span = b.span().unwrap();
    assert_eq!(span, 5..5);
    assert_eq!(&line[span], "");

    // When the optional group participates, it is a normal value.
    let record = parser.parse_line("a=3 b=x").unwrap();
    assert_eq!(record.get(0).unwrap().value(), Some(&FieldValue::UInt(3)));
}

#[test]
fn empty_capture_can_fail_conversion() {
    let parser = RecordParser::builder()
        .rule(r"^n=(\d*)$", &[FieldType::UInt])
        .build()
        .unwrap();
    let line = "n=";
    let err = parser.parse_line(line).unwrap_err();
    let err = err.convert_error().unwrap();
    assert_eq!(err.text(), "");
    assert_eq!(err.span(), 2..2);
    assert_eq!(&line[err.span()], "");
}

#[test]
fn invalid_bool_is_convert_error() {
    let parser = kv_parser();
    let line = "id=1 ok=yes";
    let err = parser.parse_line(line).unwrap_err();
    let err = err.convert_error().unwrap();
    assert_eq!(err.rule(), 0);
    assert_eq!(err.field(), 1);
    assert_eq!(err.expected(), FieldType::Bool);
    assert_eq!(&line[err.span()], "yes");
}

#[test]
fn rule_shadowing_first_full_match_wins() {
    let parser = RecordParser::builder()
        .rule(r"^id=(\d+)$", &[FieldType::UInt])
        .rule(r"^id=(\w+)$", &[FieldType::Str])
        .build()
        .unwrap();
    // Both rules could match, but the first one wins.
    let record = parser.parse_line("id=5").unwrap();
    assert_eq!(record.rule(), 0);
    assert_eq!(record.get(0).unwrap().value(), Some(&FieldValue::UInt(5)));
    // The first rule does not match here, so the second one is used.
    let record = parser.parse_line("id=abc").unwrap();
    assert_eq!(record.rule(), 1);
    assert_eq!(
        record.get(0).unwrap().value(),
        Some(&FieldValue::Str("abc".to_string())),
    );
}

#[test]
fn rules_must_match_the_entire_line() {
    let parser = RecordParser::builder()
        .rule(r"id=(\d+)", &[FieldType::UInt])
        .build()
        .unwrap();
    // Unanchored, but happens to cover the whole line.
    let record = parser.parse_line("id=5").unwrap();
    assert_eq!(record.get(0).unwrap().value(), Some(&FieldValue::UInt(5)));
    // A partial match is not enough: the rule does not apply.
    assert_eq!(parser.parse_line("id=5 ").unwrap_err(), LineError::NoMatch);
    assert_eq!(parser.parse_line("x id=5").unwrap_err(), LineError::NoMatch);
}

#[test]
fn no_rule_matched() {
    let parser = kv_parser();
    assert_eq!(parser.parse_line("nope").unwrap_err(), LineError::NoMatch);
    assert_eq!(parser.parse_line("").unwrap_err(), LineError::NoMatch);
}

#[test]
fn batch_lines_continue_after_errors() {
    let parser = RecordParser::builder()
        .rule(r"^id=(\d+) ok=(\w+)$", &[FieldType::UInt, FieldType::Bool])
        .rule(r"^(.*)$", &[FieldType::Str])
        .build()
        .unwrap();
    let text = "\
id=1 ok=true\n\
id=99999999999999999999999999 ok=true\n\
\n\
id=3 ok=false\n";
    let results: Vec<(usize, Result<_, _>)> =
        parser.parse_lines(text).collect();
    assert_eq!(results.len(), 4);

    assert_eq!(results[0].0, 0);
    let record = results[0].1.as_ref().unwrap();
    assert_eq!(record.rule(), 0);
    assert_eq!(record.get(0).unwrap().value(), Some(&FieldValue::UInt(1)));

    // The overflow error is reported and does not stop the batch.
    assert_eq!(results[1].0, 1);
    let err = results[1].1.as_ref().unwrap_err();
    let err = err.convert_error().unwrap();
    assert_eq!(err.rule(), 0);
    assert_eq!(err.text(), "99999999999999999999999999");

    // The empty line falls through to the fallback rule.
    assert_eq!(results[2].0, 2);
    let record = results[2].1.as_ref().unwrap();
    assert_eq!(record.rule(), 1);
    assert_eq!(
        record.get(0).unwrap().value(),
        Some(&FieldValue::Str(String::new()))
    );

    // Parsing continues after the error.
    assert_eq!(results[3].0, 3);
    let record = results[3].1.as_ref().unwrap();
    assert_eq!(record.get(0).unwrap().value(), Some(&FieldValue::UInt(3)));
    assert_eq!(record.get(1).unwrap().value(), Some(&FieldValue::Bool(false)));
}

#[test]
fn build_errors() {
    let err = RecordParser::builder()
        .rule(r"^id=(\d+$", &[FieldType::UInt])
        .build()
        .unwrap_err();
    assert!(matches!(err, BuildError::Regex(_)));

    let err = RecordParser::builder()
        .rule(r"^id=(\d+)$", &[FieldType::UInt, FieldType::Bool])
        .build()
        .unwrap_err();
    match err {
        BuildError::FieldCount { rule, groups, fields } => {
            assert_eq!(rule, 0);
            assert_eq!(groups, 1);
            assert_eq!(fields, 2);
        }
        err => panic!("expected field count error, got {:?}", err),
    }
}
