//! On-demand Lox scanner. Follows *Crafting Interpreters* chapter 16 — the
//! compiler drives the scanner one token at a time rather than materialising
//! a full `Vec<Token>` up front.
//!
//! Unlike the `rlox-tree` scanner, errors are reported out-of-band by
//! producing a sentinel `TokenType::Error` token whose `lexeme` carries the
//! human-readable diagnostic. The compiler inspects `Error` tokens inside its
//! normal `advance` flow and records them in its parser state.

/// Token kind produced by the scanner. Mirrors the rlox-tree `TokenType` plus
/// two sentinels (`Error`, `Eof`) as the book's clox does.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TokenType {
    // Single-character.
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

    // One or two character.
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

    Error,
    Eof,
}

/// Scanner output.
#[derive(Debug, Clone)]
pub struct Token {
    pub ttype: TokenType,
    /// Owned lexeme (or error message when `ttype == Error`). Owned rather
    /// than borrowed so the compiler can stash previous tokens past the
    /// scanner's lifetime without lifetime plumbing.
    pub lexeme: String,
    pub line: usize,
}

impl Token {
    pub fn synthetic(ttype: TokenType, lexeme: impl Into<String>) -> Self {
        Self {
            ttype,
            lexeme: lexeme.into(),
            line: 0,
        }
    }
}

/// On-demand Lox scanner. Each call to [`Scanner::scan_token`] emits exactly
/// one token, ending with `TokenType::Eof` forever.
pub struct Scanner<'src> {
    source: &'src str,
    bytes: &'src [u8],
    /// Byte offset of the first char of the lexeme currently being scanned.
    start: usize,
    /// Byte offset of the next char to consume.
    current: usize,
    /// 1-based current line number.
    line: usize,
}

impl<'src> Scanner<'src> {
    pub fn new(source: &'src str) -> Self {
        Self {
            source,
            bytes: source.as_bytes(),
            start: 0,
            current: 0,
            line: 1,
        }
    }

    /// Produce the next token. Post-EOF, keeps returning `Eof`.
    pub fn scan_token(&mut self) -> Token {
        self.skip_whitespace();
        self.start = self.current;

        if self.is_at_end() {
            return self.make_token(TokenType::Eof);
        }

        let c = self.advance();

        if is_alpha(c) {
            return self.identifier();
        }
        if is_digit(c) {
            return self.number();
        }

        match c {
            b'(' => self.make_token(TokenType::LeftParen),
            b')' => self.make_token(TokenType::RightParen),
            b'{' => self.make_token(TokenType::LeftBrace),
            b'}' => self.make_token(TokenType::RightBrace),
            b',' => self.make_token(TokenType::Comma),
            b'.' => self.make_token(TokenType::Dot),
            b'-' => self.make_token(TokenType::Minus),
            b'+' => self.make_token(TokenType::Plus),
            b';' => self.make_token(TokenType::Semicolon),
            b'*' => self.make_token(TokenType::Star),
            b'/' => self.make_token(TokenType::Slash),
            b'!' => {
                let t = if self.match_byte(b'=') {
                    TokenType::BangEqual
                } else {
                    TokenType::Bang
                };
                self.make_token(t)
            }
            b'=' => {
                let t = if self.match_byte(b'=') {
                    TokenType::EqualEqual
                } else {
                    TokenType::Equal
                };
                self.make_token(t)
            }
            b'<' => {
                let t = if self.match_byte(b'=') {
                    TokenType::LessEqual
                } else {
                    TokenType::Less
                };
                self.make_token(t)
            }
            b'>' => {
                let t = if self.match_byte(b'=') {
                    TokenType::GreaterEqual
                } else {
                    TokenType::Greater
                };
                self.make_token(t)
            }
            b'"' => self.string(),
            _ => self.error_token("Unexpected character."),
        }
    }

    // ---------- sub-scanners ----------

    fn string(&mut self) -> Token {
        while !self.is_at_end() && self.peek() != b'"' {
            if self.peek() == b'\n' {
                self.line += 1;
            }
            self.current += 1;
        }
        if self.is_at_end() {
            return self.error_token("Unterminated string.");
        }
        // Consume the closing quote.
        self.current += 1;
        self.make_token(TokenType::String)
    }

    fn number(&mut self) -> Token {
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
        self.make_token(TokenType::Number)
    }

    fn identifier(&mut self) -> Token {
        while !self.is_at_end() && is_alphanumeric(self.peek()) {
            self.current += 1;
        }
        let ttype = self.identifier_type();
        self.make_token(ttype)
    }

    /// Book chapter 16 "trie" — we keep it simple with a match on the leading
    /// byte and a delegate to `check_keyword`.
    fn identifier_type(&self) -> TokenType {
        let lex = &self.source[self.start..self.current];
        let bytes = lex.as_bytes();
        if bytes.is_empty() {
            return TokenType::Identifier;
        }
        match bytes[0] {
            b'a' => self.check_keyword(1, "nd", TokenType::And),
            b'c' => self.check_keyword(1, "lass", TokenType::Class),
            b'e' => self.check_keyword(1, "lse", TokenType::Else),
            b'i' => self.check_keyword(1, "f", TokenType::If),
            b'n' => self.check_keyword(1, "il", TokenType::Nil),
            b'o' => self.check_keyword(1, "r", TokenType::Or),
            b'p' => self.check_keyword(1, "rint", TokenType::Print),
            b'r' => self.check_keyword(1, "eturn", TokenType::Return),
            b's' => self.check_keyword(1, "uper", TokenType::Super),
            b'v' => self.check_keyword(1, "ar", TokenType::Var),
            b'w' => self.check_keyword(1, "hile", TokenType::While),
            b'f' => {
                if bytes.len() > 1 {
                    match bytes[1] {
                        b'a' => self.check_keyword(2, "lse", TokenType::False),
                        b'o' => self.check_keyword(2, "r", TokenType::For),
                        b'u' => self.check_keyword(2, "n", TokenType::Fun),
                        _ => TokenType::Identifier,
                    }
                } else {
                    TokenType::Identifier
                }
            }
            b't' => {
                if bytes.len() > 1 {
                    match bytes[1] {
                        b'h' => self.check_keyword(2, "is", TokenType::This),
                        b'r' => self.check_keyword(2, "ue", TokenType::True),
                        _ => TokenType::Identifier,
                    }
                } else {
                    TokenType::Identifier
                }
            }
            _ => TokenType::Identifier,
        }
    }

    fn check_keyword(&self, skip: usize, rest: &str, kind: TokenType) -> TokenType {
        let lex = &self.source[self.start..self.current];
        if lex.len() == skip + rest.len() && &lex[skip..] == rest {
            kind
        } else {
            TokenType::Identifier
        }
    }

    // ---------- whitespace + comment skip ----------

    fn skip_whitespace(&mut self) {
        loop {
            if self.is_at_end() {
                return;
            }
            let c = self.peek();
            match c {
                b' ' | b'\r' | b'\t' => {
                    self.current += 1;
                }
                b'\n' => {
                    self.line += 1;
                    self.current += 1;
                }
                b'/' => {
                    if self.peek_next() == b'/' {
                        while !self.is_at_end() && self.peek() != b'\n' {
                            self.current += 1;
                        }
                    } else {
                        return;
                    }
                }
                _ => return,
            }
        }
    }

    // ---------- low-level helpers ----------

    fn is_at_end(&self) -> bool {
        self.current >= self.bytes.len()
    }

    fn advance(&mut self) -> u8 {
        let b = self.bytes[self.current];
        self.current += 1;
        b
    }

    fn peek(&self) -> u8 {
        if self.is_at_end() {
            0
        } else {
            self.bytes[self.current]
        }
    }

    fn peek_next(&self) -> u8 {
        if self.current + 1 >= self.bytes.len() {
            0
        } else {
            self.bytes[self.current + 1]
        }
    }

    fn match_byte(&mut self, expected: u8) -> bool {
        if self.is_at_end() || self.bytes[self.current] != expected {
            return false;
        }
        self.current += 1;
        true
    }

    fn make_token(&self, ttype: TokenType) -> Token {
        Token {
            ttype,
            lexeme: self.source[self.start..self.current].to_string(),
            line: self.line,
        }
    }

    fn error_token(&self, msg: &str) -> Token {
        Token {
            ttype: TokenType::Error,
            lexeme: msg.to_string(),
            line: self.line,
        }
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

#[cfg(test)]
mod scanner_tests {
    use super::*;

    fn collect_types(src: &str) -> Vec<TokenType> {
        let mut s = Scanner::new(src);
        let mut out = Vec::new();
        loop {
            let tok = s.scan_token();
            let done = tok.ttype == TokenType::Eof;
            out.push(tok.ttype);
            if done {
                break;
            }
        }
        out
    }

    #[test]
    fn scanner_single_char_tokens() {
        use TokenType::*;
        assert_eq!(
            collect_types("(){},.-+;*/"),
            vec![
                LeftParen, RightParen, LeftBrace, RightBrace, Comma, Dot, Minus, Plus, Semicolon,
                Star, Slash, Eof,
            ]
        );
    }

    #[test]
    fn scanner_two_char_operators() {
        use TokenType::*;
        assert_eq!(
            collect_types("! != = == < <= > >="),
            vec![
                Bang,
                BangEqual,
                Equal,
                EqualEqual,
                Less,
                LessEqual,
                Greater,
                GreaterEqual,
                Eof,
            ]
        );
    }

    #[test]
    fn scanner_string_literal_round_trips_lexeme_including_quotes() {
        let mut s = Scanner::new("\"hello\"");
        let tok = s.scan_token();
        assert_eq!(tok.ttype, TokenType::String);
        // The lexeme includes the surrounding quotes so the compiler can
        // trim them when emitting the constant.
        assert_eq!(tok.lexeme, "\"hello\"");
        assert_eq!(s.scan_token().ttype, TokenType::Eof);
    }

    #[test]
    fn scanner_number_int_and_float() {
        let mut s = Scanner::new("123 45.67");
        let a = s.scan_token();
        assert_eq!(a.ttype, TokenType::Number);
        assert_eq!(a.lexeme, "123");
        let b = s.scan_token();
        assert_eq!(b.ttype, TokenType::Number);
        assert_eq!(b.lexeme, "45.67");
    }

    #[test]
    fn scanner_identifier_vs_keyword() {
        let mut s = Scanner::new("orchid or");
        let t1 = s.scan_token();
        assert_eq!(t1.ttype, TokenType::Identifier);
        assert_eq!(t1.lexeme, "orchid");
        let t2 = s.scan_token();
        assert_eq!(t2.ttype, TokenType::Or);
    }

    #[test]
    fn scanner_all_keywords_recognised() {
        use TokenType::*;
        let src = "and class else false fun for if nil or print return super this true var while";
        assert_eq!(
            collect_types(src),
            vec![
                And, Class, Else, False, Fun, For, If, Nil, Or, Print, Return, Super, This, True,
                Var, While, Eof,
            ]
        );
    }

    #[test]
    fn scanner_line_comment_skipped() {
        use TokenType::*;
        assert_eq!(collect_types("// hi\n+"), vec![Plus, Eof]);
    }

    #[test]
    fn scanner_unterminated_string_produces_error_token() {
        let mut s = Scanner::new("\"nope");
        let tok = s.scan_token();
        assert_eq!(tok.ttype, TokenType::Error);
        assert!(tok.lexeme.contains("Unterminated"));
    }

    #[test]
    fn scanner_unexpected_char_produces_error_token() {
        let mut s = Scanner::new("@");
        let tok = s.scan_token();
        assert_eq!(tok.ttype, TokenType::Error);
        assert!(tok.lexeme.contains("Unexpected"));
    }

    #[test]
    fn scanner_multi_line_tracks_line() {
        let mut s = Scanner::new("\n\n+");
        let tok = s.scan_token();
        assert_eq!(tok.ttype, TokenType::Plus);
        assert_eq!(tok.line, 3);
    }

    #[test]
    fn scanner_eof_is_idempotent() {
        let mut s = Scanner::new("");
        assert_eq!(s.scan_token().ttype, TokenType::Eof);
        assert_eq!(s.scan_token().ttype, TokenType::Eof);
    }
}
