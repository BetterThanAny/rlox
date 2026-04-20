//! Runtime values. M5 adds `Str(Rc<String>)` and `Function(Rc<ObjFunction>)`
//! / `Closure(Rc<Closure>)` variants alongside the M4 scalars.
//!
//! Tradeoff: we defer a proper heap-managed `Obj` tree + GC to M6. For M5,
//! shared ownership via `Rc` is sufficient — the only cycle risk is
//! closure-captures-self, which `Crafting Interpreters` chapter 25 also dodges
//! (the GC's sole job there is reclaiming unreachable closures; we simply leak
//! them until M6 lands mark-sweep).

use std::fmt;
use std::rc::Rc;

use crate::chunk::Chunk;

/// Compiled function prototype. Produced by the compiler, referenced from
/// `Value::Function`, and embedded inside a `Closure` when upvalues are bound.
///
/// Corresponds to `ObjFunction` in book chapter 24.
#[derive(Debug)]
pub struct ObjFunction {
    /// `None` for the top-level `<script>` function.
    pub name: Option<Rc<String>>,
    pub arity: usize,
    pub chunk: Chunk,
    pub upvalue_count: usize,
}

impl ObjFunction {
    pub fn new(name: Option<Rc<String>>) -> Self {
        Self {
            name,
            arity: 0,
            chunk: Chunk::new(),
            upvalue_count: 0,
        }
    }

    pub fn display_name(&self) -> &str {
        match &self.name {
            Some(n) => n.as_str(),
            None => "script",
        }
    }
}

/// Runtime upvalue cell. `Rc<RefCell<Value>>` lets closures share mutable
/// storage for captured locals — the clox mark-and-sweep upvalue list collapses
/// into a single heap cell here. Upgrading to real open/closed upvalues lands
/// with GC in M6.
pub type UpvalueCell = std::rc::Rc<std::cell::RefCell<Value>>;

/// Runtime closure: a function plus the upvalue cells it closes over.
/// Corresponds to `ObjClosure` in book chapter 25.
#[derive(Debug)]
pub struct Closure {
    pub function: Rc<ObjFunction>,
    pub upvalues: Vec<UpvalueCell>,
}

impl Closure {
    pub fn new(function: Rc<ObjFunction>) -> Self {
        let upvalues = Vec::with_capacity(function.upvalue_count);
        Self { function, upvalues }
    }
}

/// Native function pointer. Signature matches book chapter 24 natives.
pub type NativeFn = fn(&[Value]) -> Value;

/// Lox runtime value.
#[derive(Debug, Clone)]
pub enum Value {
    Nil,
    Bool(bool),
    Number(f64),
    /// Interned-for-free string. M6 will swap this for a GC-managed `ObjString`.
    Str(Rc<String>),
    /// Raw function prototype (pre-closure wrapping). Used for constant-pool
    /// entries produced by the compiler.
    Function(Rc<ObjFunction>),
    /// Executable closure (function + captured upvalues).
    Closure(Rc<Closure>),
    /// Native callable.
    Native(NativeFn),
}

impl Value {
    /// Lox truthiness rule: only `nil` and `false` are falsey.
    pub fn is_falsey(&self) -> bool {
        matches!(self, Value::Nil | Value::Bool(false))
    }

    /// Book's `valuesEqual`. Strings compare by content (cheap via `Rc` fast
    /// path). Functions / closures / natives compare by `Rc` identity —
    /// matches clox pointer equality.
    pub fn equals(&self, other: &Value) -> bool {
        match (self, other) {
            (Value::Nil, Value::Nil) => true,
            (Value::Bool(a), Value::Bool(b)) => a == b,
            (Value::Number(a), Value::Number(b)) => a == b,
            (Value::Str(a), Value::Str(b)) => Rc::ptr_eq(a, b) || **a == **b,
            (Value::Function(a), Value::Function(b)) => Rc::ptr_eq(a, b),
            (Value::Closure(a), Value::Closure(b)) => Rc::ptr_eq(a, b),
            (Value::Native(a), Value::Native(b)) => (*a as usize) == (*b as usize),
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
            Value::Str(s) => write!(f, "{s}"),
            Value::Function(func) => write!(f, "<fn {}>", func.display_name()),
            Value::Closure(c) => write!(f, "<fn {}>", c.function.display_name()),
            Value::Native(_) => write!(f, "<native fn>"),
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
    fn value_display_string_is_unquoted() {
        let s = Value::Str(Rc::new("hi".to_string()));
        assert_eq!(format!("{s}"), "hi");
    }

    #[test]
    fn value_display_function_shows_name() {
        let mut func = ObjFunction::new(Some(Rc::new("fib".to_string())));
        func.arity = 1;
        let v = Value::Function(Rc::new(func));
        assert_eq!(format!("{v}"), "<fn fib>");
    }

    #[test]
    fn value_display_function_without_name_is_script() {
        let func = ObjFunction::new(None);
        let v = Value::Function(Rc::new(func));
        assert_eq!(format!("{v}"), "<fn script>");
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
    fn value_equals_strings_by_content() {
        let a = Value::Str(Rc::new("hello".to_string()));
        let b = Value::Str(Rc::new("hello".to_string()));
        let c = Value::Str(Rc::new("world".to_string()));
        assert!(a.equals(&b));
        assert!(!a.equals(&c));
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
        assert!(!Value::Str(Rc::new(String::new())).is_falsey());
    }
}
