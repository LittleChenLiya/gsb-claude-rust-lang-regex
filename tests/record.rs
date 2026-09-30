use regex::record::{
    FieldKind, FieldSpec, FieldValue, ParseError, RecordParser, RecordRule,
};

fn id_ok_parser() -> RecordParser {
    RecordParser::new(vec![
        RecordRule::new(
            r"id=(\d+) ok=(\w+)",
            vec![
                FieldSpec::group(1, FieldKind::U64),
                FieldSpec::group(2, FieldKind::Bool),
            ],
        ),
        // A broad fallback rule that would match anything. It must never
        // mask a conversion failure from the higher priority rule.
        RecordRule::new(r".*", vec![FieldSpec::group(0, FieldKind::Str)]),
    ])
    .unwrap()
}

#[test]
fn parse_typed_fields() {
    let parser = id_ok_parser();
    let record = parser.parse_line("id=12 ok=true").unwrap();
    assert_eq!(record.rule, 0);
    assert_eq!(
        record.fields,
        vec![FieldValue::U64(12), FieldValue::Bool(true)]
    );
}

#[test]
fn fallback_rule_used_when_first_does_not_match() {
    let parser = id_ok_parser();
    let record = parser.parse_line("something else entirely").unwrap();
    assert_eq!(record.rule, 1);
    assert_eq!(
        record.fields,
        vec![FieldValue::Str("something else entirely".to_string())]
    );
}

#[test]
fn u64_overflow_is_convert_error_not_no_match() {
    let parser = id_ok_parser();
    // One more than u64::MAX.
    let line = "id=18446744073709551616 ok=true";
    let err = parser.parse_line(line).unwrap_err();
    let ParseError::Convert(err) = err else {
        panic!("expected convert error, got: {:?}", err)
    };
    assert_eq!(err.rule(), 0);
    assert_eq!(err.group(), 1);
    assert_eq!(err.kind(), FieldKind::U64);
    assert_eq!(err.text(), "18446744073709551616");
    // The reported span must line up with the original line.
    let span = err.span();
    assert_eq!(&line[span.start..span.end], "18446744073709551616");
}

#[test]
fn fallback_rule_cannot_mask_convert_error() {
    // Same as above, but the point here is that the fallback rule (which
    // matches every line) must not swallow the conversion failure.
    let parser = id_ok_parser();
    let line = "id=99999999999999999999999999 ok=false";
    let err = parser.parse_line(line).unwrap_err();
    assert!(matches!(err, ParseError::Convert(_)));
}

#[test]
fn convert_error_span_with_unicode_prefix() {
    let parser = RecordParser::new(vec![RecordRule::new(
        r"name=(\S+) id=(\d+)",
        vec![
            FieldSpec::group(1, FieldKind::Str),
            FieldSpec::group(2, FieldKind::U64),
        ],
    )])
    .unwrap();
    // Multi-byte characters before the capture group shift its byte
    // offsets, and the reported span must still index the original line.
    let line = "name=\u{65e5}\u{672c}\u{8a9e} id=18446744073709551616";
    let err = parser.parse_line(line).unwrap_err();
    let ParseError::Convert(err) = err else {
        panic!("expected convert error, got: {:?}", err)
    };
    let span = err.span();
    assert_eq!(&line[span.start..span.end], "18446744073709551616");
    assert_eq!(span.start, line.find("18446744073709551616").unwrap());

    // And the same parser handles a valid line with a unicode prefix.
    let record =
        parser.parse_line("name=\u{65e5}\u{672c}\u{8a9e} id=7").unwrap();
    assert_eq!(
        record.fields,
        vec![
            FieldValue::Str("\u{65e5}\u{672c}\u{8a9e}".to_string()),
            FieldValue::U64(7)
        ]
    );
}

#[test]
fn optional_group_missing_vs_empty_capture() {
    let parser = RecordParser::new(vec![RecordRule::new(
        r"key=(\w+)(?: value=(\S*))? tag=(.*)",
        vec![
            FieldSpec::group(1, FieldKind::Str),
            FieldSpec::group(2, FieldKind::Str),
            FieldSpec::group(3, FieldKind::Str),
        ],
    )])
    .unwrap();

    // The optional middle group does not participate at all.
    let record = parser.parse_line("key=a tag=x").unwrap();
    assert_eq!(
        record.fields,
        vec![
            FieldValue::Str("a".to_string()),
            FieldValue::Missing,
            FieldValue::Str("x".to_string()),
        ]
    );

    // The optional group participates but matches the empty string.
    let record = parser.parse_line("key=a value= tag=x").unwrap();
    assert_eq!(
        record.fields,
        vec![
            FieldValue::Str("a".to_string()),
            FieldValue::Str("".to_string()),
            FieldValue::Str("x".to_string()),
        ]
    );

    // A trailing group that matches the empty string is also not
    // "missing".
    let record = parser.parse_line("key=a tag=").unwrap();
    assert_eq!(
        record.fields,
        vec![
            FieldValue::Str("a".to_string()),
            FieldValue::Missing,
            FieldValue::Str("".to_string()),
        ]
    );
}

#[test]
fn first_matching_rule_shadows_later_rules() {
    let parser = RecordParser::new(vec![
        RecordRule::new(r"n=(\d+)", vec![FieldSpec::group(1, FieldKind::U64)]),
        RecordRule::new(r"n=(\d+)", vec![FieldSpec::group(1, FieldKind::Str)]),
    ])
    .unwrap();
    let record = parser.parse_line("n=5").unwrap();
    assert_eq!(record.rule, 0);
    assert_eq!(record.fields, vec![FieldValue::U64(5)]);
}

#[test]
fn no_rule_matching_is_no_match_error() {
    let parser = RecordParser::new(vec![RecordRule::new(
        r"id=(\d+)",
        vec![FieldSpec::group(1, FieldKind::U64)],
    )])
    .unwrap();
    let err = parser.parse_line("nothing here").unwrap_err();
    let ParseError::NoMatch(err) = err else {
        panic!("expected no-match error, got: {:?}", err)
    };
    assert_eq!(err.line(), "nothing here");
}

#[test]
fn rules_must_match_the_entire_line() {
    let parser = RecordParser::new(vec![RecordRule::new(
        r"id=(\d+)",
        vec![FieldSpec::group(1, FieldKind::U64)],
    )])
    .unwrap();
    // The pattern matches a substring, but not the whole line.
    let err = parser.parse_line("id=1 trailing").unwrap_err();
    assert!(matches!(err, ParseError::NoMatch(_)));
}

#[test]
fn parse_lines_continues_after_errors() {
    let parser = id_ok_parser();
    let lines = vec![
        "id=1 ok=true",
        "id=18446744073709551616 ok=true",
        "id=2 ok=false",
    ];
    let results = parser.parse_lines(lines);
    assert_eq!(results.len(), 3);
    assert_eq!(
        results[0].as_ref().unwrap().fields,
        vec![FieldValue::U64(1), FieldValue::Bool(true),]
    );
    assert!(matches!(results[1], Err(ParseError::Convert(_))));
    // Parsing continues after the failed line.
    assert_eq!(
        results[2].as_ref().unwrap().fields,
        vec![FieldValue::U64(2), FieldValue::Bool(false),]
    );
}

#[test]
fn named_capture_groups() {
    let parser = RecordParser::new(vec![RecordRule::new(
        r"id=(?P<id>\d+) ok=(?P<ok>\w+)",
        vec![
            FieldSpec::named("id", FieldKind::U64),
            FieldSpec::named("ok", FieldKind::Bool),
        ],
    )])
    .unwrap();
    let record = parser.parse_line("id=3 ok=false").unwrap();
    assert_eq!(
        record.fields,
        vec![FieldValue::U64(3), FieldValue::Bool(false)]
    );
}

#[test]
fn builder_rejects_bad_rules() {
    // A pattern that does not compile.
    let result = RecordParser::new(vec![RecordRule::new("(", vec![])]);
    assert!(result.is_err());

    // A capture group index that does not exist.
    let result = RecordParser::new(vec![RecordRule::new(
        r"id=(\d+)",
        vec![FieldSpec::group(2, FieldKind::U64)],
    )]);
    assert!(result.is_err());

    // A capture group name that does not exist.
    let result = RecordParser::new(vec![RecordRule::new(
        r"id=(\d+)",
        vec![FieldSpec::named("nope", FieldKind::U64)],
    )]);
    assert!(result.is_err());
}
