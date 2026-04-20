//! Runtime values for the tree-walking Lox interpreter.
//!
//! Mirrors Nystrom's `Object` / `LoxCallable` / `LoxClass` / `LoxInstance`
//! hierarchy (Crafting Interpreters, chs. 7, 10, 12, 13) as a Rust enum plus
//! trait-object callables.
//!
//! This file is written to compile *standalone* in Wave 2 — the concrete
//! `Environment` type lands in Wave 3 and the concrete `Interpreter` in Wave 4.
//! To keep `value.rs` decoupled, the bridge to the rest of the interpreter is
//! the trait pair [`Executor`] + [`EnvironmentLike`]: the interpreter
//! implements `Executor`, the environment implements `EnvironmentLike`, and
//! `LoxFunction` / `LoxClass` talk to them only through `dyn` objects.

use std::cell::RefCell;
use std::collections::HashMap;
use std::fmt;
use std::rc::Rc;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::ast::Stmt;
use crate::error::LoxError;
use crate::token::{Literal, Token};

// ---------------------------------------------------------------------------
// Forward-compatible bridge traits.
// ---------------------------------------------------------------------------

/// Interface the interpreter must provide so `LoxFunction` / `LoxClass` can
/// execute their bodies without knowing the concrete `Interpreter` type.
///
/// Wave 4 will `impl Executor for Interpreter`.
pub trait Executor {
    /// Execute a block of statements in the supplied environment.
    fn execute_block_with(
        &mut self,
        stmts: &[Stmt],
        environment_rc: Rc<RefCell<dyn EnvironmentLike>>,
    ) -> Result<(), LoxError>;

    /// Access the interpreter's globals environment (handy for native fns).
    fn globals(&self) -> Rc<RefCell<dyn EnvironmentLike>>;
}

/// Thin trait capturing the minimum operations `LoxFunction` needs on an
/// environment. Wave 3's concrete `Environment` will implement this.
///
/// `Debug` is a supertrait so `LoxFunction` (which holds a
/// `Rc<RefCell<dyn EnvironmentLike>>`) can still derive `Debug`.
pub trait EnvironmentLike: fmt::Debug {
    /// Define (or redefine) a variable in the current scope.
    fn define(&mut self, name: String, value: LoxValue);
}

// ---------------------------------------------------------------------------
// LoxValue.
// ---------------------------------------------------------------------------

/// Runtime value — the Rust counterpart of the book's `Object`.
#[derive(Debug, Clone)]
pub enum LoxValue {
    Nil,
    Bool(bool),
    Number(f64),
    Str(Rc<String>),
    Callable(Rc<dyn LoxCallable>),
    Class(Rc<LoxClass>),
    Instance(Rc<RefCell<LoxInstance>>),
}

impl From<Literal> for LoxValue {
    fn from(lit: Literal) -> Self {
        match lit {
            Literal::Nil => LoxValue::Nil,
            Literal::Bool(b) => LoxValue::Bool(b),
            Literal::Num(n) => LoxValue::Number(n),
            Literal::Str(s) => LoxValue::Str(Rc::new(s)),
        }
    }
}

impl LoxValue {
    /// Lox truthiness: only `nil` and `false` are falsey; everything else
    /// (including `0`, `""`, empty instances) is truthy.
    pub fn is_truthy(&self) -> bool {
        !matches!(self, LoxValue::Nil | LoxValue::Bool(false))
    }

    /// Lox `==` semantics. Nil equals only Nil. Numbers use IEEE-754
    /// equality (so `NaN != NaN`). Strings compare by contents. Callables,
    /// classes, and instances compare by pointer identity. Different variants
    /// are never equal.
    pub fn equals(&self, other: &LoxValue) -> bool {
        match (self, other) {
            (LoxValue::Nil, LoxValue::Nil) => true,
            (LoxValue::Bool(a), LoxValue::Bool(b)) => a == b,
            (LoxValue::Number(a), LoxValue::Number(b)) => a == b,
            (LoxValue::Str(a), LoxValue::Str(b)) => a == b,
            (LoxValue::Callable(a), LoxValue::Callable(b)) => Rc::ptr_eq(a, b),
            (LoxValue::Class(a), LoxValue::Class(b)) => Rc::ptr_eq(a, b),
            (LoxValue::Instance(a), LoxValue::Instance(b)) => Rc::ptr_eq(a, b),
            _ => false,
        }
    }
}

impl fmt::Display for LoxValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LoxValue::Nil => write!(f, "nil"),
            LoxValue::Bool(b) => write!(f, "{b}"),
            LoxValue::Number(n) => {
                // Book rule: integral doubles print without a trailing `.0`.
                if n.is_finite() && *n == n.trunc() {
                    write!(f, "{n:.0}")
                } else {
                    write!(f, "{n}")
                }
            }
            LoxValue::Str(s) => write!(f, "{s}"),
            LoxValue::Callable(c) => write!(f, "<fn {}>", c.name()),
            LoxValue::Class(c) => write!(f, "{}", c.name),
            LoxValue::Instance(i) => write!(f, "{} instance", i.borrow().class.name),
        }
    }
}

// ---------------------------------------------------------------------------
// LoxCallable + NativeFn.
// ---------------------------------------------------------------------------

/// Anything callable at runtime: native fns, user fns, classes.
pub trait LoxCallable: fmt::Debug {
    fn arity(&self) -> usize;
    fn call(&self, executor: &mut dyn Executor, args: Vec<LoxValue>) -> Result<LoxValue, LoxError>;
    fn name(&self) -> &str;
}

/// A host-provided built-in like `clock()`.
pub struct NativeFn {
    pub name_: &'static str,
    pub arity_: usize,
    pub func: fn(&mut dyn Executor, Vec<LoxValue>) -> Result<LoxValue, LoxError>,
}

impl fmt::Debug for NativeFn {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("NativeFn")
            .field("name", &self.name_)
            .field("arity", &self.arity_)
            .finish()
    }
}

impl LoxCallable for NativeFn {
    fn arity(&self) -> usize {
        self.arity_
    }

    fn call(&self, executor: &mut dyn Executor, args: Vec<LoxValue>) -> Result<LoxValue, LoxError> {
        (self.func)(executor, args)
    }

    fn name(&self) -> &str {
        self.name_
    }
}

/// The standard-library `clock()` — seconds since the UNIX epoch as a Number.
pub fn native_clock() -> LoxValue {
    LoxValue::Callable(Rc::new(NativeFn {
        name_: "clock",
        arity_: 0,
        func: |_exec, _args| {
            let secs = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_err(|_| LoxError::runtime(0, "system time before UNIX epoch"))?
                .as_secs_f64();
            Ok(LoxValue::Number(secs))
        },
    }))
}

// ---------------------------------------------------------------------------
// LoxFunction.
// ---------------------------------------------------------------------------

/// A user-declared Lox function or method. `decl` is always a
/// `Stmt::Function { .. }` node produced by the parser.
#[derive(Debug)]
pub struct LoxFunction {
    pub decl: Stmt,
    pub closure: Rc<RefCell<dyn EnvironmentLike>>,
    pub is_initializer: bool,
}

impl LoxCallable for LoxFunction {
    fn arity(&self) -> usize {
        match &self.decl {
            Stmt::Function { params, .. } => params.len(),
            _ => unreachable!("LoxFunction::decl must be Stmt::Function"),
        }
    }

    // TODO(M3): wire to Interpreter via Executor trait.
    // The real implementation builds a fresh environment enclosing
    // `self.closure`, defines each parameter, calls
    // `executor.execute_block_with(body, env)`, unwinds a `Return` sentinel,
    // and (for `is_initializer`) rebinds `this`. Deferred to Wave 4 so this
    // file compiles without a concrete Environment / Interpreter.
    fn call(
        &self,
        _executor: &mut dyn Executor,
        _args: Vec<LoxValue>,
    ) -> Result<LoxValue, LoxError> {
        unimplemented!("LoxFunction::call wired to Interpreter in Wave 4")
    }

    fn name(&self) -> &str {
        match &self.decl {
            Stmt::Function { name, .. } => &name.lexeme,
            _ => unreachable!("LoxFunction::decl must be Stmt::Function"),
        }
    }
}

// ---------------------------------------------------------------------------
// LoxClass.
// ---------------------------------------------------------------------------

/// A runtime class value: name, optional superclass, and method table.
#[derive(Debug)]
pub struct LoxClass {
    pub name: String,
    pub superclass: Option<Rc<LoxClass>>,
    pub methods: HashMap<String, Rc<LoxFunction>>,
}

impl LoxClass {
    /// Look up a method on this class, walking the superclass chain.
    pub fn find_method(&self, name: &str) -> Option<Rc<LoxFunction>> {
        if let Some(m) = self.methods.get(name) {
            return Some(Rc::clone(m));
        }
        if let Some(parent) = &self.superclass {
            return parent.find_method(name);
        }
        None
    }
}

impl LoxCallable for LoxClass {
    fn arity(&self) -> usize {
        self.find_method("init").map(|i| i.arity()).unwrap_or(0)
    }

    // TODO(M3): wire to Interpreter via Executor trait.
    // Real implementation allocates a `LoxInstance`, binds `init` if present
    // with the fresh instance as `this`, invokes it, and returns the instance.
    fn call(
        &self,
        _executor: &mut dyn Executor,
        _args: Vec<LoxValue>,
    ) -> Result<LoxValue, LoxError> {
        unimplemented!("LoxClass::call wired to Interpreter in Wave 4")
    }

    fn name(&self) -> &str {
        &self.name
    }
}

// ---------------------------------------------------------------------------
// LoxInstance.
// ---------------------------------------------------------------------------

/// Runtime instance of a class — open-ended field bag plus class pointer.
#[derive(Debug)]
pub struct LoxInstance {
    pub class: Rc<LoxClass>,
    pub fields: HashMap<String, LoxValue>,
}

impl LoxInstance {
    /// `instance.name` lookup: check fields first, then class methods.
    ///
    /// Method binding (threading `this` into the closure) is performed by the
    /// interpreter in Wave 4; here we merely surface the raw callable so
    /// tests can observe that methods are reachable.
    pub fn get(&self, name: &Token) -> Result<LoxValue, LoxError> {
        if let Some(v) = self.fields.get(&name.lexeme) {
            return Ok(v.clone());
        }
        if let Some(method) = self.class.find_method(&name.lexeme) {
            return Ok(LoxValue::Callable(method));
        }
        Err(LoxError::runtime(
            name.line,
            format!("Undefined property '{}'.", name.lexeme),
        ))
    }

    /// `instance.name = value`.
    pub fn set(&mut self, name: &Token, value: LoxValue) {
        self.fields.insert(name.lexeme.clone(), value);
    }
}

// ---------------------------------------------------------------------------
// Tests.
// ---------------------------------------------------------------------------

#[cfg(test)]
mod value_tests {
    use super::*;
    use crate::token::TokenType;

    fn ident(name: &str) -> Token {
        Token::new(TokenType::Identifier, name, None, 1)
    }

    #[test]
    fn value_display_nil() {
        assert_eq!(format!("{}", LoxValue::Nil), "nil");
    }

    #[test]
    fn value_display_bool() {
        assert_eq!(format!("{}", LoxValue::Bool(true)), "true");
        assert_eq!(format!("{}", LoxValue::Bool(false)), "false");
    }

    #[test]
    fn value_display_number_integral() {
        assert_eq!(format!("{}", LoxValue::Number(42.0)), "42");
        assert_eq!(format!("{}", LoxValue::Number(-3.0)), "-3");
        assert_eq!(format!("{}", LoxValue::Number(0.0)), "0");
    }

    #[test]
    fn value_display_number_fractional() {
        assert_eq!(format!("{}", LoxValue::Number(2.5)), "2.5");
        assert_eq!(format!("{}", LoxValue::Number(-0.25)), "-0.25");
    }

    #[test]
    fn value_display_string() {
        let v = LoxValue::Str(Rc::new("hello".to_string()));
        assert_eq!(format!("{v}"), "hello");
    }

    #[test]
    fn value_is_truthy_rules() {
        // Only Nil and Bool(false) are falsey.
        assert!(!LoxValue::Nil.is_truthy());
        assert!(!LoxValue::Bool(false).is_truthy());
        // Everything else is truthy — including 0 and "".
        assert!(LoxValue::Bool(true).is_truthy());
        assert!(LoxValue::Number(0.0).is_truthy());
        assert!(LoxValue::Number(-1.0).is_truthy());
        assert!(LoxValue::Str(Rc::new(String::new())).is_truthy());
        assert!(LoxValue::Str(Rc::new("x".into())).is_truthy());
    }

    #[test]
    fn value_equals_semantics() {
        // Nil == Nil.
        assert!(LoxValue::Nil.equals(&LoxValue::Nil));
        // Number equality.
        assert!(LoxValue::Number(1.0).equals(&LoxValue::Number(1.0)));
        assert!(!LoxValue::Number(1.0).equals(&LoxValue::Number(2.0)));
        // NaN is never equal to NaN.
        let nan = LoxValue::Number(f64::NAN);
        assert!(!nan.equals(&LoxValue::Number(f64::NAN)));
        // Strings compare by content, not by Rc identity.
        let a = LoxValue::Str(Rc::new("foo".into()));
        let b = LoxValue::Str(Rc::new("foo".into()));
        assert!(a.equals(&b));
        assert!(!a.equals(&LoxValue::Str(Rc::new("bar".into()))));
        // Different variants never equal.
        assert!(!LoxValue::Nil.equals(&LoxValue::Bool(false)));
        assert!(!LoxValue::Number(0.0).equals(&LoxValue::Bool(false)));
        assert!(!LoxValue::Str(Rc::new("1".into())).equals(&LoxValue::Number(1.0)));
    }

    #[test]
    fn value_from_literal_nil_bool_num_str() {
        assert!(matches!(LoxValue::from(Literal::Nil), LoxValue::Nil));
        assert!(matches!(
            LoxValue::from(Literal::Bool(true)),
            LoxValue::Bool(true)
        ));
        match LoxValue::from(Literal::Num(3.5)) {
            LoxValue::Number(n) => assert_eq!(n, 3.5),
            _ => panic!("expected Number"),
        }
        match LoxValue::from(Literal::Str("hi".into())) {
            LoxValue::Str(s) => assert_eq!(&*s, "hi"),
            _ => panic!("expected Str"),
        }
    }

    #[test]
    fn value_native_clock_callable_arity_zero() {
        let v = native_clock();
        match v {
            LoxValue::Callable(c) => {
                assert_eq!(c.arity(), 0);
                assert_eq!(c.name(), "clock");
            }
            _ => panic!("native_clock should produce Callable"),
        }
    }

    #[test]
    fn value_lox_class_find_method_searches_superclass() {
        // Build a tiny `foo` method shell. We never call it, so we don't
        // need a real closure — just a Stmt::Function to satisfy the type.
        let foo_decl = Stmt::Function {
            name: ident("foo"),
            params: vec![],
            body: vec![],
        };
        // A dummy EnvironmentLike impl just so we can construct the closure Rc.
        #[derive(Debug)]
        struct DummyEnv;
        impl EnvironmentLike for DummyEnv {
            fn define(&mut self, _name: String, _value: LoxValue) {}
        }
        let closure: Rc<RefCell<dyn EnvironmentLike>> = Rc::new(RefCell::new(DummyEnv));
        let foo_fn = Rc::new(LoxFunction {
            decl: foo_decl,
            closure,
            is_initializer: false,
        });

        let mut a_methods: HashMap<String, Rc<LoxFunction>> = HashMap::new();
        a_methods.insert("foo".into(), Rc::clone(&foo_fn));
        let a = Rc::new(LoxClass {
            name: "A".into(),
            superclass: None,
            methods: a_methods,
        });

        let b = LoxClass {
            name: "B".into(),
            superclass: Some(Rc::clone(&a)),
            methods: HashMap::new(),
        };

        // B has no direct `foo`, but inherits from A.
        assert!(b.find_method("foo").is_some());
        // And `bar` isn't anywhere.
        assert!(b.find_method("bar").is_none());
        // A still finds its own.
        assert!(a.find_method("foo").is_some());
    }

    #[test]
    fn value_lox_instance_get_set_round_trip() {
        let class = Rc::new(LoxClass {
            name: "C".into(),
            superclass: None,
            methods: HashMap::new(),
        });
        let mut inst = LoxInstance {
            class,
            fields: HashMap::new(),
        };
        let x = ident("x");
        inst.set(&x, LoxValue::Number(1.0));
        match inst.get(&x).expect("field set above") {
            LoxValue::Number(n) => assert_eq!(n, 1.0),
            _ => panic!("expected Number(1)"),
        }
    }

    #[test]
    fn value_lox_instance_get_undefined_errors() {
        let class = Rc::new(LoxClass {
            name: "C".into(),
            superclass: None,
            methods: HashMap::new(),
        });
        let inst = LoxInstance {
            class,
            fields: HashMap::new(),
        };
        let name = Token::new(TokenType::Identifier, "missing", None, 4);
        let err = inst.get(&name).expect_err("should be undefined");
        match err {
            LoxError::Runtime { line, msg } => {
                assert_eq!(line, 4);
                assert!(
                    msg.contains("Undefined property"),
                    "message should mention Undefined property, got: {msg}"
                );
                assert!(msg.contains("missing"));
            }
            other => panic!("expected Runtime error, got {other:?}"),
        }
    }

    #[test]
    fn value_display_class_and_instance() {
        let class = Rc::new(LoxClass {
            name: "Dog".into(),
            superclass: None,
            methods: HashMap::new(),
        });
        assert_eq!(format!("{}", LoxValue::Class(Rc::clone(&class))), "Dog");
        let inst = Rc::new(RefCell::new(LoxInstance {
            class: Rc::clone(&class),
            fields: HashMap::new(),
        }));
        assert_eq!(format!("{}", LoxValue::Instance(inst)), "Dog instance");
    }
}
