//! Shared error type for the rlox tree-walking interpreter.
//!
//! Two variants match the two pipeline stages where errors originate as of
//! M8: compile-time (scanner/parser/resolver) and runtime (interpreter).
//! Display format follows *Crafting Interpreters* (Nystrom, ch. 4, 7, 13):
//!   `[line N] Error<loc>: <msg>`   (compile-time)
//!   `<msg>\n[line N] in script`    (runtime — mimics clox single-frame trace)
//!
//! The `loc` field on `Syntax` is expected to carry its own leading formatting
//! (e.g. `" at '('"`, `" at end"`, or `""`) so the book's exact output is
//! preserved byte-for-byte.

use thiserror::Error;

#[derive(Debug, Error, Clone, PartialEq)]
pub enum LoxError {
    #[error("[line {line}] Error{loc}: {msg}")]
    Syntax {
        line: usize,
        loc: String,
        msg: String,
    },

    #[error("{msg}\n[line {line}] in script")]
    Runtime { line: usize, msg: String },
}

impl LoxError {
    pub fn syntax(line: usize, loc: impl Into<String>, msg: impl Into<String>) -> Self {
        Self::Syntax {
            line,
            loc: loc.into(),
            msg: msg.into(),
        }
    }

    pub fn runtime(line: usize, msg: impl Into<String>) -> Self {
        Self::Runtime {
            line,
            msg: msg.into(),
        }
    }
}

#[cfg(test)]
mod error_tests {
    use super::*;

    #[test]
    fn error_syntax_display_matches_book() {
        let e = LoxError::syntax(3, " at '('", "expected expression");
        assert_eq!(e.to_string(), "[line 3] Error at '(': expected expression");
    }

    #[test]
    fn error_syntax_display_with_empty_loc() {
        let e = LoxError::syntax(1, "", "bad");
        assert_eq!(e.to_string(), "[line 1] Error: bad");
    }

    #[test]
    fn error_syntax_display_at_end() {
        let e = LoxError::syntax(42, " at end", "expected ';'");
        assert_eq!(e.to_string(), "[line 42] Error at end: expected ';'");
    }

    #[test]
    fn error_runtime_display() {
        let e = LoxError::runtime(5, "divide by zero");
        assert_eq!(e.to_string(), "divide by zero\n[line 5] in script");
    }

    #[test]
    fn error_clone_and_eq() {
        let a = LoxError::syntax(7, " at 'foo'", "unexpected token");
        let b = a.clone();
        assert_eq!(a, b);

        let r1 = LoxError::runtime(9, "type mismatch");
        let r2 = LoxError::runtime(9, "type mismatch");
        assert_eq!(r1, r2);
        assert_eq!(r1, r1.clone());

        // Sanity: Syntax vs Runtime are not equal.
        assert_ne!(LoxError::runtime(1, "x"), LoxError::syntax(1, "", "x"));
    }
}
