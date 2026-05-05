//! Runtime values. M5 adds `Str(Rc<String>)` and `Function(Rc<ObjFunction>)`
//! / `Closure(Rc<Closure>)` variants alongside the M4 scalars. M6 part A adds
//! `Class`, `Instance`, `BoundMethod` wrapped in `Rc` so methods and instance
//! fields can share ownership without a dedicated heap walk.
//!
//! Tradeoff: we defer a proper heap-managed `Obj` tree + GC to M6 part B. For
//! now, shared ownership via `Rc` is sufficient — the only cycle risk is
//! closure-captures-self and instance-references-bound-method-back-to-self,
//! which `Crafting Interpreters` chapter 25 also dodges (the GC's sole job
//! there is reclaiming unreachable closures; we simply leak them until the
//! mark-sweep refactor lands).

use std::cell::RefCell;
use std::collections::HashMap;
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

/// Host-provided callable metadata. Clox stores native functions as a bare
/// function pointer, but the VM needs arity metadata to report call-site
/// argument errors consistently with user functions and the tree-walk runtime.
#[derive(Debug, Clone)]
pub struct NativeFunction {
    pub name: Rc<String>,
    pub arity: usize,
    pub function: NativeFn,
}

/// Runtime class object. Holds the method table — book chapter 27/28 keeps
/// methods in a hash keyed by name; inheritance copies the parent's methods
/// at `OP_INHERIT` time so resolution never walks the chain.
///
/// M6-GC: replaces `Rc` with GC-managed raw ptr.
#[derive(Debug)]
pub struct ObjClass {
    pub name: Rc<String>,
    /// Method table: method name → `Closure` (methods are closures with
    /// `this` pre-wired into slot 0 of their locals).
    pub methods: RefCell<HashMap<Rc<String>, Rc<Closure>>>,
}

impl ObjClass {
    pub fn new(name: Rc<String>) -> Self {
        Self {
            name,
            methods: RefCell::new(HashMap::new()),
        }
    }

    /// Look up a method by name. Because `OP_INHERIT` copies the parent's
    /// methods into the child, this never has to walk a chain — a single
    /// hash-map probe suffices.
    pub fn find_method(&self, name: &Rc<String>) -> Option<Rc<Closure>> {
        self.methods.borrow().get(name).cloned()
    }
}

/// Runtime instance of a class. Field storage is per-instance.
///
/// M6-GC: replaces `Rc` with GC-managed raw ptr.
#[derive(Debug)]
pub struct ObjInstance {
    pub class: Rc<ObjClass>,
    pub fields: RefCell<HashMap<Rc<String>, Value>>,
}

impl ObjInstance {
    pub fn new(class: Rc<ObjClass>) -> Self {
        Self {
            class,
            fields: RefCell::new(HashMap::new()),
        }
    }
}

/// A method retrieved off an instance via `instance.method`. Stores the
/// receiver alongside the closure so a later `OP_CALL` slides the receiver
/// into slot 0 before invoking the method's body.
///
/// M6-GC: replaces `Rc` with GC-managed raw ptr.
#[derive(Debug)]
pub struct ObjBoundMethod {
    /// The instance value (always a `Value::Instance`).
    pub receiver: Value,
    pub method: Rc<Closure>,
}

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
    Native(Rc<NativeFunction>),
    /// Class object — callable to allocate an instance.
    Class(Rc<ObjClass>),
    /// Live instance of a class.
    Instance(Rc<ObjInstance>),
    /// Method bound to a receiver. Callable like a closure.
    BoundMethod(Rc<ObjBoundMethod>),
}

impl Value {
    /// Lox truthiness rule: only `nil` and `false` are falsey. Classes,
    /// instances, and bound methods are all truthy.
    pub fn is_falsey(&self) -> bool {
        matches!(self, Value::Nil | Value::Bool(false))
    }

    /// Book's `valuesEqual`. Strings compare by content (cheap via `Rc` fast
    /// path). Functions / closures / natives / classes / instances / bound
    /// methods compare by `Rc` identity — matches clox pointer equality.
    pub fn equals(&self, other: &Value) -> bool {
        match (self, other) {
            (Value::Nil, Value::Nil) => true,
            (Value::Bool(a), Value::Bool(b)) => a == b,
            (Value::Number(a), Value::Number(b)) => a == b,
            (Value::Str(a), Value::Str(b)) => Rc::ptr_eq(a, b) || **a == **b,
            (Value::Function(a), Value::Function(b)) => Rc::ptr_eq(a, b),
            (Value::Closure(a), Value::Closure(b)) => Rc::ptr_eq(a, b),
            (Value::Native(a), Value::Native(b)) => Rc::ptr_eq(a, b),
            (Value::Class(a), Value::Class(b)) => Rc::ptr_eq(a, b),
            (Value::Instance(a), Value::Instance(b)) => Rc::ptr_eq(a, b),
            (Value::BoundMethod(a), Value::BoundMethod(b)) => Rc::ptr_eq(a, b),
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
            Value::Class(c) => write!(f, "{}", c.name),
            Value::Instance(i) => write!(f, "{} instance", i.class.name),
            Value::BoundMethod(bm) => write!(f, "<fn {}>", bm.method.function.display_name()),
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

    #[test]
    fn value_display_class_shows_name() {
        let class = Rc::new(ObjClass::new(Rc::new("Animal".to_string())));
        let v = Value::Class(class);
        assert_eq!(format!("{v}"), "Animal");
    }

    #[test]
    fn value_display_instance_shows_class_name_plus_instance() {
        let class = Rc::new(ObjClass::new(Rc::new("Animal".to_string())));
        let inst = Rc::new(ObjInstance::new(class));
        let v = Value::Instance(inst);
        assert_eq!(format!("{v}"), "Animal instance");
    }

    #[test]
    fn value_display_bound_method_shows_method_name() {
        let class = Rc::new(ObjClass::new(Rc::new("Animal".to_string())));
        let inst = Rc::new(ObjInstance::new(class));
        let func = ObjFunction::new(Some(Rc::new("speak".to_string())));
        let closure = Rc::new(Closure::new(Rc::new(func)));
        let bm = Rc::new(ObjBoundMethod {
            receiver: Value::Instance(inst),
            method: closure,
        });
        assert_eq!(format!("{}", Value::BoundMethod(bm)), "<fn speak>");
    }

    #[test]
    fn value_equals_class_uses_rc_identity() {
        let a = Rc::new(ObjClass::new(Rc::new("A".to_string())));
        let b = Rc::new(ObjClass::new(Rc::new("A".to_string())));
        assert!(Value::Class(a.clone()).equals(&Value::Class(a.clone())));
        assert!(!Value::Class(a).equals(&Value::Class(b)));
    }

    #[test]
    fn class_find_method_returns_inserted_closure() {
        let class = ObjClass::new(Rc::new("C".to_string()));
        let func = Rc::new(ObjFunction::new(Some(Rc::new("m".to_string()))));
        let closure = Rc::new(Closure::new(func));
        let key = Rc::new("m".to_string());
        class.methods.borrow_mut().insert(key.clone(), closure);
        assert!(class.find_method(&key).is_some());
        assert!(class.find_method(&Rc::new("nope".to_string())).is_none());
    }

    #[test]
    fn instance_is_truthy() {
        let class = Rc::new(ObjClass::new(Rc::new("A".to_string())));
        let inst = Rc::new(ObjInstance::new(class));
        assert!(!Value::Instance(inst).is_falsey());
    }
}
