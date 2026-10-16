//! YAML 1.2 parsing into a span-carrying JSON-compatible value tree.

use std::collections::HashSet;

use saphyr_parser::{Event, Parser, ScalarStyle, Span as ParserSpan, Tag};
use serde_json::Number;

use crate::diagnostics::{Diagnostic, DiagnosticCode, SourceSpan};

const MAX_YAML_DEPTH: usize = 128;

/// A parsed YAML value with the source range for its authored node.
#[derive(Clone, Debug, PartialEq)]
pub struct ConfigValue {
    /// The JSON-compatible value kind.
    pub kind: ConfigValueKind,
    /// The source range occupied by this value.
    pub span: SourceSpan,
}

/// The supported JSON-compatible YAML value kinds.
#[derive(Clone, Debug, PartialEq)]
pub enum ConfigValueKind {
    /// A YAML null value.
    Null,
    /// A YAML Boolean value.
    Boolean(bool),
    /// A finite JSON number.
    Number(Number),
    /// A YAML string.
    String(String),
    /// An ordered YAML sequence.
    Sequence(Vec<ConfigValue>),
    /// An ordered YAML mapping with string keys.
    Mapping(Vec<ConfigEntry>),
}

/// One string-keyed mapping member and its key source range.
#[derive(Clone, Debug, PartialEq)]
pub struct ConfigEntry {
    /// The decoded string key.
    pub key: String,
    /// The source range occupied by the key.
    pub key_span: SourceSpan,
    /// The value associated with the key.
    pub value: ConfigValue,
}

/// One accepted YAML document.
#[derive(Clone, Debug, PartialEq)]
pub struct YamlDocument {
    root: ConfigValue,
}

impl YamlDocument {
    /// Returns the parsed root mapping.
    #[must_use]
    pub const fn root(&self) -> &ConfigValue {
        &self.root
    }
}

impl ConfigValue {
    /// Converts this value to a JSON value and drops its source metadata.
    ///
    /// Parsing guarantees that every number is finite and every object key is
    /// unique, so this conversion cannot fail. Use the [`ConfigValue`] tree
    /// directly when diagnostics or source-aware tooling needs node spans.
    #[must_use]
    pub fn to_json_value(&self) -> serde_json::Value {
        match &self.kind {
            ConfigValueKind::Null => serde_json::Value::Null,
            ConfigValueKind::Boolean(value) => serde_json::Value::Bool(*value),
            ConfigValueKind::Number(value) => serde_json::Value::Number(value.clone()),
            ConfigValueKind::String(value) => serde_json::Value::String(value.clone()),
            ConfigValueKind::Sequence(items) => {
                serde_json::Value::Array(items.iter().map(Self::to_json_value).collect())
            }
            ConfigValueKind::Mapping(entries) => serde_json::Value::Object(
                entries
                    .iter()
                    .map(|entry| (entry.key.clone(), entry.value.to_json_value()))
                    .collect(),
            ),
        }
    }

    /// Returns the mapping members when this value is a mapping.
    #[must_use]
    pub fn as_mapping(&self) -> Option<&[ConfigEntry]> {
        match &self.kind {
            ConfigValueKind::Mapping(entries) => Some(entries),
            _ => None,
        }
    }

    /// Returns the sequence items when this value is a sequence.
    #[must_use]
    pub fn as_sequence(&self) -> Option<&[ConfigValue]> {
        match &self.kind {
            ConfigValueKind::Sequence(items) => Some(items),
            _ => None,
        }
    }

    /// Returns the string contents when this value is a string.
    #[must_use]
    pub fn as_str(&self) -> Option<&str> {
        match &self.kind {
            ConfigValueKind::String(value) => Some(value),
            _ => None,
        }
    }
}

#[derive(Clone, Debug)]
struct EventItem<'source> {
    event: Event<'source>,
    span: SourceSpan,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ExplicitTag {
    NonSpecific,
    String,
    Null,
    Boolean,
    Integer,
    Float,
    Mapping,
    Sequence,
}

/// Parses one YAML 1.2 mapping document and rejects YAML features outside the
/// Burn Check JSON-compatible data model.
///
/// The parser preserves spans, rejects duplicate and non-string keys, and does
/// not expand aliases or merge keys. Plain scalars use YAML 1.2 core resolution;
/// quoted scalars remain strings unless an accepted standard tag selects a type.
///
/// # Errors
/// Returns one diagnostic when YAML syntax or the restricted data model fails.
pub fn parse_yaml(source: &str) -> Result<YamlDocument, Vec<Diagnostic>> {
    if let Some((offset, line)) = find_directive(source) {
        return Err(vec![Diagnostic::at(
            DiagnosticCode::YamlDirective,
            "YAML directives are not allowed in check files.",
            SourceSpan::point(offset, line, 1),
        )]);
    }

    let mut parser = Parser::new_from_str(source);
    let mut events = Vec::new();
    while let Some(result) = parser.next_event() {
        match result {
            Ok((event, span)) => events.push(EventItem {
                event,
                span: source_span(span),
            }),
            Err(error) => {
                let marker = error.marker();
                let code = if error.info().contains("unknown anchor") {
                    DiagnosticCode::YamlAlias
                } else {
                    DiagnosticCode::YamlSyntax
                };
                return Err(vec![Diagnostic::at(
                    code,
                    error.info(),
                    SourceSpan::point(
                        marker.index(),
                        marker.line(),
                        marker.col().saturating_add(1),
                    ),
                )]);
            }
        }
    }

    let mut cursor = 0;
    while matches!(
        events.get(cursor).map(|item| &item.event),
        Some(Event::StreamStart)
    ) {
        cursor += 1;
    }
    let Some(document_start) = events.get(cursor) else {
        return Err(vec![Diagnostic::new(
            DiagnosticCode::YamlDocumentCount,
            "Expected exactly one YAML document.",
        )]);
    };
    if matches!(document_start.event, Event::StreamEnd) {
        return Err(vec![Diagnostic::at(
            DiagnosticCode::YamlRootType,
            "The YAML document root must be a mapping.",
            document_start.span,
        )]);
    }
    if !matches!(document_start.event, Event::DocumentStart(_)) {
        return Err(vec![Diagnostic::at(
            DiagnosticCode::YamlSyntax,
            "Expected a YAML document start.",
            document_start.span,
        )]);
    }
    cursor += 1;

    let root = if let Some(item) = events.get(cursor) {
        if matches!(item.event, Event::DocumentEnd) {
            ConfigValue {
                kind: ConfigValueKind::Null,
                span: item.span,
            }
        } else {
            parse_node(&events, &mut cursor, 0)?
        }
    } else {
        return Err(vec![Diagnostic::new(
            DiagnosticCode::YamlSyntax,
            "Unexpected end of YAML input.",
        )]);
    };
    if !matches!(
        events.get(cursor).map(|item| &item.event),
        Some(Event::DocumentEnd)
    ) {
        return Err(vec![diagnostic_for_event(
            events.get(cursor),
            DiagnosticCode::YamlSyntax,
            "Expected the end of the YAML document.",
        )]);
    }
    cursor += 1;

    if let Some(item) = events.get(cursor) {
        if matches!(item.event, Event::DocumentStart(_)) {
            return Err(vec![Diagnostic::at(
                DiagnosticCode::YamlDocumentCount,
                "Expected exactly one YAML document.",
                item.span,
            )]);
        }
        if !matches!(item.event, Event::StreamEnd) {
            return Err(vec![Diagnostic::at(
                DiagnosticCode::YamlSyntax,
                "Unexpected content after the YAML document.",
                item.span,
            )]);
        }
    }

    if !matches!(root.kind, ConfigValueKind::Mapping(_)) {
        return Err(vec![Diagnostic::at(
            DiagnosticCode::YamlRootType,
            "The YAML document root must be a mapping.",
            root.span,
        )]);
    }

    Ok(YamlDocument { root })
}

fn find_directive(source: &str) -> Option<(usize, usize)> {
    let mut offset = 0;
    for (line_index, line) in source.split_inclusive('\n').enumerate() {
        if line.starts_with('%') {
            return Some((offset, line_index + 1));
        }
        offset += line.len();
    }
    None
}

fn source_span(span: ParserSpan) -> SourceSpan {
    SourceSpan {
        start: span.start.index(),
        end: span.end.index(),
        line: span.start.line(),
        column: span.start.col().saturating_add(1),
        end_line: span.end.line(),
        end_column: span.end.col().saturating_add(1),
    }
}

fn diagnostic_for_event(
    item: Option<&EventItem<'_>>,
    code: DiagnosticCode,
    message: &str,
) -> Diagnostic {
    item.map_or_else(
        || Diagnostic::new(code, message),
        |event| Diagnostic::at(code, message, event.span),
    )
}

fn parse_node(
    events: &[EventItem<'_>],
    cursor: &mut usize,
    depth: usize,
) -> Result<ConfigValue, Vec<Diagnostic>> {
    if depth > MAX_YAML_DEPTH {
        return Err(vec![diagnostic_for_event(
            events.get(*cursor),
            DiagnosticCode::YamlNestingLimit,
            "YAML nesting exceeds the maximum depth of 128.",
        )]);
    }
    let Some(item) = events.get(*cursor) else {
        return Err(vec![Diagnostic::new(
            DiagnosticCode::YamlSyntax,
            "Unexpected end of YAML input.",
        )]);
    };
    *cursor += 1;

    match &item.event {
        Event::Alias(_) => Err(vec![Diagnostic::at(
            DiagnosticCode::YamlAlias,
            "YAML aliases are not allowed.",
            item.span,
        )]),
        Event::Scalar(value, style, anchor, tag) => {
            reject_anchor(*anchor, item.span)?;
            let tag = explicit_tag(tag.as_deref(), item.span)?;
            let kind = resolve_scalar(value, *style, tag, item.span)?;
            Ok(ConfigValue {
                kind,
                span: item.span,
            })
        }
        Event::SequenceStart(anchor, tag) => {
            reject_anchor(*anchor, item.span)?;
            let tag = explicit_tag(tag.as_deref(), item.span)?;
            if matches!(
                tag,
                Some(
                    ExplicitTag::String
                        | ExplicitTag::Null
                        | ExplicitTag::Boolean
                        | ExplicitTag::Integer
                        | ExplicitTag::Float
                        | ExplicitTag::Mapping
                )
            ) {
                return Err(vec![Diagnostic::at(
                    DiagnosticCode::YamlInvalidScalar,
                    "The explicit YAML tag does not match a sequence value.",
                    item.span,
                )]);
            }
            let mut values = Vec::new();
            loop {
                if matches!(
                    events.get(*cursor).map(|event| &event.event),
                    Some(Event::SequenceEnd)
                ) {
                    let end = events[*cursor].span;
                    *cursor += 1;
                    return Ok(ConfigValue {
                        kind: ConfigValueKind::Sequence(values),
                        span: joined_span(item.span, end),
                    });
                }
                values.push(parse_node(events, cursor, depth + 1)?);
            }
        }
        Event::MappingStart(anchor, tag) => {
            reject_anchor(*anchor, item.span)?;
            let tag = explicit_tag(tag.as_deref(), item.span)?;
            if matches!(
                tag,
                Some(
                    ExplicitTag::String
                        | ExplicitTag::Null
                        | ExplicitTag::Boolean
                        | ExplicitTag::Integer
                        | ExplicitTag::Float
                        | ExplicitTag::Sequence
                )
            ) {
                return Err(vec![Diagnostic::at(
                    DiagnosticCode::YamlInvalidScalar,
                    "The explicit YAML tag does not match a mapping value.",
                    item.span,
                )]);
            }
            let mut keys = HashSet::new();
            let mut entries = Vec::new();
            loop {
                if matches!(
                    events.get(*cursor).map(|event| &event.event),
                    Some(Event::MappingEnd)
                ) {
                    let end = events[*cursor].span;
                    *cursor += 1;
                    return Ok(ConfigValue {
                        kind: ConfigValueKind::Mapping(entries),
                        span: joined_span(item.span, end),
                    });
                }
                let key = parse_node(events, cursor, depth + 1)?;
                let ConfigValueKind::String(key_text) = key.kind else {
                    return Err(vec![Diagnostic::at(
                        DiagnosticCode::YamlNonStringKey,
                        "Mapping keys must resolve to strings.",
                        key.span,
                    )]);
                };
                if key_text == "<<" {
                    return Err(vec![Diagnostic::at(
                        DiagnosticCode::YamlMergeKey,
                        "YAML merge keys are not allowed.",
                        key.span,
                    )]);
                }
                if !keys.insert(key_text.clone()) {
                    return Err(vec![Diagnostic::at(
                        DiagnosticCode::YamlDuplicateKey,
                        format!("Duplicate mapping key `{key_text}`."),
                        key.span,
                    )]);
                }
                let value = parse_node(events, cursor, depth + 1)?;
                entries.push(ConfigEntry {
                    key: key_text,
                    key_span: key.span,
                    value,
                });
            }
        }
        _ => Err(vec![Diagnostic::at(
            DiagnosticCode::YamlSyntax,
            "Unexpected YAML parser event while reading a value.",
            item.span,
        )]),
    }
}

fn reject_anchor(anchor: usize, span: SourceSpan) -> Result<(), Vec<Diagnostic>> {
    if anchor == 0 {
        Ok(())
    } else {
        Err(vec![Diagnostic::at(
            DiagnosticCode::YamlAnchor,
            "YAML anchors are not allowed.",
            span,
        )])
    }
}

fn explicit_tag(
    tag: Option<&Tag>,
    span: SourceSpan,
) -> Result<Option<ExplicitTag>, Vec<Diagnostic>> {
    let Some(tag) = tag else {
        return Ok(None);
    };
    let resolved = if tag.handle.is_empty() && tag.suffix == "!" {
        ExplicitTag::NonSpecific
    } else if tag.handle == "tag:yaml.org,2002:" {
        match tag.suffix.as_str() {
            "str" => ExplicitTag::String,
            "null" => ExplicitTag::Null,
            "bool" => ExplicitTag::Boolean,
            "int" => ExplicitTag::Integer,
            "float" => ExplicitTag::Float,
            "map" => ExplicitTag::Mapping,
            "seq" => ExplicitTag::Sequence,
            _ => return Err(unsupported_tag(tag, span)),
        }
    } else {
        return Err(unsupported_tag(tag, span));
    };
    Ok(Some(resolved))
}

fn unsupported_tag(tag: &Tag, span: SourceSpan) -> Vec<Diagnostic> {
    vec![Diagnostic::at(
        DiagnosticCode::YamlUnsupportedTag,
        format!("YAML tag `{tag}` is not allowed."),
        span,
    )]
}

fn resolve_scalar(
    raw: &str,
    style: ScalarStyle,
    tag: Option<ExplicitTag>,
    span: SourceSpan,
) -> Result<ConfigValueKind, Vec<Diagnostic>> {
    match tag {
        Some(ExplicitTag::NonSpecific) => Ok(ConfigValueKind::String(raw.to_owned())),
        Some(ExplicitTag::String) => Ok(ConfigValueKind::String(raw.to_owned())),
        Some(ExplicitTag::Null) => parse_null(raw).map_or_else(
            || Err(invalid_scalar("null", raw, span)),
            |_| Ok(ConfigValueKind::Null),
        ),
        Some(ExplicitTag::Boolean) => parse_boolean(raw).map_or_else(
            || Err(invalid_scalar("Boolean", raw, span)),
            |value| Ok(ConfigValueKind::Boolean(value)),
        ),
        Some(ExplicitTag::Integer) => match parse_integer(raw) {
            Some(value) => Ok(ConfigValueKind::Number(value)),
            None if integer_parts(raw).is_some() => Err(nonfinite_number(span)),
            None => Err(invalid_scalar("integer", raw, span)),
        },
        Some(ExplicitTag::Float) => match parse_explicit_float(raw) {
            Ok(value) => Ok(ConfigValueKind::Number(value)),
            Err(NumberParseError::Invalid) => {
                Err(invalid_scalar("finite decimal float", raw, span))
            }
            Err(NumberParseError::NonFinite) => Err(nonfinite_number(span)),
        },
        Some(ExplicitTag::Mapping | ExplicitTag::Sequence) => {
            Err(invalid_scalar("scalar", raw, span))
        }
        None if style == ScalarStyle::Plain => resolve_plain_scalar(raw, span),
        None => Ok(ConfigValueKind::String(raw.to_owned())),
    }
}

fn resolve_plain_scalar(raw: &str, span: SourceSpan) -> Result<ConfigValueKind, Vec<Diagnostic>> {
    if parse_null(raw).is_some() {
        return Ok(ConfigValueKind::Null);
    }
    if let Some(value) = parse_boolean(raw) {
        return Ok(ConfigValueKind::Boolean(value));
    }
    if let Some(number) = parse_integer(raw) {
        return Ok(ConfigValueKind::Number(number));
    }
    if integer_parts(raw).is_some() {
        return Err(nonfinite_number(span));
    }
    if is_nonfinite_float(raw) {
        return Err(nonfinite_number(span));
    }
    if is_float_lexeme(raw) {
        let Some(number) = parse_core_float(raw) else {
            return Err(vec![Diagnostic::at(
                DiagnosticCode::YamlNonFiniteNumber,
                "The YAML number is outside the finite JSON number range.",
                span,
            )]);
        };
        return Ok(ConfigValueKind::Number(number));
    }
    Ok(ConfigValueKind::String(raw.to_owned()))
}

fn parse_null(raw: &str) -> Option<()> {
    matches!(raw, "" | "~" | "null" | "Null" | "NULL").then_some(())
}

fn parse_boolean(raw: &str) -> Option<bool> {
    match raw {
        "true" | "True" | "TRUE" => Some(true),
        "false" | "False" | "FALSE" => Some(false),
        _ => None,
    }
}

fn parse_integer(raw: &str) -> Option<Number> {
    let (negative, radix, digits) = integer_parts(raw)?;
    if let Ok(magnitude) = u128::from_str_radix(&digits, radix) {
        if negative {
            if magnitude <= (i64::MAX as u128) + 1 {
                let value = if magnitude == (i64::MAX as u128) + 1 {
                    i64::MIN
                } else {
                    -(magnitude as i64)
                };
                return Some(Number::from(value));
            }
            return Number::from_f64(-(magnitude as f64));
        }
        if let Ok(value) = u64::try_from(magnitude) {
            return Some(Number::from(value));
        }
        return Number::from_f64(magnitude as f64);
    }

    let magnitude = parse_large_integer(&digits, radix)?;
    Number::from_f64(if negative { -magnitude } else { magnitude })
}

fn integer_parts(raw: &str) -> Option<(bool, u32, String)> {
    let (negative, digits) = signed_digits(raw)?;
    let (radix, digits) = if let Some(rest) = digits
        .strip_prefix("0b")
        .or_else(|| digits.strip_prefix("0B"))
    {
        (2, rest)
    } else if let Some(rest) = digits
        .strip_prefix("0o")
        .or_else(|| digits.strip_prefix("0O"))
    {
        (8, rest)
    } else if let Some(rest) = digits
        .strip_prefix("0x")
        .or_else(|| digits.strip_prefix("0X"))
    {
        (16, rest)
    } else {
        (10, digits)
    };
    Some((negative, radix, clean_digit_groups(digits, radix)?))
}

fn parse_large_integer(digits: &str, radix: u32) -> Option<f64> {
    if radix == 10 {
        return digits.parse::<f64>().ok().filter(|value| value.is_finite());
    }
    let mut value = 0.0_f64;
    for byte in digits.bytes() {
        value = value * f64::from(radix) + f64::from(digit_value(byte)?);
        if !value.is_finite() {
            return None;
        }
    }
    Some(value)
}

fn signed_digits(raw: &str) -> Option<(bool, &str)> {
    if let Some(digits) = raw.strip_prefix('-') {
        Some((true, digits))
    } else if let Some(digits) = raw.strip_prefix('+') {
        Some((false, digits))
    } else {
        Some((false, raw))
    }
}

fn clean_digit_groups(raw: &str, radix: u32) -> Option<String> {
    (!raw.is_empty()
        && raw
            .bytes()
            .all(|byte| digit_value(byte).is_some_and(|digit| digit < radix)))
    .then(|| raw.to_owned())
}

fn digit_value(byte: u8) -> Option<u32> {
    match byte {
        b'0'..=b'9' => Some(u32::from(byte - b'0')),
        b'a'..=b'f' => Some(u32::from(byte - b'a') + 10),
        b'A'..=b'F' => Some(u32::from(byte - b'A') + 10),
        _ => None,
    }
}

fn is_nonfinite_float(raw: &str) -> bool {
    matches!(
        raw.to_ascii_lowercase().as_str(),
        ".inf" | "+.inf" | "-.inf" | ".nan" | "+.nan" | "-.nan"
    )
}

fn is_float_lexeme(raw: &str) -> bool {
    let Some((_, unsigned)) = signed_digits(raw) else {
        return false;
    };
    let bytes = unsigned.as_bytes();
    let mut index = 0;
    let whole_end = consume_digit_group(bytes, index);
    if let Some(end) = whole_end {
        index = end;
    }
    let mut has_float_marker = false;
    if bytes.get(index) == Some(&b'.') {
        has_float_marker = true;
        index += 1;
        if let Some(end) = consume_digit_group(bytes, index) {
            index = end;
        }
        if whole_end.is_none() && index == 1 {
            return false;
        }
    } else if whole_end.is_none() {
        return false;
    }
    if bytes
        .get(index)
        .is_some_and(|byte| matches!(byte, b'e' | b'E'))
    {
        has_float_marker = true;
        index += 1;
        if bytes
            .get(index)
            .is_some_and(|byte| matches!(byte, b'+' | b'-'))
        {
            index += 1;
        }
        let Some(end) = consume_digit_group(bytes, index) else {
            return false;
        };
        index = end;
    }
    has_float_marker && index == bytes.len()
}

fn consume_digit_group(bytes: &[u8], mut index: usize) -> Option<usize> {
    if !bytes.get(index).is_some_and(u8::is_ascii_digit) {
        return None;
    }
    index += 1;
    while index < bytes.len() {
        if bytes[index].is_ascii_digit() {
            index += 1;
        } else {
            break;
        }
    }
    Some(index)
}

fn parse_core_float(raw: &str) -> Option<Number> {
    let normalized = if raw.starts_with('.') {
        format!("0{raw}")
    } else if raw.starts_with("+.") {
        format!("+0{}", &raw[1..])
    } else if raw.starts_with("-.") {
        format!("-0{}", &raw[1..])
    } else if raw.ends_with('.') {
        format!("{raw}0")
    } else {
        raw.to_owned()
    };
    let value = normalized.parse::<f64>().ok()?;
    if value == 0.0 && has_nonzero_significand(&normalized) {
        return None;
    }
    Number::from_f64(value)
}

fn has_nonzero_significand(raw: &str) -> bool {
    raw.bytes()
        .take_while(|byte| !matches!(byte, b'e' | b'E'))
        .any(|byte| byte.is_ascii_digit() && byte != b'0')
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum NumberParseError {
    Invalid,
    NonFinite,
}

fn parse_explicit_float(raw: &str) -> Result<Number, NumberParseError> {
    let bytes = raw.as_bytes();
    let mut index = usize::from(matches!(bytes.first(), Some(b'+' | b'-')));
    let mut whole_digits = 0;
    while bytes.get(index).is_some_and(u8::is_ascii_digit) {
        whole_digits += 1;
        index += 1;
    }
    let mut fractional_digits = 0;
    if bytes.get(index) == Some(&b'.') {
        index += 1;
        while bytes.get(index).is_some_and(u8::is_ascii_digit) {
            fractional_digits += 1;
            index += 1;
        }
    }
    if whole_digits == 0 && fractional_digits == 0 {
        return Err(NumberParseError::Invalid);
    }
    if bytes
        .get(index)
        .is_some_and(|byte| matches!(byte, b'e' | b'E'))
    {
        index += 1;
        if bytes
            .get(index)
            .is_some_and(|byte| matches!(byte, b'+' | b'-'))
        {
            index += 1;
        }
        let mut exponent_digits = 0;
        while bytes.get(index).is_some_and(u8::is_ascii_digit) {
            exponent_digits += 1;
            index += 1;
        }
        if exponent_digits == 0 {
            return Err(NumberParseError::Invalid);
        }
    }
    if index != bytes.len() {
        return Err(NumberParseError::Invalid);
    }
    let normalized = raw.strip_prefix('+').unwrap_or(raw);
    let value = normalized
        .parse::<f64>()
        .map_err(|_| NumberParseError::Invalid)?;
    if value == 0.0 && has_nonzero_significand(normalized) {
        return Err(NumberParseError::Invalid);
    }
    Number::from_f64(value).ok_or(NumberParseError::NonFinite)
}

fn nonfinite_number(span: SourceSpan) -> Vec<Diagnostic> {
    vec![Diagnostic::at(
        DiagnosticCode::YamlNonFiniteNumber,
        "Non-finite YAML numbers are not allowed.",
        span,
    )]
}

fn invalid_scalar(expected: &str, raw: &str, span: SourceSpan) -> Vec<Diagnostic> {
    vec![Diagnostic::at(
        DiagnosticCode::YamlInvalidScalar,
        format!("Expected a valid {expected} scalar, found `{raw}`."),
        span,
    )]
}

fn joined_span(start: SourceSpan, end: SourceSpan) -> SourceSpan {
    SourceSpan {
        start: start.start,
        end: end.end,
        line: start.line,
        column: start.column,
        end_line: end.end_line,
        end_column: end.end_column,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn root_value(source: &str, key: &str) -> ConfigValue {
        let document = parse_yaml(source).expect("YAML should parse");
        let entries = document.root().as_mapping().expect("root mapping");
        entries
            .iter()
            .find(|entry| entry.key == key)
            .expect("key exists")
            .value
            .clone()
    }

    #[test]
    fn resolves_yaml_12_core_scalars_and_explicit_standard_tags() {
        for (source, key, expected) in [
            ("on: yes\n", "on", "yes"),
            ("value: no\n", "value", "no"),
            ("value: !!str 12\n", "value", "12"),
            ("value: !!bool true\n", "value", "true"),
            ("value: ! Plain\n", "value", "Plain"),
            ("value: !!null null\n", "value", ""),
        ] {
            let value = root_value(source, key);
            match (&value.kind, expected) {
                (ConfigValueKind::String(actual), expected) => assert_eq!(actual, expected),
                (ConfigValueKind::Boolean(actual), "true") => assert!(*actual),
                (ConfigValueKind::Null, "") => {}
                (kind, expected) => panic!("unexpected {kind:?}, expected {expected:?}"),
            }
        }
        for value in ["1_0", "1_0.0", "0xA_F"] {
            let source = format!("value: {value}\n");
            assert_eq!(
                root_value(&source, "value").kind,
                ConfigValueKind::String(value.to_owned()),
                "{value} must stay a string under YAML 1.2 core resolution"
            );
        }
    }

    #[test]
    fn accepts_integer_form_explicit_floats_without_changing_integers() {
        let zero = root_value("value: !!float 0\n", "value");
        let negative = root_value("value: !!float -1\n", "value");
        let integer = root_value("value: 2\n", "value");
        assert!(
            matches!(zero.kind, ConfigValueKind::Number(ref number) if number.as_f64() == Some(0.0))
        );
        assert!(
            matches!(negative.kind, ConfigValueKind::Number(ref number) if number.as_f64() == Some(-1.0))
        );
        assert_eq!(integer.kind, ConfigValueKind::Number(Number::from(2)));
    }

    #[test]
    fn accepts_standard_collection_tags_and_converts_to_json() {
        let document = parse_yaml("value: !!seq [true, !!str 2]\nobject: !!map {a: null}\n")
            .expect("standard tags with matching node types");
        let json = document.root().to_json_value();
        assert_eq!(json["value"][0], serde_json::Value::Bool(true));
        assert_eq!(json["value"][1], serde_json::Value::String("2".to_owned()));
        assert_eq!(json["object"]["a"], serde_json::Value::Null);
    }

    #[test]
    fn quoted_overflow_is_a_string_and_plain_overflow_is_rejected() {
        let quoted = root_value("value: '1e999'\n", "value");
        assert_eq!(quoted.kind, ConfigValueKind::String("1e999".to_owned()));
        let error = parse_yaml("value: 1e999\n").expect_err("overflow must fail");
        assert_eq!(error[0].code, DiagnosticCode::YamlNonFiniteNumber);
        let underflow = parse_yaml("value: !!float 1e-9999\n")
            .expect_err("underflow must not silently become zero");
        assert_eq!(underflow[0].code, DiagnosticCode::YamlInvalidScalar);
    }

    #[test]
    fn preserves_scalar_key_and_collection_spans() {
        let document = parse_yaml("first: [one, {two: 2}]\n").expect("valid YAML");
        let entry = &document.root().as_mapping().expect("root mapping")[0];
        assert_eq!(
            entry.key_span,
            SourceSpan {
                start: 0,
                end: 5,
                line: 1,
                column: 1,
                end_line: 1,
                end_column: 6,
            }
        );
        assert_eq!(
            entry.value.span,
            SourceSpan {
                start: 7,
                end: 22,
                line: 1,
                column: 8,
                end_line: 1,
                end_column: 23,
            }
        );
        let sequence = entry.value.as_sequence().expect("sequence");
        assert_eq!(sequence[0].span.start, 8);
        assert_eq!(sequence[0].span.end, 11);
        let mapping = sequence[1].as_mapping().expect("nested mapping");
        assert_eq!(mapping[0].key_span.start, 14);
        assert_eq!(mapping[0].key_span.end, 17);
        assert_eq!(mapping[0].value.span.start, 19);
        assert_eq!(mapping[0].value.span.end, 20);
    }

    #[test]
    fn reports_rejected_yaml_features_at_the_offending_source_span() {
        let cases = [
            ("a: 1\na: 2\n", DiagnosticCode::YamlDuplicateKey, 5),
            ("1: value\n", DiagnosticCode::YamlNonStringKey, 0),
            ("true: value\n", DiagnosticCode::YamlNonStringKey, 0),
            ("? [a, b]\n: value\n", DiagnosticCode::YamlNonStringKey, 2),
            (
                "value: !custom text\n",
                DiagnosticCode::YamlUnsupportedTag,
                15,
            ),
            (
                "value: !custom {}\n",
                DiagnosticCode::YamlUnsupportedTag,
                15,
            ),
            ("value: &shared text\n", DiagnosticCode::YamlAnchor, 15),
            ("value: *missing\n", DiagnosticCode::YamlAlias, 7),
            ("value: {<<: {a: b}}\n", DiagnosticCode::YamlMergeKey, 8),
            ("value: {\"<<\": {a: b}}\n", DiagnosticCode::YamlMergeKey, 8),
            ("value: .inf\n", DiagnosticCode::YamlNonFiniteNumber, 7),
            ("value: -.Inf\n", DiagnosticCode::YamlNonFiniteNumber, 7),
            ("value: .nan\n", DiagnosticCode::YamlNonFiniteNumber, 7),
            (
                "value: !!set {a: null}\n",
                DiagnosticCode::YamlUnsupportedTag,
                13,
            ),
            ("value: !!float 2x\n", DiagnosticCode::YamlInvalidScalar, 15),
        ];
        for (source, code, offset) in cases {
            let error = parse_yaml(source).expect_err(source).remove(0);
            assert_eq!(error.code, code, "{source}");
            assert_eq!(
                error.span.expect("source location").start,
                offset,
                "{source}"
            );
        }
    }

    #[test]
    fn rejects_document_shape_directives_and_multiple_documents() {
        for (source, code) in [
            ("", DiagnosticCode::YamlRootType),
            ("- item\n", DiagnosticCode::YamlRootType),
            ("scalar\n", DiagnosticCode::YamlRootType),
            ("a: b\n---\nc: d\n", DiagnosticCode::YamlDocumentCount),
            ("%YAML 1.1\n---\na: b\n", DiagnosticCode::YamlDirective),
            (
                "%TAG !e! tag:example.com,2020:\n---\na: b\n",
                DiagnosticCode::YamlDirective,
            ),
        ] {
            let error = parse_yaml(source).expect_err(source).remove(0);
            assert_eq!(error.code, code, "{source}");
            if !source.is_empty() {
                assert!(error.span.is_some(), "{source}");
            }
        }
    }
}
