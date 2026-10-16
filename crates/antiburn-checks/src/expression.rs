//! Parser for the currently supported expression syntax.

use serde::Serialize;
use serde_json::Number;

use crate::diagnostics::DiagnosticCode;

const MAX_EXPRESSION_DEPTH: usize = 128;

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
            Self::Not(value) => value.visit_paths(visitor),
            Self::Binary { left, right, .. } => {
                left.visit_paths(visitor);
                right.visit_paths(visitor);
            }
        }
    }
}

/// Parses one expression using the supported operators and static access paths.
///
/// Function calls, projections, dynamic indexes, arithmetic, and assignment are
/// not supported yet.
///
/// # Errors
/// Returns a stable unsupported-feature or expression-syntax error.
pub fn parse_expression(source: &str) -> Result<Expression, ExpressionError> {
    let tokens = lex(source)?;
    let mut parser = ExpressionParser { tokens, cursor: 0 };
    let expression = parser.parse_binary(0, 0)?;
    if !matches!(parser.current(), TokenKind::End) {
        return Err(parser.error(
            DiagnosticCode::CompileInvalidExpression,
            "Unexpected content after the expression.",
        ));
    }
    Ok(expression)
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
    ) -> Result<Expression, ExpressionError> {
        if depth > MAX_EXPRESSION_DEPTH {
            return Err(self.error(
                DiagnosticCode::CompileInvalidExpression,
                "Expression nesting exceeds the maximum depth of 128.",
            ));
        }
        let mut left = self.parse_unary(depth + 1)?;
        while let Some((operator, precedence)) = self.binary_operator() {
            if precedence < minimum_precedence {
                break;
            }
            self.advance();
            let right = self.parse_binary(precedence + 1, depth + 1)?;
            left = Expression::Binary {
                operator,
                left: Box::new(left),
                right: Box::new(right),
            };
        }
        Ok(left)
    }

    fn parse_unary(&mut self, depth: usize) -> Result<Expression, ExpressionError> {
        if depth > MAX_EXPRESSION_DEPTH {
            return Err(self.error(
                DiagnosticCode::CompileInvalidExpression,
                "Expression nesting exceeds the maximum depth of 128.",
            ));
        }
        if matches!(self.current(), TokenKind::Not) {
            self.advance();
            return Ok(Expression::Not(Box::new(self.parse_unary(depth + 1)?)));
        }
        self.parse_primary(depth + 1)
    }

    fn parse_primary(&mut self, depth: usize) -> Result<Expression, ExpressionError> {
        let mut expression = match self.advance() {
            TokenKind::Identifier(value) => match value.as_str() {
                "true" => Expression::Literal(ExpressionLiteral::Boolean(true)),
                "false" => Expression::Literal(ExpressionLiteral::Boolean(false)),
                "null" => Expression::Literal(ExpressionLiteral::Null),
                _ => {
                    if matches!(self.current(), TokenKind::LeftParen) {
                        return Err(self.error(
                            DiagnosticCode::CompileUnsupportedFeature,
                            "Function calls are not supported yet.",
                        ));
                    }
                    Expression::Path(vec![PathSegment::Property(value)])
                }
            },
            TokenKind::String(value) => Expression::Literal(ExpressionLiteral::String(value)),
            TokenKind::Number(value) => Expression::Literal(ExpressionLiteral::Number(value)),
            TokenKind::LeftParen => {
                let nested = self.parse_binary(0, depth + 1)?;
                if !matches!(self.advance(), TokenKind::RightParen) {
                    return Err(self.error(
                        DiagnosticCode::CompileInvalidExpression,
                        "Expected `)` to close the expression group.",
                    ));
                }
                nested
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
                            push_access(&mut expression, PathSegment::Property(name), self)?;
                        }
                        TokenKind::Star => {
                            return Err(self.error(
                                DiagnosticCode::CompileUnsupportedFeature,
                                "Projection syntax is not supported yet.",
                            ));
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
                    let segment = match self.advance() {
                        TokenKind::String(name) => PathSegment::Property(name),
                        TokenKind::Number(number) => {
                            let Some(index) = number
                                .as_u64()
                                .and_then(|value| usize::try_from(value).ok())
                            else {
                                return Err(self.error(
                                    DiagnosticCode::CompileUnsupportedFeature,
                                    "Only nonnegative literal array indexes are supported.",
                                ));
                            };
                            PathSegment::Index(index)
                        }
                        _ => {
                            return Err(self.error(
                                DiagnosticCode::CompileUnsupportedFeature,
                                "Dynamic bracket access is not supported yet.",
                            ));
                        }
                    };
                    if !matches!(self.advance(), TokenKind::RightBracket) {
                        return Err(self.error(
                            DiagnosticCode::CompileInvalidExpression,
                            "Expected `]` after a fixed bracket key or index.",
                        ));
                    }
                    push_access(&mut expression, segment, self)?;
                }
                TokenKind::LeftParen => {
                    return Err(self.error(
                        DiagnosticCode::CompileUnsupportedFeature,
                        "Function and method calls are not supported yet.",
                    ));
                }
                _ => break,
            }
        }
        Ok(expression)
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

fn push_access(
    expression: &mut Expression,
    segment: PathSegment,
    parser: &ExpressionParser,
) -> Result<(), ExpressionError> {
    if let Expression::Path(segments) = expression {
        segments.push(segment);
        Ok(())
    } else {
        Err(parser.error(
            DiagnosticCode::CompileInvalidExpression,
            "Property access requires a context path.",
        ))
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
