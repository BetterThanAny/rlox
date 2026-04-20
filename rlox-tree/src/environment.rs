//! Scope chain / environment for the tree-walking Lox interpreter.
//!
//! Mirrors Nystrom's `Environment` class (Crafting Interpreters, chs. 8 + 11).
//! Each `Environment` stores a map of variable bindings for the current scope
//! and an optional `Rc<RefCell<Environment>>` pointer to its enclosing scope.
//! `define`/`get`/`assign` follow the book exactly; `get_at`/`assign_at`
//! support the resolver-aware fast path introduced in chapter 11.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use crate::error::LoxError;
use crate::token::Token;
use crate::value::{EnvironmentLike, LoxValue};

#[derive(Debug, Default)]
pub struct Environment {
    values: HashMap<String, LoxValue>,
    enclosing: Option<Rc<RefCell<Environment>>>,
}

impl Environment {
    /// Build a fresh global environment with no parent scope.
    pub fn new() -> Self {
        Self::default()
    }

    /// Build a nested environment whose parent is `parent`.
    pub fn with_enclosing(parent: Rc<RefCell<Environment>>) -> Self {
        Self {
            values: HashMap::new(),
            enclosing: Some(parent),
        }
    }

    /// Define (or redefine) a variable in the current scope.
    pub fn define(&mut self, name: impl Into<String>, value: LoxValue) {
        self.values.insert(name.into(), value);
    }

    /// Book's `get`: look up a variable, walking the enclosing chain until
    /// found. Returns a runtime error with the book's standard message if the
    /// name isn't bound anywhere.
    pub fn get(&self, name: &Token) -> Result<LoxValue, LoxError> {
        if let Some(v) = self.values.get(&name.lexeme) {
            return Ok(v.clone());
        }
        if let Some(parent) = &self.enclosing {
            return parent.borrow().get(name);
        }
        Err(LoxError::runtime(
            name.line,
            format!("Undefined variable '{}'.", name.lexeme),
        ))
    }

    /// Book's `assign`: update an existing binding, walking the enclosing
    /// chain. Returns a runtime error if the name isn't bound anywhere.
    pub fn assign(&mut self, name: &Token, value: LoxValue) -> Result<(), LoxError> {
        if self.values.contains_key(&name.lexeme) {
            self.values.insert(name.lexeme.clone(), value);
            return Ok(());
        }
        if let Some(parent) = &self.enclosing {
            return parent.borrow_mut().assign(name, value);
        }
        Err(LoxError::runtime(
            name.line,
            format!("Undefined variable '{}'.", name.lexeme),
        ))
    }

    /// Resolver-aware lookup: hop `distance` ancestors, then read by name.
    /// `distance == 0` means "current scope". Returns `None` if the
    /// resolver misrouted us and the variable isn't actually there.
    pub fn get_at(&self, distance: usize, name: &str) -> Option<LoxValue> {
        if distance == 0 {
            return self.values.get(name).cloned();
        }
        // Walk `distance` hops up the chain.
        let mut current: Rc<RefCell<Environment>> = match &self.enclosing {
            Some(parent) => Rc::clone(parent),
            None => return None,
        };
        for _ in 1..distance {
            let next = match &current.borrow().enclosing {
                Some(parent) => Rc::clone(parent),
                None => return None,
            };
            current = next;
        }
        let borrow = current.borrow();
        borrow.values.get(name).cloned()
    }

    /// Resolver-aware write: hop `distance` ancestors, then write by name.
    /// Returns `true` on success, `false` if the target scope didn't have
    /// that name (resolver bug).
    pub fn assign_at(&mut self, distance: usize, name: &str, value: LoxValue) -> bool {
        if distance == 0 {
            if self.values.contains_key(name) {
                self.values.insert(name.to_string(), value);
                return true;
            }
            return false;
        }
        let mut current: Rc<RefCell<Environment>> = match &self.enclosing {
            Some(parent) => Rc::clone(parent),
            None => return false,
        };
        for _ in 1..distance {
            let next = match &current.borrow().enclosing {
                Some(parent) => Rc::clone(parent),
                None => return false,
            };
            current = next;
        }
        let mut borrow = current.borrow_mut();
        if borrow.values.contains_key(name) {
            borrow.values.insert(name.to_string(), value);
            true
        } else {
            false
        }
    }
}

/// Bridge impl so `LoxFunction` (defined in `value.rs`) can define parameters
/// into an `Environment` through a `dyn EnvironmentLike`.
impl EnvironmentLike for Environment {
    fn define(&mut self, name: String, value: LoxValue) {
        self.values.insert(name, value);
    }
}

// ---------------------------------------------------------------------------
// Tests.
// ---------------------------------------------------------------------------

#[cfg(test)]
mod environment_tests {
    use super::*;
    use crate::token::TokenType;

    fn ident(name: &str) -> Token {
        Token::new(TokenType::Identifier, name, None, 1)
    }

    fn ident_line(name: &str, line: usize) -> Token {
        Token::new(TokenType::Identifier, name, None, line)
    }

    #[test]
    fn environment_define_then_get_roundtrip() {
        let mut env = Environment::new();
        env.define("x", LoxValue::Number(42.0));
        let got = env.get(&ident("x")).expect("x was defined");
        match got {
            LoxValue::Number(n) => assert_eq!(n, 42.0),
            other => panic!("expected Number(42), got {other:?}"),
        }
    }

    #[test]
    fn environment_shadow_in_nested_scope() {
        // Outer defines x = 1; inner scope shadows with x = 2.
        let outer = Rc::new(RefCell::new(Environment::new()));
        outer.borrow_mut().define("x", LoxValue::Number(1.0));

        {
            let mut inner = Environment::with_enclosing(Rc::clone(&outer));
            inner.define("x", LoxValue::Number(2.0));
            // Inner sees its own binding.
            match inner.get(&ident("x")).unwrap() {
                LoxValue::Number(n) => assert_eq!(n, 2.0),
                _ => panic!("inner should see 2"),
            }
        }
        // After inner is dropped, outer still has its original binding.
        let got = outer.borrow().get(&ident("x")).unwrap();
        match got {
            LoxValue::Number(n) => assert_eq!(n, 1.0),
            _ => panic!("outer should still see 1"),
        }
    }

    #[test]
    fn environment_get_walks_enclosing_chain() {
        let outer = Rc::new(RefCell::new(Environment::new()));
        outer.borrow_mut().define("y", LoxValue::Number(7.0));
        let inner = Environment::with_enclosing(Rc::clone(&outer));
        // `y` is not in `inner`, so the lookup must walk to `outer`.
        match inner.get(&ident("y")).expect("found via chain") {
            LoxValue::Number(n) => assert_eq!(n, 7.0),
            _ => panic!("expected 7"),
        }
    }

    #[test]
    fn environment_assign_walks_enclosing_chain() {
        let outer = Rc::new(RefCell::new(Environment::new()));
        outer.borrow_mut().define("z", LoxValue::Number(1.0));
        let mut inner = Environment::with_enclosing(Rc::clone(&outer));
        // Assign from inner; must mutate outer.
        inner
            .assign(&ident("z"), LoxValue::Number(99.0))
            .expect("should resolve via chain");
        let got = outer.borrow().get(&ident("z")).unwrap();
        match got {
            LoxValue::Number(n) => assert_eq!(n, 99.0),
            _ => panic!("outer should now hold 99"),
        }
    }

    #[test]
    fn environment_get_undefined_returns_runtime_error() {
        let env = Environment::new();
        let err = env
            .get(&ident_line("foo", 3))
            .expect_err("foo was never defined");
        match err {
            LoxError::Runtime { line, msg } => {
                assert_eq!(line, 3);
                assert_eq!(msg, "Undefined variable 'foo'.");
            }
            other => panic!("expected Runtime error, got {other:?}"),
        }
    }

    #[test]
    fn environment_assign_undefined_returns_runtime_error() {
        let mut env = Environment::new();
        let err = env
            .assign(&ident_line("bar", 9), LoxValue::Nil)
            .expect_err("bar was never defined");
        match err {
            LoxError::Runtime { line, msg } => {
                assert_eq!(line, 9);
                assert_eq!(msg, "Undefined variable 'bar'.");
            }
            other => panic!("expected Runtime error, got {other:?}"),
        }
    }

    #[test]
    fn environment_get_at_depth_zero_reads_self() {
        let mut env = Environment::new();
        env.define("a", LoxValue::Number(5.0));
        match env.get_at(0, "a") {
            Some(LoxValue::Number(n)) => assert_eq!(n, 5.0),
            other => panic!("expected Number(5), got {other:?}"),
        }
        // Missing name at depth 0 should be None.
        assert!(env.get_at(0, "nope").is_none());
    }

    #[test]
    fn environment_get_at_depth_two_reads_grandparent() {
        // grand (depth 2) -> parent (depth 1) -> child (self).
        let grand = Rc::new(RefCell::new(Environment::new()));
        grand.borrow_mut().define("g", LoxValue::Number(10.0));

        let parent = Rc::new(RefCell::new(Environment::with_enclosing(Rc::clone(&grand))));
        parent.borrow_mut().define("p", LoxValue::Number(20.0));

        let mut child = Environment::with_enclosing(Rc::clone(&parent));
        child.define("c", LoxValue::Number(30.0));

        // depth 0 -> child, depth 1 -> parent, depth 2 -> grand.
        match child.get_at(2, "g") {
            Some(LoxValue::Number(n)) => assert_eq!(n, 10.0),
            other => panic!("expected Number(10) at depth 2, got {other:?}"),
        }
        match child.get_at(1, "p") {
            Some(LoxValue::Number(n)) => assert_eq!(n, 20.0),
            other => panic!("expected Number(20) at depth 1, got {other:?}"),
        }
        match child.get_at(0, "c") {
            Some(LoxValue::Number(n)) => assert_eq!(n, 30.0),
            other => panic!("expected Number(30) at depth 0, got {other:?}"),
        }
        // Over-shooting the chain returns None, not a panic.
        assert!(child.get_at(5, "g").is_none());
    }

    #[test]
    fn environment_assign_at_writes_to_correct_depth() {
        let grand = Rc::new(RefCell::new(Environment::new()));
        grand.borrow_mut().define("g", LoxValue::Number(0.0));
        let parent = Rc::new(RefCell::new(Environment::with_enclosing(Rc::clone(&grand))));
        parent.borrow_mut().define("p", LoxValue::Number(0.0));
        let mut child = Environment::with_enclosing(Rc::clone(&parent));
        child.define("c", LoxValue::Number(0.0));

        // Write to self (depth 0).
        assert!(child.assign_at(0, "c", LoxValue::Number(3.0)));
        // Write to parent (depth 1).
        assert!(child.assign_at(1, "p", LoxValue::Number(2.0)));
        // Write to grand (depth 2).
        assert!(child.assign_at(2, "g", LoxValue::Number(1.0)));

        match child.get_at(0, "c").unwrap() {
            LoxValue::Number(n) => assert_eq!(n, 3.0),
            _ => panic!("c should be 3"),
        }
        match parent.borrow().values.get("p").cloned().unwrap() {
            LoxValue::Number(n) => assert_eq!(n, 2.0),
            _ => panic!("p should be 2"),
        }
        match grand.borrow().values.get("g").cloned().unwrap() {
            LoxValue::Number(n) => assert_eq!(n, 1.0),
            _ => panic!("g should be 1"),
        }

        // Miss: name not in that scope returns false.
        assert!(!child.assign_at(0, "missing", LoxValue::Nil));
        // Miss: over-shoot returns false.
        assert!(!child.assign_at(9, "g", LoxValue::Nil));
    }

    #[test]
    fn environment_environmentlike_define_bridges_to_inherent() {
        // Verify the EnvironmentLike impl writes into the same map as
        // the inherent `define` method.
        let mut env = Environment::new();
        // Call through the trait to be sure we're exercising that path.
        <Environment as EnvironmentLike>::define(
            &mut env,
            "bridged".to_string(),
            LoxValue::Bool(true),
        );
        match env.get(&ident("bridged")).unwrap() {
            LoxValue::Bool(b) => assert!(b),
            _ => panic!("expected Bool(true)"),
        }
    }
}
