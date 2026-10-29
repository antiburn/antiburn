//! Parser for pure Burn Check expressions.

use serde::Serialize;
use serde_json::Number;

use crate::diagnostics::DiagnosticCode;

const MAX_EXPRESSION_PARSE_DEPTH: usize = 128;
const MAX_EXPRESSION_DEPTH: usize = crate::expression_eval::DEFAULT_MAX_EVALUATION_DEPTH;

/// One property or fixed array index in a normalized expression path.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub enum PathSegment {
    /// An object property name.
    Property(String),
    /// A nonnegative literal array index.
    Index(usize),
}

/// A parsed expression.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub enum Expression {
    /// A JSON-compatible literal.
    Literal(ExpressionLiteral),
    /// A static property or array-index path.
    Path(Vec<PathSegment>),
    /// Access on a computed value, including dynamic keys and projections.
    Access {
        /// The value that owns the selected fields.
        base: Box<Self>,
        /// Access steps in authored order.
        segments: Vec<AccessSegment>,
    },
    /// A supported pure function call.
    Call {
        /// The function to call.
        function: ExpressionFunction,
        /// Arguments in authored order.
        arguments: Vec<Self>,
    },
    /// Boolean negation.
    Not(Box<Self>),
    /// A supported binary operation.
    Binary {
        /// The operation to apply.
        operator: BinaryOperator,
        /// The left operand.
        left: Box<Self>,
        /// The right operand.
        right: Box<Self>,
    },
}

/// One step in access on a computed value.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub enum AccessSegment {
    /// A fixed object key or array length.
    Property(String),
    /// A fixed nonnegative array index.
    Index(usize),
    /// A string key or nonnegative integral index evaluated at runtime.
    Dynamic(Box<Expression>),
    /// Selection over ordered array items or sorted object values.
    Projection,
}

/// A supported pure expression function.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum ExpressionFunction {
    StrictEquals,
    Contains,
    StartsWith,
    EndsWith,
    Join,
    Format,
    ToJSON,
    FromJSON,
}

impl ExpressionFunction {
    fn parse(name: &str) -> Option<Self> {
        match name.to_ascii_lowercase().as_str() {
            "strictequals" => Some(Self::StrictEquals),
            "contains" => Some(Self::Contains),
            "startswith" => Some(Self::StartsWith),
            "endswith" => Some(Self::EndsWith),
            "join" => Some(Self::Join),
            "format" => Some(Self::Format),
            "tojson" => Some(Self::ToJSON),
            "fromjson" => Some(Self::FromJSON),
            _ => None,
        }
    }

    pub(crate) fn accepts_arity(self, count: usize) -> bool {
        match self {
            Self::StrictEquals | Self::Contains | Self::StartsWith | Self::EndsWith => count == 2,
            Self::Join => (1..=2).contains(&count),
            Self::Format => count >= 2,
            Self::ToJSON | Self::FromJSON => count == 1,
        }
    }
}

/// A literal value in an expression.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub enum ExpressionLiteral {
    /// A string literal.
    String(String),
    /// A finite number literal.
    Number(Number),
    /// A Boolean literal.
    Boolean(bool),
    /// A null literal.
    Null,
}

/// A supported expression operator.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum BinaryOperator {
    /// Less than.
    Less,
    /// Less than or equal.
    LessOrEqual,
    /// Greater than.
    Greater,
    /// Greater than or equal.
    GreaterOrEqual,
    /// Equal using the Burn Check comparison rules.
    Equal,
    /// Not equal using the Burn Check comparison rules.
    NotEqual,
    /// Boolean conjunction with short-circuit evaluation at runtime.
    And,
    /// Boolean disjunction with short-circuit evaluation at runtime.
    Or,
}

/// A syntax or capability error found while parsing an expression.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExpressionError {
    /// The stable diagnostic code.
    pub code: DiagnosticCode,
    /// The short explanation.
    pub message: String,
}

impl Expression {
    /// Visits every static path in this expression.
    pub fn visit_paths(&self, visitor: &mut impl FnMut(&[PathSegment])) {
        match self {
            Self::Literal(_) => {}
            Self::Path(segments) => visitor(segments),
            Self::Access { base, segments } => {
                base.visit_paths(visitor);
                for segment in segments {
                    if let AccessSegment::Dynamic(expression) = segment {
                        expression.visit_paths(visitor);
                    }
                }
            }
            Self::Call { arguments, .. } => {
                for argument in arguments {
                    argument.visit_paths(visitor);
                }
            }
            Self::Not(value) => value.visit_paths(visitor),
            Self::Binary { left, right, .. } => {
                left.visit_paths(visitor);
                right.visit_paths(visitor);
            }
        }
    }

    /// Visits each computed access chain and all nested expressions.
    pub fn visit_accesses(&self, visitor: &mut impl FnMut(&Self, &[AccessSegment])) {
        match self {
            Self::Access { base, segments } => {
                visitor(base, segments);
                base.visit_accesses(visitor);
                for segment in segments {
                    if let AccessSegment::Dynamic(expression) = segment {
                        expression.visit_accesses(visitor);
                    }
                }
            }
            Self::Call { arguments, .. } => {
                for argument in arguments {
                    argument.visit_accesses(visitor);
                }
            }
            Self::Not(value) => value.visit_accesses(visitor),
            Self::Binary { left, right, .. } => {
                left.visit_accesses(visitor);
                right.visit_accesses(visitor);
            }
            Self::Literal(_) | Self::Path(_) => {}
        }
    }
}

/// Parses one expression with pure calls, access, and supported operators.
///
/// # Errors
/// Returns a stable unsupported-feature or expression-syntax error.
pub fn parse_expression(source: &str) -> Result<Expression, ExpressionError> {
    let tokens = lex(source)?;
    let mut parser = ExpressionParser { tokens, cursor: 0 };
    let expression = parser.parse_binary(0, 0)?.expression;
    if !matches!(parser.current(), TokenKind::End) {
        return Err(parser.error(
            DiagnosticCode::CompileInvalidExpression,
            "Unexpected content after the expression.",
        ));
    }
    Ok(expression)
}

struct ParsedExpression {
    expression: Expression,
    depth: usize,
}

#[derive(Clone, Debug, PartialEq)]
enum TokenKind {
    Identifier(String),
    String(String),
    Number(Number),
    And,
    Or,
    Equal,
    NotEqual,
    Less,
    LessOrEqual,
    Greater,
    GreaterOrEqual,
    Not,
    LeftParen,
    RightParen,
    LeftBracket,
    RightBracket,
    Dot,
    Star,
    Comma,
    End,
}

#[derive(Clone, Debug)]
struct Token {
    kind: TokenKind,
    offset: usize,
}

struct ExpressionParser {
    tokens: Vec<Token>,
    cursor: usize,
}

impl ExpressionParser {
    fn current(&self) -> &TokenKind {
        &self.tokens[self.cursor].kind
    }

    fn advance(&mut self) -> TokenKind {
        let kind = self.current().clone();
        if !matches!(kind, TokenKind::End) {
            self.cursor += 1;
        }
        kind
    }

    fn parse_binary(
        &mut self,
        minimum_precedence: u8,
        depth: usize,
    ) -> Result<ParsedExpression, ExpressionError> {
        self.check_parse_depth(depth)?;
        let mut left = self.parse_unary(depth + 1)?;
        while let Some((operator, precedence)) = self.binary_operator() {
            if precedence < minimum_precedence {
                break;
            }
            self.advance();
            let right = self.parse_binary(precedence + 1, depth + 1)?;
            let expression_depth = left.depth.max(right.depth).saturating_add(1);
            if expression_depth > MAX_EXPRESSION_DEPTH {
                return Err(self.expression_depth_error());
            }
            left = ParsedExpression {
                expression: Expression::Binary {
                    operator,
                    left: Box::new(left.expression),
                    right: Box::new(right.expression),
                },
                depth: expression_depth,
            };
        }
        Ok(left)
    }

    fn parse_unary(&mut self, depth: usize) -> Result<ParsedExpression, ExpressionError> {
        self.check_parse_depth(depth)?;
        if matches!(self.current(), TokenKind::Not) {
            self.advance();
            let nested = self.parse_unary(depth + 1)?;
            let expression_depth = nested.depth.saturating_add(1);
            if expression_depth > MAX_EXPRESSION_DEPTH {
                return Err(self.expression_depth_error());
            }
            return Ok(ParsedExpression {
                expression: Expression::Not(Box::new(nested.expression)),
                depth: expression_depth,
            });
        }
        self.parse_primary(depth + 1)
    }

    fn parse_primary(&mut self, depth: usize) -> Result<ParsedExpression, ExpressionError> {
        self.check_parse_depth(depth)?;
        let mut expression_depth = 1;
        let mut grouped = matches!(self.current(), TokenKind::LeftParen);
        let mut expression = match self.advance() {
            TokenKind::Identifier(value) => match value.as_str() {
                "true" => Expression::Literal(ExpressionLiteral::Boolean(true)),
                "false" => Expression::Literal(ExpressionLiteral::Boolean(false)),
                "null" => Expression::Literal(ExpressionLiteral::Null),
                _ if matches!(self.current(), TokenKind::LeftParen) => {
                    let call = self.parse_call(&value, depth)?;
                    expression_depth = call.depth;
                    call.expression
                }
                _ => Expression::Path(vec![PathSegment::Property(value)]),
            },
            TokenKind::String(value) => Expression::Literal(ExpressionLiteral::String(value)),
            TokenKind::Number(value) => Expression::Literal(ExpressionLiteral::Number(value)),
            TokenKind::LeftParen => {
                let nested = self.parse_binary(0, depth + 1)?;
                expression_depth = nested.depth;
                if !matches!(self.advance(), TokenKind::RightParen) {
                    return Err(self.error(
                        DiagnosticCode::CompileInvalidExpression,
                        "Expected `)` to close the expression group.",
                    ));
                }
                nested.expression
            }
            TokenKind::End => {
                return Err(self.error(
                    DiagnosticCode::CompileInvalidExpression,
                    "Expected an expression value.",
                ));
            }
            _ => {
                return Err(self.error(
                    DiagnosticCode::CompileInvalidExpression,
                    "Expected a literal, path, or parenthesized expression.",
                ));
            }
        };

        loop {
            match self.current() {
                TokenKind::Dot => {
                    self.advance();
                    match self.advance() {
                        TokenKind::Identifier(name) => {
                            push_access(
                                &mut expression,
                                AccessSegment::Property(name),
                                &mut grouped,
                            );
                        }
                        TokenKind::Star => {
                            push_access(&mut expression, AccessSegment::Projection, &mut grouped);
                        }
                        _ => {
                            return Err(self.error(
                                DiagnosticCode::CompileInvalidExpression,
                                "Expected a property name after `.`.",
                            ));
                        }
                    }
                }
                TokenKind::LeftBracket => {
                    self.advance();
                    let selector = self.parse_binary(0, depth + 1)?;
                    expression_depth = expression_depth.max(selector.depth + 1);
                    let segment = match selector.expression {
                        Expression::Literal(ExpressionLiteral::String(name)) => {
                            AccessSegment::Property(name)
                        }
                        Expression::Literal(ExpressionLiteral::Number(ref number))
                            if number
                                .as_u64()
                                .and_then(|value| usize::try_from(value).ok())
                                .is_some() =>
                        {
                            AccessSegment::Index(
                                number
                                    .as_u64()
                                    .and_then(|value| usize::try_from(value).ok())
                                    .expect("checked index"),
                            )
                        }
                        expression => AccessSegment::Dynamic(Box::new(expression)),
                    };
                    if !matches!(self.advance(), TokenKind::RightBracket) {
                        return Err(self.error(
                            DiagnosticCode::CompileInvalidExpression,
                            "Expected `]` after a bracket key or index.",
                        ));
                    }
                    push_access(&mut expression, segment, &mut grouped);
                }
                TokenKind::LeftParen => {
                    return Err(self.error(
                        DiagnosticCode::CompileUnsupportedFeature,
                        "Method calls and calls on computed values are not supported.",
                    ));
                }
                _ => break,
            }
        }
        if expression_depth > MAX_EXPRESSION_DEPTH {
            return Err(self.expression_depth_error());
        }
        Ok(ParsedExpression {
            expression,
            depth: expression_depth,
        })
    }

    fn parse_call(
        &mut self,
        name: &str,
        depth: usize,
    ) -> Result<ParsedExpression, ExpressionError> {
        let Some(function) = ExpressionFunction::parse(name) else {
            return Err(self.error(
                DiagnosticCode::CompileUnsupportedFeature,
                "Unknown expression function.",
            ));
        };
        self.advance();
        let mut arguments = Vec::new();
        let mut expression_depth = 1;
        if !matches!(self.current(), TokenKind::RightParen) {
            loop {
                let argument = self.parse_binary(0, depth + 1)?;
                expression_depth = expression_depth.max(argument.depth + 1);
                arguments.push(argument.expression);
                if !matches!(self.current(), TokenKind::Comma) {
                    break;
                }
                self.advance();
            }
        }
        if !matches!(self.advance(), TokenKind::RightParen) {
            return Err(self.error(
                DiagnosticCode::CompileInvalidExpression,
                "Expected `)` after function arguments.",
            ));
        }
        if !function.accepts_arity(arguments.len()) {
            return Err(self.error(
                DiagnosticCode::CompileInvalidExpression,
                "Wrong number of expression function arguments.",
            ));
        }
        Ok(ParsedExpression {
            expression: Expression::Call {
                function,
                arguments,
            },
            depth: expression_depth,
        })
    }

    fn check_parse_depth(&self, depth: usize) -> Result<(), ExpressionError> {
        if depth > MAX_EXPRESSION_PARSE_DEPTH {
            return Err(self.error(
                DiagnosticCode::CompileInvalidExpression,
                "Expression syntax exceeds the parser nesting limit of 128.",
            ));
        }
        Ok(())
    }

    fn expression_depth_error(&self) -> ExpressionError {
        self.error(
            DiagnosticCode::CompileInvalidExpression,
            &format!(
                "Expression AST exceeds the maximum evaluation depth of {MAX_EXPRESSION_DEPTH}."
            ),
        )
    }

    fn binary_operator(&self) -> Option<(BinaryOperator, u8)> {
        match self.current() {
            TokenKind::Or => Some((BinaryOperator::Or, 1)),
            TokenKind::And => Some((BinaryOperator::And, 2)),
            TokenKind::Equal => Some((BinaryOperator::Equal, 3)),
            TokenKind::NotEqual => Some((BinaryOperator::NotEqual, 3)),
            TokenKind::Less => Some((BinaryOperator::Less, 4)),
            TokenKind::LessOrEqual => Some((BinaryOperator::LessOrEqual, 4)),
            TokenKind::Greater => Some((BinaryOperator::Greater, 4)),
            TokenKind::GreaterOrEqual => Some((BinaryOperator::GreaterOrEqual, 4)),
            _ => None,
        }
    }

    fn error(&self, code: DiagnosticCode, message: &str) -> ExpressionError {
        let offset = self.tokens[self.cursor].offset;
        ExpressionError {
            code,
            message: format!("{message} (expression byte {offset})."),
        }
    }
}

fn push_access(expression: &mut Expression, segment: AccessSegment, grouped: &mut bool) {
    if *grouped && matches!(expression, Expression::Access { .. }) {
        let base = std::mem::replace(expression, Expression::Literal(ExpressionLiteral::Null));
        *expression = Expression::Access {
            base: Box::new(base),
            segments: vec![segment],
        };
        *grouped = false;
        return;
    }
    *grouped = false;
    match (expression, segment) {
        (Expression::Path(segments), AccessSegment::Property(name)) => {
            segments.push(PathSegment::Property(name))
        }
        (Expression::Path(segments), AccessSegment::Index(index)) => {
            segments.push(PathSegment::Index(index))
        }
        (Expression::Access { segments, .. }, segment) => segments.push(segment),
        (expression, segment) => {
            let base = std::mem::replace(expression, Expression::Literal(ExpressionLiteral::Null));
            *expression = Expression::Access {
                base: Box::new(base),
                segments: vec![segment],
            };
        }
    }
}

fn lex(source: &str) -> Result<Vec<Token>, ExpressionError> {
    let bytes = source.as_bytes();
    let mut tokens = Vec::new();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index].is_ascii_whitespace() {
            index += 1;
            continue;
        }
        let start = index;
        let kind = match bytes[index] {
            b'&' if bytes.get(index + 1) == Some(&b'&') => {
                index += 2;
                TokenKind::And
            }
            b'|' if bytes.get(index + 1) == Some(&b'|') => {
                index += 2;
                TokenKind::Or
            }
            b'=' if bytes.get(index + 1) == Some(&b'=') => {
                index += 2;
                TokenKind::Equal
            }
            b'!' if bytes.get(index + 1) == Some(&b'=') => {
                index += 2;
                TokenKind::NotEqual
            }
            b'!' => {
                index += 1;
                TokenKind::Not
            }
            b'<' if bytes.get(index + 1) == Some(&b'=') => {
                index += 2;
                TokenKind::LessOrEqual
            }
            b'<' => {
                index += 1;
                TokenKind::Less
            }
            b'>' if bytes.get(index + 1) == Some(&b'=') => {
                index += 2;
                TokenKind::GreaterOrEqual
            }
            b'>' => {
                index += 1;
                TokenKind::Greater
            }
            b'(' => {
                index += 1;
                TokenKind::LeftParen
            }
            b')' => {
                index += 1;
                TokenKind::RightParen
            }
            b'[' => {
                index += 1;
                TokenKind::LeftBracket
            }
            b']' => {
                index += 1;
                TokenKind::RightBracket
            }
            b'.' => {
                index += 1;
                TokenKind::Dot
            }
            b'*' => {
                index += 1;
                TokenKind::Star
            }
            b',' => {
                index += 1;
                TokenKind::Comma
            }
            b'\'' => {
                let (value, end) = lex_string(source, index)?;
                index = end;
                TokenKind::String(value)
            }
            b'-' if bytes.get(index + 1).is_some_and(u8::is_ascii_digit) => {
                let (value, end) = lex_number(source, index)?;
                index = end;
                TokenKind::Number(value)
            }
            byte if byte.is_ascii_digit() => {
                let (value, end) = lex_number(source, index)?;
                index = end;
                TokenKind::Number(value)
            }
            byte if byte.is_ascii_alphabetic() || byte == b'_' => {
                index += 1;
                while bytes
                    .get(index)
                    .is_some_and(|byte| byte.is_ascii_alphanumeric() || *byte == b'_')
                {
                    index += 1;
                }
                TokenKind::Identifier(source[start..index].to_owned())
            }
            _ => {
                return Err(ExpressionError {
                    code: DiagnosticCode::CompileInvalidExpression,
                    message: format!("Unexpected expression character at byte {index}."),
                });
            }
        };
        tokens.push(Token {
            kind,
            offset: start,
        });
    }
    tokens.push(Token {
        kind: TokenKind::End,
        offset: source.len(),
    });
    Ok(tokens)
}

fn lex_string(source: &str, start: usize) -> Result<(String, usize), ExpressionError> {
    let bytes = source.as_bytes();
    let mut index = start + 1;
    let mut value = String::new();
    while index < bytes.len() {
        if bytes[index] == b'\'' {
            if bytes.get(index + 1) == Some(&b'\'') {
                value.push('\'');
                index += 2;
                continue;
            }
            return Ok((value, index + 1));
        }
        let character = source[index..]
            .chars()
            .next()
            .expect("valid UTF-8 boundary");
        value.push(character);
        index += character.len_utf8();
    }
    Err(ExpressionError {
        code: DiagnosticCode::CompileInvalidExpression,
        message: format!("Unterminated string literal at expression byte {start}."),
    })
}

fn lex_number(source: &str, start: usize) -> Result<(Number, usize), ExpressionError> {
    let bytes = source.as_bytes();
    let mut index = start;
    if bytes.get(index) == Some(&b'-') {
        index += 1;
    }
    if bytes.get(index) == Some(&b'0') {
        index += 1;
    } else {
        while bytes.get(index).is_some_and(u8::is_ascii_digit) {
            index += 1;
        }
    }
    if bytes.get(index) == Some(&b'.') {
        index += 1;
        let start_fraction = index;
        while bytes.get(index).is_some_and(u8::is_ascii_digit) {
            index += 1;
        }
        if start_fraction == index {
            return Err(number_error(start));
        }
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
        let start_exponent = index;
        while bytes.get(index).is_some_and(u8::is_ascii_digit) {
            index += 1;
        }
        if start_exponent == index {
            return Err(number_error(start));
        }
    }
    let text = &source[start..index];
    let number = serde_json::from_str::<Number>(text).map_err(|_| number_error(start))?;
    Ok((number, index))
}

fn number_error(start: usize) -> ExpressionError {
    ExpressionError {
        code: DiagnosticCode::CompileInvalidExpression,
        message: format!("Invalid finite JSON number at expression byte {start}."),
    }
}

#[cfg(test)]
#[path = "expression/tests.rs"]
mod tests;
