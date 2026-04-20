//! Tree-walking evaluator for Lox (book chapters 7-13).
//!
//! Walks the resolved AST statement-by-statement, producing output via
//! `print` and propagating runtime errors up the stack. Control flow for
//! `return` is threaded through a `Signal` enum — the book throws a Java
//! exception subclass; in Rust we model it as a non-error `Result::Err`
//! variant carrying the return value, then unwind just enough frames to
//! reach the enclosing function body.
//!
//! ## Signal vs LoxError::Return
//!
//! We chose the `Signal` enum approach (wrapping `LoxError` + `Return(LoxValue)`)
//! rather than adding a `Return` variant to `LoxError`. The upsides:
//!   - `LoxError` stays `Clone + PartialEq` (needed by the existing error test
//!     suite).
//!   - The distinction between "user-visible error" and "internal control
//!     flow" is explicit at every use site — `execute` returns `Signal`,
//!     `evaluate` returns `LoxError` (since an expression never directly
//!     contains a `return` stmt — only block bodies do).
//!   - `LoxValue` is not `Eq`, so shoehorning a `Return(LoxValue)` into a
//!     `PartialEq` enum would require a manual impl that skips the variant.

use std::cell::RefCell;
use std::collections::HashMap;
use std::io::{self, Write};
use std::rc::Rc;

use crate::ast::{Expr, Stmt};
use crate::environment::Environment;
use crate::error::LoxError;
use crate::token::{Token, TokenType};
use crate::value::{
    instance_get, instantiate_class, native_clock, Executor, LoxClass, LoxFunction, LoxValue,
};

// ---------------------------------------------------------------------------
// Internal control-flow signal.
// ---------------------------------------------------------------------------

/// What `execute` can propagate upward. Only statements inside a function body
/// ever observe `Return`; the top-level `interpret` converts it to an error
/// (which the resolver actually forbids, so this is a belt-and-braces check).
enum Signal {
    Error(LoxError),
    Return(LoxValue),
}

impl From<LoxError> for Signal {
    fn from(e: LoxError) -> Self {
        Signal::Error(e)
    }
}

// ---------------------------------------------------------------------------
// Interpreter.
// ---------------------------------------------------------------------------

pub struct Interpreter {
    pub globals: Rc<RefCell<Environment>>,
    env: Rc<RefCell<Environment>>,
    locals: HashMap<usize, usize>,
    /// Where `print` writes. Defaults to `Box::new(io::stdout())` for the
    /// binary; tests swap in a `Vec<u8>` and read it back via
    /// `take_output`.
    output: Box<dyn Write>,
}

impl Default for Interpreter {
    fn default() -> Self {
        Self::new()
    }
}

impl Interpreter {
    pub fn new() -> Self {
        let globals = Rc::new(RefCell::new(Environment::new()));
        globals.borrow_mut().define("clock", native_clock());
        Self {
            env: Rc::clone(&globals),
            globals,
            locals: HashMap::new(),
            output: Box::new(io::stdout()),
        }
    }

    /// Install the resolver's side-table. Called by the driver between
    /// resolver and `interpret`.
    pub fn install_locals(&mut self, locals: HashMap<usize, usize>) {
        // Merge rather than replace so REPL sessions (which call this once per
        // line) accumulate bindings across input lines.
        self.locals.extend(locals);
    }

    /// Redirect `print` output to the supplied writer. Useful for tests.
    pub fn set_output(&mut self, w: Box<dyn Write>) {
        self.output = w;
    }

    /// Top-level entry point — runs each statement in sequence.
    pub fn interpret(&mut self, stmts: &[Stmt]) -> Result<(), LoxError> {
        for s in stmts {
            match self.execute(s) {
                Ok(()) => {}
                Err(Signal::Error(e)) => return Err(e),
                Err(Signal::Return(_)) => {
                    // Resolver prevents this; being defensive.
                    return Err(LoxError::runtime(0, "'return' outside function."));
                }
            }
        }
        // Best-effort flush so the binary's stdout shows up promptly.
        let _ = self.output.flush();
        Ok(())
    }

    // =======================================================================
    // Statements.
    // =======================================================================

    fn execute(&mut self, stmt: &Stmt) -> Result<(), Signal> {
        match stmt {
            Stmt::Expression(e) => {
                self.evaluate(e)?;
                Ok(())
            }
            Stmt::Print(e) => {
                let v = self.evaluate(e)?;
                // Writes to the interpreter's sink. `write!` returns io::Error;
                // translate to a runtime error so the interpreter can surface
                // it (should effectively never fire on normal stdout/Vec).
                writeln!(self.output, "{v}").map_err(|err| {
                    Signal::Error(LoxError::runtime(0, format!("I/O error: {err}")))
                })?;
                Ok(())
            }
            Stmt::Var { name, initializer } => {
                let value = match initializer {
                    Some(expr) => self.evaluate(expr)?,
                    None => LoxValue::Nil,
                };
                self.env.borrow_mut().define(name.lexeme.clone(), value);
                Ok(())
            }
            Stmt::Block(stmts) => {
                let new_env = Rc::new(RefCell::new(Environment::with_enclosing(Rc::clone(
                    &self.env,
                ))));
                self.execute_block(stmts, new_env)
            }
            Stmt::If {
                cond,
                then_branch,
                else_branch,
            } => {
                let c = self.evaluate(cond)?;
                if c.is_truthy() {
                    self.execute(then_branch)?;
                } else if let Some(eb) = else_branch {
                    self.execute(eb)?;
                }
                Ok(())
            }
            Stmt::While { cond, body } => {
                loop {
                    let c = self.evaluate(cond)?;
                    if !c.is_truthy() {
                        break;
                    }
                    self.execute(body)?;
                }
                Ok(())
            }
            Stmt::Function { name, .. } => {
                let function = LoxFunction {
                    decl: stmt.clone(),
                    closure: Rc::clone(&self.env),
                    is_initializer: false,
                };
                self.env
                    .borrow_mut()
                    .define(name.lexeme.clone(), LoxValue::Callable(Rc::new(function)));
                Ok(())
            }
            Stmt::Return { value, .. } => {
                let v = match value {
                    Some(e) => self.evaluate(e)?,
                    None => LoxValue::Nil,
                };
                Err(Signal::Return(v))
            }
            Stmt::Class {
                name,
                superclass,
                methods,
            } => self.execute_class(name, superclass.as_ref(), methods),
        }
    }

    fn execute_class(
        &mut self,
        name: &Token,
        superclass: Option<&Expr>,
        methods: &[Stmt],
    ) -> Result<(), Signal> {
        // Evaluate the superclass expression, if any.
        let superclass_rc: Option<Rc<LoxClass>> = match superclass {
            Some(sc_expr) => match self.evaluate(sc_expr)? {
                LoxValue::Class(c) => Some(c),
                _ => {
                    let line = match sc_expr {
                        Expr::Variable { name, .. } => name.line,
                        _ => name.line,
                    };
                    return Err(Signal::Error(LoxError::runtime(
                        line,
                        "Superclass must be a class.",
                    )));
                }
            },
            None => None,
        };

        // Declare the class name in the current scope (bound to Nil first so
        // the class can reference itself inside method bodies).
        self.env
            .borrow_mut()
            .define(name.lexeme.clone(), LoxValue::Nil);

        // If there's a superclass, push an extra scope binding `super`.
        // We save the outer env so we can restore it after the method-closure
        // dance, because method closures must capture the `super`-bearing env.
        let outer_env = Rc::clone(&self.env);
        if let Some(sc) = &superclass_rc {
            let sup_env = Rc::new(RefCell::new(Environment::with_enclosing(Rc::clone(
                &self.env,
            ))));
            sup_env
                .borrow_mut()
                .define("super", LoxValue::Class(Rc::clone(sc)));
            self.env = sup_env;
        }

        // Build each method's LoxFunction with the current env as the closure
        // (which, for subclasses, has `super` defined).
        let mut method_table: HashMap<String, Rc<LoxFunction>> = HashMap::new();
        for m in methods {
            if let Stmt::Function { name: m_name, .. } = m {
                let is_init = m_name.lexeme == "init";
                let func = LoxFunction {
                    decl: m.clone(),
                    closure: Rc::clone(&self.env),
                    is_initializer: is_init,
                };
                method_table.insert(m_name.lexeme.clone(), Rc::new(func));
            }
        }

        // Pop the `super` scope if we pushed one.
        if superclass_rc.is_some() {
            self.env = outer_env;
        }

        let class_rc = Rc::new(LoxClass {
            name: name.lexeme.clone(),
            superclass: superclass_rc,
            methods: method_table,
        });
        self.env
            .borrow_mut()
            .assign(name, LoxValue::Class(class_rc))
            .map_err(Signal::Error)?;
        Ok(())
    }

    /// Executes `stmts` with `self.env` temporarily swapped to `new_env`.
    /// Restores the previous env on every control-flow exit (Ok / error /
    /// return sentinel). This is the block-level entry used by `Stmt::Block`
    /// and by the function-body helper on the `Executor` trait.
    fn execute_block(
        &mut self,
        stmts: &[Stmt],
        new_env: Rc<RefCell<Environment>>,
    ) -> Result<(), Signal> {
        let previous = std::mem::replace(&mut self.env, new_env);
        let mut result: Result<(), Signal> = Ok(());
        for s in stmts {
            if let Err(sig) = self.execute(s) {
                result = Err(sig);
                break;
            }
        }
        self.env = previous;
        result
    }

    // =======================================================================
    // Expressions.
    // =======================================================================

    fn evaluate(&mut self, expr: &Expr) -> Result<LoxValue, LoxError> {
        match expr {
            Expr::Literal(lit) => Ok(LoxValue::from(lit.clone())),
            Expr::Grouping(inner) => self.evaluate(inner),
            Expr::Unary { op, right } => {
                let r = self.evaluate(right)?;
                match op.ttype {
                    TokenType::Minus => match r {
                        LoxValue::Number(n) => Ok(LoxValue::Number(-n)),
                        _ => Err(LoxError::runtime(op.line, "Operand must be a number.")),
                    },
                    TokenType::Bang => Ok(LoxValue::Bool(!r.is_truthy())),
                    _ => unreachable!("parser never emits other unary ops"),
                }
            }
            Expr::Binary { left, op, right } => self.evaluate_binary(left, op, right),
            Expr::Logical { left, op, right } => {
                let l = self.evaluate(left)?;
                // Short-circuit: Or returns l if truthy; And returns l if falsey.
                match op.ttype {
                    TokenType::Or => {
                        if l.is_truthy() {
                            return Ok(l);
                        }
                    }
                    TokenType::And => {
                        if !l.is_truthy() {
                            return Ok(l);
                        }
                    }
                    _ => unreachable!("parser never emits other logical ops"),
                }
                self.evaluate(right)
            }
            Expr::Variable { name, id } => self.look_up_variable(name, *id),
            Expr::Assign { name, value, id } => {
                let v = self.evaluate(value)?;
                if let Some(&depth) = self.locals.get(id) {
                    let ok = self
                        .env
                        .borrow_mut()
                        .assign_at(depth, &name.lexeme, v.clone());
                    if !ok {
                        return Err(LoxError::runtime(
                            name.line,
                            format!("Undefined variable '{}'.", name.lexeme),
                        ));
                    }
                } else {
                    self.globals.borrow_mut().assign(name, v.clone())?;
                }
                Ok(v)
            }
            Expr::Call {
                callee,
                paren,
                args,
            } => self.evaluate_call(callee, paren, args),
            Expr::Get { object, name } => {
                let obj = self.evaluate(object)?;
                match obj {
                    LoxValue::Instance(inst) => instance_get(&inst, name),
                    _ => Err(LoxError::runtime(
                        name.line,
                        "Only instances have properties.",
                    )),
                }
            }
            Expr::Set {
                object,
                name,
                value,
            } => {
                let obj = self.evaluate(object)?;
                let LoxValue::Instance(inst) = obj else {
                    return Err(LoxError::runtime(name.line, "Only instances have fields."));
                };
                let v = self.evaluate(value)?;
                inst.borrow_mut().set(name, v.clone());
                Ok(v)
            }
            Expr::This { keyword, id } => self.look_up_variable(keyword, *id),
            Expr::Super {
                keyword,
                method,
                id,
            } => self.evaluate_super(keyword, method, *id),
        }
    }

    fn evaluate_binary(
        &mut self,
        left: &Expr,
        op: &Token,
        right: &Expr,
    ) -> Result<LoxValue, LoxError> {
        let l = self.evaluate(left)?;
        let r = self.evaluate(right)?;
        match op.ttype {
            TokenType::Minus => arith_num(op, l, r, |a, b| a - b),
            TokenType::Star => arith_num(op, l, r, |a, b| a * b),
            TokenType::Slash => arith_num(op, l, r, |a, b| a / b),
            TokenType::Plus => match (l, r) {
                (LoxValue::Number(a), LoxValue::Number(b)) => Ok(LoxValue::Number(a + b)),
                (LoxValue::Str(a), LoxValue::Str(b)) => {
                    let mut s = String::with_capacity(a.len() + b.len());
                    s.push_str(&a);
                    s.push_str(&b);
                    Ok(LoxValue::Str(Rc::new(s)))
                }
                _ => Err(LoxError::runtime(
                    op.line,
                    "Operands must be two numbers or two strings.",
                )),
            },
            TokenType::Greater => cmp_num(op, l, r, |a, b| a > b),
            TokenType::GreaterEqual => cmp_num(op, l, r, |a, b| a >= b),
            TokenType::Less => cmp_num(op, l, r, |a, b| a < b),
            TokenType::LessEqual => cmp_num(op, l, r, |a, b| a <= b),
            TokenType::EqualEqual => Ok(LoxValue::Bool(l.equals(&r))),
            TokenType::BangEqual => Ok(LoxValue::Bool(!l.equals(&r))),
            _ => unreachable!("parser never emits other binary ops"),
        }
    }

    fn evaluate_call(
        &mut self,
        callee: &Expr,
        paren: &Token,
        args: &[Expr],
    ) -> Result<LoxValue, LoxError> {
        let callee_val = self.evaluate(callee)?;
        let mut arg_vals: Vec<LoxValue> = Vec::with_capacity(args.len());
        for a in args {
            arg_vals.push(self.evaluate(a)?);
        }

        match callee_val {
            LoxValue::Callable(c) => {
                if arg_vals.len() != c.arity() {
                    return Err(LoxError::runtime(
                        paren.line,
                        format!(
                            "Expected {} arguments but got {}.",
                            c.arity(),
                            arg_vals.len()
                        ),
                    ));
                }
                c.call(self, arg_vals)
            }
            LoxValue::Class(class_rc) => {
                let arity = class_rc.arity();
                if arg_vals.len() != arity {
                    return Err(LoxError::runtime(
                        paren.line,
                        format!("Expected {} arguments but got {}.", arity, arg_vals.len()),
                    ));
                }
                instantiate_class(class_rc, self, arg_vals)
            }
            _ => Err(LoxError::runtime(
                paren.line,
                "Can only call functions and classes.",
            )),
        }
    }

    fn evaluate_super(
        &mut self,
        keyword: &Token,
        method: &Token,
        id: usize,
    ) -> Result<LoxValue, LoxError> {
        // Resolver guarantees `super` has a local entry. `this` lives exactly
        // one scope nearer the call site (see resolver's class scope setup).
        let distance = *self
            .locals
            .get(&id)
            .ok_or_else(|| LoxError::runtime(keyword.line, "Internal: 'super' not resolved."))?;
        let superclass = match self.env.borrow().get_at(distance, "super") {
            Some(LoxValue::Class(c)) => c,
            _ => {
                return Err(LoxError::runtime(
                    keyword.line,
                    "Internal: 'super' missing or not a class.",
                ));
            }
        };
        let this_val = self
            .env
            .borrow()
            .get_at(distance - 1, "this")
            .ok_or_else(|| LoxError::runtime(keyword.line, "Internal: 'this' not in scope."))?;
        let LoxValue::Instance(this_inst) = this_val else {
            return Err(LoxError::runtime(
                keyword.line,
                "Internal: 'this' not an instance.",
            ));
        };
        let m = superclass.find_method(&method.lexeme).ok_or_else(|| {
            LoxError::runtime(
                method.line,
                format!("Undefined property '{}'.", method.lexeme),
            )
        })?;
        Ok(LoxValue::Callable(Rc::new(m.bind(this_inst))))
    }

    fn look_up_variable(&self, name: &Token, id: usize) -> Result<LoxValue, LoxError> {
        if let Some(&depth) = self.locals.get(&id) {
            self.env
                .borrow()
                .get_at(depth, &name.lexeme)
                .ok_or_else(|| {
                    LoxError::runtime(name.line, format!("Undefined variable '{}'.", name.lexeme))
                })
        } else {
            self.globals.borrow().get(name)
        }
    }
}

// Plumbing for `LoxFunction::call` — the Executor trait is how value.rs talks
// back into the interpreter without importing it.
impl Executor for Interpreter {
    fn execute_function_body(
        &mut self,
        body: &[Stmt],
        env: Rc<RefCell<Environment>>,
    ) -> Result<LoxValue, LoxError> {
        match self.execute_block(body, env) {
            Ok(()) => Ok(LoxValue::Nil),
            Err(Signal::Return(v)) => Ok(v),
            Err(Signal::Error(e)) => Err(e),
        }
    }
}

// ---------------------------------------------------------------------------
// Small helpers kept free of `self` borrows so the hot path above stays
// readable.
// ---------------------------------------------------------------------------

fn arith_num(
    op: &Token,
    l: LoxValue,
    r: LoxValue,
    f: impl FnOnce(f64, f64) -> f64,
) -> Result<LoxValue, LoxError> {
    match (l, r) {
        (LoxValue::Number(a), LoxValue::Number(b)) => Ok(LoxValue::Number(f(a, b))),
        _ => Err(LoxError::runtime(op.line, "Operands must be numbers.")),
    }
}

fn cmp_num(
    op: &Token,
    l: LoxValue,
    r: LoxValue,
    f: impl FnOnce(f64, f64) -> bool,
) -> Result<LoxValue, LoxError> {
    match (l, r) {
        (LoxValue::Number(a), LoxValue::Number(b)) => Ok(LoxValue::Bool(f(a, b))),
        _ => Err(LoxError::runtime(op.line, "Operands must be numbers.")),
    }
}
