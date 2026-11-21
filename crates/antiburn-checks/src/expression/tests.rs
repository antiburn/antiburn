use super::*;

#[test]
fn fixed_access_and_parenthesized_literals_keep_the_static_visitor() {
    let expression = parse_expression("message[('tool')]['arguments'][0]").unwrap();
    assert_eq!(
        expression,
        Expression::Path(vec![
            PathSegment::Property("message".to_owned()),
            PathSegment::Property("tool".to_owned()),
            PathSegment::Property("arguments".to_owned()),
            PathSegment::Index(0)
        ])
    );
}

#[test]
fn visitors_expose_every_dynamic_projection_and_call_reference() {
    let expression =
        parse_expression("contains(message.tool.arguments[session.agent].*.value, repo.name)")
            .unwrap();
    let mut paths = Vec::new();
    expression.visit_paths(&mut |path| paths.push(path.to_vec()));
    assert_eq!(paths.len(), 3);
    assert_eq!(
        paths[1],
        vec![
            PathSegment::Property("session".to_owned()),
            PathSegment::Property("agent".to_owned())
        ]
    );
    let mut chains = Vec::new();
    expression.visit_accesses(&mut |base, segments| chains.push((base.clone(), segments.to_vec())));
    assert_eq!(chains.len(), 1);
    assert!(
        matches!(chains[0].1.as_slice(), [AccessSegment::Dynamic(_), AccessSegment::Projection, AccessSegment::Property(name)] if name == "value")
    );
    let expression = parse_expression("fromJSON(message.tool.result.output).field").unwrap();
    let mut paths = Vec::new();
    expression.visit_paths(&mut |path| paths.push(path.to_vec()));
    assert_eq!(paths.len(), 1);
}

#[test]
fn invalid_calls_arity_and_unsupported_syntax_remain_rejected() {
    for source in [
        "unknown(1)",
        "hashFiles('x')",
        "contains('x')",
        "contains('x', 'x', 'x')",
        "strictEquals()",
        "join()",
        "join('a', ',', 1)",
        "format('x')",
        "fromJSON()",
        "toJSON(1,2)",
        "format('{0}',)",
        "data.method()",
        "fromJSON('{}')()",
        "data[0:1]",
        "data[]",
        "data.1",
        "\"text\"",
        "1 + 2",
        "0xff",
        "1e400",
        "-1e400",
    ] {
        assert!(parse_expression(source).is_err(), "{source}");
    }
}

#[test]
fn numeric_literals_keep_finite_f64_and_integer_values() {
    for (source, expected) in [
        ("1.25", Number::from_f64(1.25).unwrap()),
        ("2e2", Number::from_f64(200.0).unwrap()),
        ("1.00000000000000001", Number::from_f64(1.0).unwrap()),
        ("-0", Number::from_f64(-0.0).unwrap()),
        ("-0.0", Number::from_f64(-0.0).unwrap()),
        ("9007199254740993", Number::from(9_007_199_254_740_993_u64)),
        ("18446744073709551615", Number::from(u64::MAX)),
        ("-9223372036854775808", Number::from(i64::MIN)),
        (
            "18446744073709551616",
            Number::from_f64(18_446_744_073_709_551_616.0).unwrap(),
        ),
    ] {
        assert_eq!(
            parse_expression(source).unwrap(),
            Expression::Literal(ExpressionLiteral::Number(expected)),
            "{source}"
        );
    }
}

#[test]
fn call_access_operators_and_doubled_quotes_parse_together() {
    assert!(
        parse_expression(
            "!strictEquals(fromJSON('{\"x\":true}').x, false) && 'it''s' != null || false"
        )
        .is_ok()
    );
    assert!(parse_expression("format('{0}', startsWith('a', 'A'))").is_ok());
}
