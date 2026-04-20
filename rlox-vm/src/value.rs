//! Runtime values. For M4 we only model the unboxed scalars (`Nil`, `Bool`,
//! `Number`). Heap-allocated values (strings/functions/classes) land in M6.

use std::fmt;

/// Lox runtime value. The tagged union mirrors the book's `Value` struct (ch.
/// 14-15), except the heap variant is deferred until M6 when GC is in place.
#[derive(Debug, Clone, Copy)]
pub enum Value {
    Nil,
    Bool(bool),
    Number(f64),
    // TODO(M6): Obj(*mut crate::object::Obj) — heap-allocated strings/fns/classes.
}

impl Value {
    /// Lox truthiness rule: only `nil` and `false` are falsey.
    pub fn is_falsey(&self) -> bool {
        matches!(self, Value::Nil | Value::Bool(false))
    }

    /// Book's `valuesEqual`: cross-variant comparisons are always `false`.
    /// Numeric equality uses IEEE semantics (NaN != NaN) — matches clox.
    pub fn equals(&self, other: &Value) -> bool {
        match (self, other) {
            (Value::Nil, Value::Nil) => true,
            (Value::Bool(a), Value::Bool(b)) => a == b,
            (Value::Number(a), Value::Number(b)) => a == b,
            _ => false,
        }
    }
}

impl fmt::Display for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Value::Nil => write!(f, "nil"),
            Value::Bool(b) => write!(f, "{b}"),
            Value::Number(n) => {
                // Match book output: integral numbers print without a decimal
                // point (`42` rather than `42.0`); non-integrals use default
                // f64 formatting.
                if n.is_finite() && *n == n.trunc() {
                    write!(f, "{n:.0}")
                } else {
                    write!(f, "{n}")
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn value_display_nil() {
        assert_eq!(format!("{}", Value::Nil), "nil");
    }

    #[test]
    fn value_display_bool() {
        assert_eq!(format!("{}", Value::Bool(true)), "true");
        assert_eq!(format!("{}", Value::Bool(false)), "false");
    }

    #[test]
    fn value_display_integral_number() {
        assert_eq!(format!("{}", Value::Number(42.0)), "42");
        assert_eq!(format!("{}", Value::Number(-7.0)), "-7");
        assert_eq!(format!("{}", Value::Number(0.0)), "0");
    }

    #[test]
    fn value_display_fractional_number() {
        assert_eq!(format!("{}", Value::Number(1.5)), "1.5");
        assert_eq!(format!("{}", Value::Number(2.25)), "2.25");
    }

    #[test]
    fn value_equals_same_variant() {
        assert!(Value::Nil.equals(&Value::Nil));
        assert!(Value::Bool(true).equals(&Value::Bool(true)));
        assert!(!Value::Bool(true).equals(&Value::Bool(false)));
        assert!(Value::Number(1.0).equals(&Value::Number(1.0)));
        assert!(!Value::Number(1.0).equals(&Value::Number(2.0)));
    }

    #[test]
    fn value_equals_different_variants_never_equal() {
        assert!(!Value::Nil.equals(&Value::Bool(false)));
        assert!(!Value::Nil.equals(&Value::Number(0.0)));
        assert!(!Value::Bool(false).equals(&Value::Number(0.0)));
        assert!(!Value::Bool(true).equals(&Value::Number(1.0)));
    }

    #[test]
    fn value_falsey_rules() {
        assert!(Value::Nil.is_falsey());
        assert!(Value::Bool(false).is_falsey());
        assert!(!Value::Bool(true).is_falsey());
        assert!(!Value::Number(0.0).is_falsey(), "0 is truthy in Lox");
        assert!(!Value::Number(1.0).is_falsey());
    }
}
