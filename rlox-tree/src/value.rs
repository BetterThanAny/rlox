//! Runtime values for the tree-walking Lox interpreter.
//!
//! Mirrors Nystrom's `Object` / `LoxCallable` / `LoxClass` / `LoxInstance`
//! hierarchy (Crafting Interpreters, chs. 7, 10, 12, 13) as a Rust enum plus
//! trait-object callables.
//!
//! The bridge to the interpreter is the [`Executor`] trait: `LoxFunction` and
//! `LoxClass` cannot own the concrete `Interpreter` directly (which owns them
//! via `Rc<dyn LoxCallable>`), so they call back through `&mut dyn Executor`
//! when they need to execute a function body. `LoxFunction::closure` is stored
//! as a concrete `Rc<RefCell<Environment>>` — cross-module cycle between
//! `value.rs` and `environment.rs` is resolved at the `use` layer and is fine
//! for rustc.

use std::cell::RefCell;
use std::collections::HashMap;
use std::fmt;
use std::rc::Rc;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::ast::Stmt;
use crate::environment::Environment;
use crate::error::LoxError;
use crate::token::{Literal, Token};

// ---------------------------------------------------------------------------
// Bridge traits.
// ---------------------------------------------------------------------------

/// Interface the interpreter must provide so `LoxFunction` / `LoxClass` can
/// execute their bodies without knowing the concrete `Interpreter` type.
///
/// `Interpreter` implements this in `interpreter.rs`.
pub trait Executor {
    /// Run a function body inside the given environment. Returns the return
    /// value produced by a `return` statement, or `Nil` if the body ran to
    /// completion. The implementation catches the function-level return sentinel
    /// and converts it back to a plain `Result<LoxValue, LoxError>`.
    fn execute_function_body(
        &mut self,
        body: &[Stmt],
        env: Rc<RefCell<Environment>>,
    ) -> Result<LoxValue, LoxError>;
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
            LoxValue::Callable(c) => {
                if c.is_native() {
                    write!(f, "<native fn>")
                } else {
                    write!(f, "<fn {}>", c.name())
                }
            }
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
    /// `true` for host-provided built-ins like `clock`. User-defined
    /// functions, methods, and classes should return `false` (the default).
    fn is_native(&self) -> bool {
        false
    }
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

    fn is_native(&self) -> bool {
        true
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
    pub closure: Rc<RefCell<Environment>>,
    pub is_initializer: bool,
}

impl LoxFunction {
    /// Book ch. 12 `LoxFunction.bind(instance)`: return a new LoxFunction whose
    /// closure is a child of the original closure with `this` defined.
    pub fn bind(&self, instance: Rc<RefCell<LoxInstance>>) -> LoxFunction {
        let env = Rc::new(RefCell::new(Environment::with_enclosing(Rc::clone(
            &self.closure,
        ))));
        env.borrow_mut()
            .define("this", LoxValue::Instance(instance));
        LoxFunction {
            decl: self.decl.clone(),
            closure: env,
            is_initializer: self.is_initializer,
        }
    }
}

impl LoxCallable for LoxFunction {
    fn arity(&self) -> usize {
        match &self.decl {
            Stmt::Function { params, .. } => params.len(),
            _ => unreachable!("LoxFunction::decl must be Stmt::Function"),
        }
    }

    fn call(&self, executor: &mut dyn Executor, args: Vec<LoxValue>) -> Result<LoxValue, LoxError> {
        let Stmt::Function { params, body, .. } = &self.decl else {
            unreachable!("LoxFunction::decl must be Stmt::Function");
        };

        // Build the call-frame env: a fresh child of the closure, with each
        // parameter defined to its corresponding argument.
        let env = Rc::new(RefCell::new(Environment::with_enclosing(Rc::clone(
            &self.closure,
        ))));
        {
            let mut env_mut = env.borrow_mut();
            for (param, arg) in params.iter().zip(args.into_iter()) {
                env_mut.define(param.lexeme.clone(), arg);
            }
        }

        let result = executor.execute_function_body(body, env)?;

        // Book ch. 12.6: init() always returns `this`, even on explicit bare
        // `return;`. The function's closure includes a `this` binding (see
        // `bind` above), so fetch it out of the closure at depth 0.
        if self.is_initializer {
            if let Some(this) = self.closure.borrow().get_at(0, "this") {
                return Ok(this);
            }
        }
        Ok(result)
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

    /// Arity of the class's `init` method, or 0 if none.
    pub fn arity(&self) -> usize {
        self.find_method("init").map(|i| i.arity()).unwrap_or(0)
    }
}

/// Invoke a class as a constructor. Takes an owned `Rc<LoxClass>` so the
/// resulting `LoxInstance` points at the exact same class value stored in the
/// interpreter's environment (preserving pointer identity for class `==`).
///
/// The `LoxCallable` trait only surfaces `&self`, so classes are **not**
/// called through it — the interpreter dispatches on `LoxValue::Class`
/// directly and invokes this helper.
pub fn instantiate_class(
    class: Rc<LoxClass>,
    executor: &mut dyn Executor,
    args: Vec<LoxValue>,
) -> Result<LoxValue, LoxError> {
    let instance = Rc::new(RefCell::new(LoxInstance {
        class: Rc::clone(&class),
        fields: HashMap::new(),
    }));
    if let Some(initializer) = class.find_method("init") {
        initializer
            .bind(Rc::clone(&instance))
            .call(executor, args)?;
    }
    Ok(LoxValue::Instance(instance))
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
    /// `instance.name = value`.
    pub fn set(&mut self, name: &Token, value: LoxValue) {
        self.fields.insert(name.lexeme.clone(), value);
    }
}

/// `instance.name` lookup: check fields first, then (bound) class methods.
///
/// This lives outside the `LoxInstance` impl because method binding needs
/// `Rc<RefCell<LoxInstance>>` (not `&LoxInstance`) to thread `this` into the
/// method's closure.
pub fn instance_get(
    instance: &Rc<RefCell<LoxInstance>>,
    name: &Token,
) -> Result<LoxValue, LoxError> {
    let borrow = instance.borrow();
    if let Some(v) = borrow.fields.get(&name.lexeme) {
        return Ok(v.clone());
    }
    if let Some(method) = borrow.class.find_method(&name.lexeme) {
        // Bind `this` into a fresh closure and surface as a Callable.
        drop(borrow);
        let bound = method.bind(Rc::clone(instance));
        return Ok(LoxValue::Callable(Rc::new(bound)));
    }
    Err(LoxError::runtime(
        name.line,
        format!("Undefined property '{}'.", name.lexeme),
    ))
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
        // Build a tiny `foo` method shell. We never call it, so a real but
        // empty Environment is fine as the closure.
        let foo_decl = Stmt::Function {
            name: ident("foo"),
            params: vec![],
            body: vec![],
        };
        let closure = Rc::new(RefCell::new(Environment::new()));
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
        let inst = Rc::new(RefCell::new(LoxInstance {
            class,
            fields: HashMap::new(),
        }));
        let x = ident("x");
        inst.borrow_mut().set(&x, LoxValue::Number(1.0));
        match instance_get(&inst, &x).expect("field set above") {
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
        let inst = Rc::new(RefCell::new(LoxInstance {
            class,
            fields: HashMap::new(),
        }));
        let name = Token::new(TokenType::Identifier, "missing", None, 4);
        let err = instance_get(&inst, &name).expect_err("should be undefined");
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
