//! Lexer for Lox. Implements *Crafting Interpreters* chapter 4 semantics.
//!
//! The scanner consumes a source string and produces a `Vec<Token>` terminated
//! by a single `TokenType::Eof` token. It tracks line numbers, supports
//! multi-line strings (without escapes, per the book), and reports the first
//! syntactic error as a human-readable `String` with a `[line N]` prefix.

use crate::token::{Literal, Token, TokenType};

/// Stateful scanner over a borrowed source buffer.
pub struct Scanner<'a> {
    source: &'a str,
    source_bytes: &'a [u8],
    tokens: Vec<Token>,
    /// Byte offset of the first char of the lexeme currently being scanned.
    start: usize,
    /// Byte offset of the next char to consume.
    current: usize,
    /// 1-based current line number.
    line: usize,
}

impl<'a> Scanner<'a> {
    pub fn new(source: &'a str) -> Self {
        Self {
            source,
            source_bytes: source.as_bytes(),
            tokens: Vec::new(),
            start: 0,
            current: 0,
            line: 1,
        }
    }

    /// Scan the entire source, returning either the produced tokens (ending in
    /// `Eof`) or the first error encountered.
    pub fn scan_tokens(mut self) -> Result<Vec<Token>, String> {
        while !self.is_at_end() {
            self.start = self.current;
            self.scan_token()?;
        }
        self.tokens
            .push(Token::new(TokenType::Eof, String::new(), None, self.line));
        Ok(self.tokens)
    }

    fn scan_token(&mut self) -> Result<(), String> {
        let c = self.advance();
        match c {
            b'(' => self.add_token(TokenType::LeftParen),
            b')' => self.add_token(TokenType::RightParen),
            b'{' => self.add_token(TokenType::LeftBrace),
            b'}' => self.add_token(TokenType::RightBrace),
            b',' => self.add_token(TokenType::Comma),
            b'.' => self.add_token(TokenType::Dot),
            b'-' => self.add_token(TokenType::Minus),
            b'+' => self.add_token(TokenType::Plus),
            b';' => self.add_token(TokenType::Semicolon),
            b'*' => self.add_token(TokenType::Star),
            b'!' => {
                let t = if self.match_byte(b'=') {
                    TokenType::BangEqual
                } else {
                    TokenType::Bang
                };
                self.add_token(t);
            }
            b'=' => {
                let t = if self.match_byte(b'=') {
                    TokenType::EqualEqual
                } else {
                    TokenType::Equal
                };
                self.add_token(t);
            }
            b'<' => {
                let t = if self.match_byte(b'=') {
                    TokenType::LessEqual
                } else {
                    TokenType::Less
                };
                self.add_token(t);
            }
            b'>' => {
                let t = if self.match_byte(b'=') {
                    TokenType::GreaterEqual
                } else {
                    TokenType::Greater
                };
                self.add_token(t);
            }
            b'/' => {
                if self.match_byte(b'/') {
                    // Line comment: consume to end of line (not including \n).
                    while !self.is_at_end() && self.peek() != b'\n' {
                        self.current += 1;
                    }
                } else {
                    self.add_token(TokenType::Slash);
                }
            }
            b' ' | b'\r' | b'\t' => {}
            b'\n' => self.line += 1,
            b'"' => self.string()?,
            b if is_digit(b) => self.number(),
            b if is_alpha(b) => self.identifier(),
            other => {
                return Err(format!(
                    "[line {}] Error: Unexpected character '{}'.",
                    self.line, other as char
                ));
            }
        }
        Ok(())
    }

    fn string(&mut self) -> Result<(), String> {
        // Consume chars until the closing `"`. Multi-line allowed; newlines
        // increment `line`. No escape sequences per book.
        while !self.is_at_end() && self.peek() != b'"' {
            if self.peek() == b'\n' {
                self.line += 1;
            }
            self.current += 1;
        }

        if self.is_at_end() {
            return Err(format!("[line {}] Error: Unterminated string.", self.line));
        }

        // Consume the closing `"`.
        self.current += 1;

        // Literal excludes surrounding quotes. Byte offsets are safe here
        // because `"` is a single ASCII byte.
        let value = &self.source[self.start + 1..self.current - 1];
        self.add_token_with_literal(TokenType::String, Some(Literal::Str(value.to_string())));
        Ok(())
    }

    fn number(&mut self) {
        while !self.is_at_end() && is_digit(self.peek()) {
            self.current += 1;
        }
        // Fractional part only if `.` followed by a digit (rejects `5.`).
        if !self.is_at_end() && self.peek() == b'.' && is_digit(self.peek_next()) {
            self.current += 1; // consume '.'
            while !self.is_at_end() && is_digit(self.peek()) {
                self.current += 1;
            }
        }
        let text = &self.source[self.start..self.current];
        let n: f64 = text
            .parse()
            .expect("scanner produced a non-parseable number lexeme");
        self.add_token_with_literal(TokenType::Number, Some(Literal::Num(n)));
    }

    fn identifier(&mut self) {
        while !self.is_at_end() && is_alphanumeric(self.peek()) {
            self.current += 1;
        }
        let text = &self.source[self.start..self.current];
        let ttype = keyword(text).unwrap_or(TokenType::Identifier);
        self.add_token(ttype);
    }

    // ---------- low-level helpers ----------

    fn is_at_end(&self) -> bool {
        self.current >= self.source_bytes.len()
    }

    /// Consume and return the next ASCII byte. Scanner only operates on ASCII
    /// because Lox lexical classes are all ASCII; non-ASCII bytes fall into
    /// the "unexpected character" branch.
    fn advance(&mut self) -> u8 {
        let b = self.source_bytes[self.current];
        self.current += 1;
        b
    }

    fn peek(&self) -> u8 {
        if self.is_at_end() {
            0
        } else {
            self.source_bytes[self.current]
        }
    }

    fn peek_next(&self) -> u8 {
        if self.current + 1 >= self.source_bytes.len() {
            0
        } else {
            self.source_bytes[self.current + 1]
        }
    }

    fn match_byte(&mut self, expected: u8) -> bool {
        if self.is_at_end() || self.source_bytes[self.current] != expected {
            return false;
        }
        self.current += 1;
        true
    }

    fn add_token(&mut self, ttype: TokenType) {
        self.add_token_with_literal(ttype, None);
    }

    fn add_token_with_literal(&mut self, ttype: TokenType, literal: Option<Literal>) {
        let lexeme = self.source[self.start..self.current].to_string();
        self.tokens
            .push(Token::new(ttype, lexeme, literal, self.line));
    }
}

// ---------- character class helpers (ASCII) ----------

fn is_digit(b: u8) -> bool {
    b.is_ascii_digit()
}

fn is_alpha(b: u8) -> bool {
    b.is_ascii_alphabetic() || b == b'_'
}

fn is_alphanumeric(b: u8) -> bool {
    is_alpha(b) || is_digit(b)
}

fn keyword(ident: &str) -> Option<TokenType> {
    Some(match ident {
        "and" => TokenType::And,
        "class" => TokenType::Class,
        "else" => TokenType::Else,
        "false" => TokenType::False,
        "fun" => TokenType::Fun,
        "for" => TokenType::For,
        "if" => TokenType::If,
        "nil" => TokenType::Nil,
        "or" => TokenType::Or,
        "print" => TokenType::Print,
        "return" => TokenType::Return,
        "super" => TokenType::Super,
        "this" => TokenType::This,
        "true" => TokenType::True,
        "var" => TokenType::Var,
        "while" => TokenType::While,
        _ => return None,
    })
}

#[cfg(test)]
mod scanner_tests {
    use super::*;
    use crate::token::TokenType::*;

    fn types(src: &str) -> Vec<TokenType> {
        Scanner::new(src)
            .scan_tokens()
            .unwrap()
            .into_iter()
            .map(|t| t.ttype)
            .collect()
    }

    #[test]
    fn scanner_single_char_tokens() {
        // Covers every single-char token the grammar defines.
        let src = "(){},.-+;/*";
        assert_eq!(
            types(src),
            vec![
                LeftParen, RightParen, LeftBrace, RightBrace, Comma, Dot, Minus, Plus, Semicolon,
                Slash, Star, Eof
            ]
        );
    }

    #[test]
    fn scanner_operators_two_char() {
        assert_eq!(
            types("! != = == < <= > >="),
            vec![
                Bang,
                BangEqual,
                Equal,
                EqualEqual,
                Less,
                LessEqual,
                Greater,
                GreaterEqual,
                Eof
            ]
        );
    }

    #[test]
    fn scanner_line_comment_ignored() {
        assert_eq!(types("// comment\n+"), vec![Plus, Eof]);
    }

    #[test]
    fn scanner_string_literal_single_line() {
        let toks = Scanner::new("\"hi\"").scan_tokens().unwrap();
        assert_eq!(toks.len(), 2);
        assert_eq!(toks[0].ttype, String);
        assert_eq!(toks[0].literal, Some(Literal::Str("hi".to_string())));
        assert_eq!(toks[1].ttype, Eof);
    }

    #[test]
    fn scanner_string_literal_multi_line() {
        let toks = Scanner::new("\"a\nb\"").scan_tokens().unwrap();
        assert_eq!(toks[0].ttype, String);
        assert_eq!(toks[0].literal, Some(Literal::Str("a\nb".to_string())));
        // Token's `line` reflects the line where the string ends (book behavior).
        assert_eq!(toks[0].line, 2);
        assert_eq!(toks[1].ttype, Eof);
        assert_eq!(toks[1].line, 2);
    }

    #[test]
    fn scanner_unterminated_string_errors() {
        let err = Scanner::new("\"abc").scan_tokens().unwrap_err();
        assert!(err.contains("Unterminated"), "got: {err}");
        assert!(err.contains("[line 1]"), "got: {err}");
    }

    #[test]
    fn scanner_number_int_and_float() {
        let toks = Scanner::new("123 45.67").scan_tokens().unwrap();
        assert_eq!(toks.len(), 3);
        assert_eq!(toks[0].ttype, Number);
        assert_eq!(toks[0].literal, Some(Literal::Num(123.0)));
        assert_eq!(toks[1].ttype, Number);
        assert_eq!(toks[1].literal, Some(Literal::Num(45.67)));
        assert_eq!(toks[2].ttype, Eof);
    }

    #[test]
    fn scanner_identifier_and_keyword() {
        let toks = Scanner::new("orchid or").scan_tokens().unwrap();
        assert_eq!(toks.len(), 3);
        assert_eq!(toks[0].ttype, Identifier);
        assert_eq!(toks[0].lexeme, "orchid");
        assert_eq!(toks[1].ttype, Or);
        assert_eq!(toks[1].lexeme, "or");
    }

    #[test]
    fn scanner_multi_line_tracks_line() {
        let toks = Scanner::new("\n\n+").scan_tokens().unwrap();
        // tokens: Plus (on line 3), Eof (on line 3).
        assert_eq!(toks[0].ttype, Plus);
        assert_eq!(toks[0].line, 3);
        assert_eq!(toks[1].ttype, Eof);
        assert_eq!(toks[1].line, 3);
    }

    #[test]
    fn scanner_rejects_stray_char_with_line() {
        let err = Scanner::new("@").scan_tokens().unwrap_err();
        assert!(err.contains("[line 1]"), "got: {err}");
        assert!(err.contains("'@'"), "got: {err}");
    }

    #[test]
    fn scanner_empty_produces_only_eof() {
        let toks = Scanner::new("").scan_tokens().unwrap();
        assert_eq!(toks.len(), 1);
        assert_eq!(toks[0].ttype, Eof);
        assert_eq!(toks[0].line, 1);
        assert_eq!(toks[0].lexeme, "");
    }

    // Extra coverage: book edge cases worth pinning down.
    #[test]
    fn scanner_trailing_dot_is_separate_token() {
        // `5.` should lex as Number(5) then Dot, not Number(5.0).
        let toks = Scanner::new("5.").scan_tokens().unwrap();
        assert_eq!(toks[0].ttype, Number);
        assert_eq!(toks[0].literal, Some(Literal::Num(5.0)));
        assert_eq!(toks[1].ttype, Dot);
        assert_eq!(toks[2].ttype, Eof);
    }

    #[test]
    fn scanner_leading_dot_is_separate_token() {
        // `.5` should lex as Dot then Number(5).
        let toks = Scanner::new(".5").scan_tokens().unwrap();
        assert_eq!(toks[0].ttype, Dot);
        assert_eq!(toks[1].ttype, Number);
        assert_eq!(toks[1].literal, Some(Literal::Num(5.0)));
    }

    #[test]
    fn scanner_all_keywords_recognised() {
        let src = "and class else false fun for if nil or print return super this true var while";
        assert_eq!(
            types(src),
            vec![
                And, Class, Else, False, Fun, For, If, Nil, Or, Print, Return, Super, This, True,
                Var, While, Eof,
            ]
        );
    }
}
