//! Token types for the Lox scanner and parser.

use std::fmt;

/// Every lexical token kind the Lox scanner can produce.
///
/// Mirrors the book's `TokenType` enum (Chapter 4).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum TokenType {
    // Single-character tokens.
    LeftParen,
    RightParen,
    LeftBrace,
    RightBrace,
    Comma,
    Dot,
    Minus,
    Plus,
    Semicolon,
    Slash,
    Star,

    // One or two character tokens.
    Bang,
    BangEqual,
    Equal,
    EqualEqual,
    Greater,
    GreaterEqual,
    Less,
    LessEqual,

    // Literals.
    Identifier,
    String,
    Number,

    // Keywords.
    And,
    Class,
    Else,
    False,
    Fun,
    For,
    If,
    Nil,
    Or,
    Print,
    Return,
    Super,
    This,
    True,
    Var,
    While,

    Eof,
}

/// Literal payload attached to certain tokens and carried through values.
#[derive(Debug, Clone)]
pub enum Literal {
    Str(String),
    Num(f64),
    Bool(bool),
    Nil,
}

impl PartialEq for Literal {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Literal::Str(a), Literal::Str(b)) => a == b,
            (Literal::Num(a), Literal::Num(b)) => a == b,
            (Literal::Bool(a), Literal::Bool(b)) => a == b,
            (Literal::Nil, Literal::Nil) => true,
            _ => false,
        }
    }
}

impl fmt::Display for Literal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Literal::Str(s) => write!(f, "{s}"),
            Literal::Num(n) => {
                if *n == n.trunc() && n.is_finite() {
                    write!(f, "{n:.0}")
                } else {
                    write!(f, "{n}")
                }
            }
            Literal::Bool(b) => write!(f, "{b}"),
            Literal::Nil => write!(f, "nil"),
        }
    }
}

/// A single Lox token produced by the scanner.
#[derive(Debug, Clone)]
pub struct Token {
    pub ttype: TokenType,
    pub lexeme: String,
    pub literal: Option<Literal>,
    pub line: usize,
}

impl Token {
    pub fn new(ttype: TokenType, lexeme: impl Into<String>, literal: Option<Literal>, line: usize) -> Self {
        Self { ttype, lexeme: lexeme.into(), literal, line }
    }
}

impl fmt::Display for Token {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.literal {
            Some(lit) => write!(f, "{:?} {} {}", self.ttype, self.lexeme, lit),
            None => write!(f, "{:?} {}", self.ttype, self.lexeme),
        }
    }
}

#[cfg(test)]
mod token_tests {
    use super::*;

    #[test]
    fn token_type_equality() {
        assert_eq!(TokenType::LeftParen, TokenType::LeftParen);
        assert_ne!(TokenType::LeftParen, TokenType::RightParen);
    }

    #[test]
    fn token_display_includes_lexeme() {
        let t = Token::new(TokenType::Identifier, "foo", None, 1);
        let s = format!("{t}");
        assert!(s.contains("Identifier"));
        assert!(s.contains("foo"));
    }

    #[test]
    fn literal_number_integral_has_no_trailing_decimal() {
        let n = Literal::Num(42.0);
        assert_eq!(format!("{n}"), "42");
    }

    #[test]
    fn literal_number_fractional_preserved() {
        let n = Literal::Num(3.14);
        assert_eq!(format!("{n}"), "3.14");
    }

    #[test]
    fn literal_nil_display_is_nil() {
        assert_eq!(format!("{}", Literal::Nil), "nil");
    }

    #[test]
    fn literal_bool_display() {
        assert_eq!(format!("{}", Literal::Bool(true)), "true");
        assert_eq!(format!("{}", Literal::Bool(false)), "false");
    }

    #[test]
    fn literal_str_display_is_unquoted() {
        let s = Literal::Str("hi".to_string());
        assert_eq!(format!("{s}"), "hi");
    }

    #[test]
    fn literal_equality() {
        assert_eq!(Literal::Num(1.0), Literal::Num(1.0));
        assert_ne!(Literal::Num(1.0), Literal::Num(2.0));
        assert_eq!(Literal::Str("a".into()), Literal::Str("a".into()));
        assert_eq!(Literal::Nil, Literal::Nil);
    }
}
