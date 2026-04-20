//! Static variable resolver for Lox.
//!
//! Mirrors *Crafting Interpreters* chapter 11 ("Resolving and Binding").
//! Walks the AST once after parsing, records the scope depth at which each
//! variable/assignment/`this`/`super` expression resolves, and checks the
//! static rules the interpreter would not be able to enforce correctly on
//! its own (self-referencing initializers, duplicate locals, misplaced
//! `return`/`this`/`super`, inheritance from self).
//!
//! Output is a side-table mapping `Expr` id → depth (number of scopes
//! between the expression and the scope that declared the name). Globals
//! stay absent from the map and are looked up dynamically at runtime.

use std::collections::HashMap;

use crate::ast::*;
use crate::error::LoxError;
use crate::token::Token;

#[derive(Copy, Clone, PartialEq, Eq)]
enum FunctionType {
    None,
    Function,
    Initializer,
    Method,
}

#[derive(Copy, Clone, PartialEq, Eq)]
enum ClassType {
    None,
    Class,
    Subclass,
}

pub struct Resolver {
    scopes: Vec<HashMap<String, bool>>,
    locals: HashMap<usize, usize>,
    current_fn: FunctionType,
    current_class: ClassType,
    errors: Vec<LoxError>,
}

impl Default for Resolver {
    fn default() -> Self {
        Self::new()
    }
}

impl Resolver {
    pub fn new() -> Self {
        Self {
            scopes: Vec::new(),
            locals: HashMap::new(),
            current_fn: FunctionType::None,
            current_class: ClassType::None,
            errors: Vec::new(),
        }
    }

    /// Walk the program; returns the expr-id → depth side-table on success
    /// or every error that was accumulated along the way.
    pub fn resolve(mut self, stmts: &[Stmt]) -> Result<HashMap<usize, usize>, Vec<LoxError>> {
        self.resolve_stmts(stmts);
        if self.errors.is_empty() {
            Ok(self.locals)
        } else {
            Err(self.errors)
        }
    }

    // ---------- statement/expression walkers ----------

    fn resolve_stmts(&mut self, stmts: &[Stmt]) {
        for s in stmts {
            self.resolve_stmt(s);
        }
    }

    fn resolve_stmt(&mut self, stmt: &Stmt) {
        match stmt {
            Stmt::Block(stmts) => {
                self.begin_scope();
                self.resolve_stmts(stmts);
                self.end_scope();
            }
            Stmt::Var { name, initializer } => {
                self.declare(name);
                if let Some(init) = initializer {
                    self.resolve_expr(init);
                }
                self.define(name);
            }
            Stmt::Function { name, params, body } => {
                self.declare(name);
                self.define(name);
                self.resolve_function(params, body, FunctionType::Function);
            }
            Stmt::Class {
                name,
                superclass,
                methods,
            } => {
                let enclosing_class = self.current_class;
                self.current_class = ClassType::Class;

                self.declare(name);
                self.define(name);

                if let Some(sc) = superclass {
                    if let Expr::Variable { name: sc_name, .. } = sc {
                        if sc_name.lexeme == name.lexeme {
                            self.errors.push(LoxError::resolve(
                                sc_name.line,
                                "A class can't inherit from itself.",
                            ));
                        }
                    }
                    self.current_class = ClassType::Subclass;
                    self.resolve_expr(sc);

                    self.begin_scope();
                    if let Some(scope) = self.scopes.last_mut() {
                        scope.insert("super".to_string(), true);
                    }
                }

                self.begin_scope();
                if let Some(scope) = self.scopes.last_mut() {
                    scope.insert("this".to_string(), true);
                }

                for method in methods {
                    if let Stmt::Function {
                        name: m_name,
                        params,
                        body,
                    } = method
                    {
                        let declaration = if m_name.lexeme == "init" {
                            FunctionType::Initializer
                        } else {
                            FunctionType::Method
                        };
                        self.resolve_function(params, body, declaration);
                    }
                }

                self.end_scope();

                if superclass.is_some() {
                    self.end_scope();
                }

                self.current_class = enclosing_class;
            }
            Stmt::Expression(e) => self.resolve_expr(e),
            Stmt::If {
                cond,
                then_branch,
                else_branch,
            } => {
                self.resolve_expr(cond);
                self.resolve_stmt(then_branch);
                if let Some(else_b) = else_branch {
                    self.resolve_stmt(else_b);
                }
            }
            Stmt::Print(e) => self.resolve_expr(e),
            Stmt::Return { keyword, value } => {
                if self.current_fn == FunctionType::None {
                    self.errors.push(LoxError::resolve(
                        keyword.line,
                        "Can't return from top-level code.",
                    ));
                }
                if let Some(v) = value {
                    if self.current_fn == FunctionType::Initializer {
                        self.errors.push(LoxError::resolve(
                            keyword.line,
                            "Can't return a value from an initializer.",
                        ));
                    }
                    self.resolve_expr(v);
                }
            }
            Stmt::While { cond, body } => {
                self.resolve_expr(cond);
                self.resolve_stmt(body);
            }
        }
    }

    fn resolve_expr(&mut self, expr: &Expr) {
        match expr {
            Expr::Variable { name, id } => {
                if let Some(scope) = self.scopes.last() {
                    if scope.get(&name.lexeme) == Some(&false) {
                        self.errors.push(LoxError::resolve(
                            name.line,
                            "Can't read local variable in its own initializer.",
                        ));
                    }
                }
                self.resolve_local(*id, &name.lexeme);
            }
            Expr::Assign { name, value, id } => {
                self.resolve_expr(value);
                self.resolve_local(*id, &name.lexeme);
            }
            Expr::Binary { left, right, .. } => {
                self.resolve_expr(left);
                self.resolve_expr(right);
            }
            Expr::Call { callee, args, .. } => {
                self.resolve_expr(callee);
                for a in args {
                    self.resolve_expr(a);
                }
            }
            Expr::Get { object, .. } => {
                self.resolve_expr(object);
            }
            Expr::Grouping(e) => self.resolve_expr(e),
            Expr::Literal(_) => {}
            Expr::Logical { left, right, .. } => {
                self.resolve_expr(left);
                self.resolve_expr(right);
            }
            Expr::Set { object, value, .. } => {
                self.resolve_expr(value);
                self.resolve_expr(object);
            }
            Expr::Super { keyword, id, .. } => {
                if self.current_class == ClassType::None {
                    self.errors.push(LoxError::resolve(
                        keyword.line,
                        "Can't use 'super' outside of a class.",
                    ));
                } else if self.current_class == ClassType::Class {
                    self.errors.push(LoxError::resolve(
                        keyword.line,
                        "Can't use 'super' in a class with no superclass.",
                    ));
                }
                self.resolve_local(*id, "super");
            }
            Expr::This { keyword, id } => {
                if self.current_class == ClassType::None {
                    self.errors.push(LoxError::resolve(
                        keyword.line,
                        "Can't use 'this' outside of a class.",
                    ));
                    return;
                }
                self.resolve_local(*id, "this");
            }
            Expr::Unary { right, .. } => self.resolve_expr(right),
        }
    }

    // ---------- scope / binding helpers ----------

    fn begin_scope(&mut self) {
        self.scopes.push(HashMap::new());
    }

    fn end_scope(&mut self) {
        self.scopes.pop();
    }

    fn declare(&mut self, name: &Token) {
        let Some(scope) = self.scopes.last_mut() else {
            // Global: duplicate declarations are allowed per book.
            return;
        };
        if scope.contains_key(&name.lexeme) {
            self.errors.push(LoxError::resolve(
                name.line,
                "Already a variable with this name in this scope.",
            ));
        }
        scope.insert(name.lexeme.clone(), false);
    }

    fn define(&mut self, name: &Token) {
        if let Some(scope) = self.scopes.last_mut() {
            scope.insert(name.lexeme.clone(), true);
        }
    }

    /// Walks scopes from innermost outward; if found, records
    /// `locals[id] = hops`, leaving globals unresolved.
    fn resolve_local(&mut self, id: usize, name: &str) {
        for (i, scope) in self.scopes.iter().enumerate().rev() {
            if scope.contains_key(name) {
                let depth = self.scopes.len() - 1 - i;
                self.locals.insert(id, depth);
                return;
            }
        }
    }

    fn resolve_function(&mut self, params: &[Token], body: &[Stmt], fn_type: FunctionType) {
        let enclosing = self.current_fn;
        self.current_fn = fn_type;

        self.begin_scope();
        for p in params {
            self.declare(p);
            self.define(p);
        }
        self.resolve_stmts(body);
        self.end_scope();

        self.current_fn = enclosing;
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod resolver_tests {
    use super::*;
    use crate::parser::Parser;
    use crate::scanner::Scanner;

    fn resolve_src(src: &str) -> Result<HashMap<usize, usize>, Vec<LoxError>> {
        let tokens = Scanner::new(src)
            .scan_tokens()
            .expect("test source must scan cleanly");
        let stmts = Parser::new(tokens)
            .parse()
            .expect("test source must parse cleanly");
        Resolver::new().resolve(&stmts)
    }

    /// Assert the error list contains at least one error whose rendered
    /// message contains `needle`, else panic with the full error dump.
    fn assert_err_contains(errs: &[LoxError], needle: &str) {
        assert!(
            errs.iter().any(|e| e.to_string().contains(needle)),
            "expected an error containing {:?}; got: {:?}",
            needle,
            errs
        );
    }

    #[test]
    fn resolver_happy_closure_builds_side_table() {
        let locals = resolve_src("fun c() { var x = 1; fun inner() { return x; } return inner; }")
            .expect("should resolve cleanly");
        // At minimum: Variable(x) inside inner + Variable(inner) inside outer body
        // should have been recorded with a depth.
        assert!(
            !locals.is_empty(),
            "expected at least one local entry, got {:?}",
            locals
        );
    }

    #[test]
    fn resolver_self_reference_in_initializer_errors() {
        // Must be inside a block; the book's rule only fires for *local* vars.
        let errs = resolve_src("{ var a = a; }").unwrap_err();
        assert_err_contains(&errs, "Can't read local variable in its own initializer.");
    }

    #[test]
    fn resolver_duplicate_local_declaration_errors() {
        let errs = resolve_src("{ var a = 1; var a = 2; }").unwrap_err();
        assert_err_contains(&errs, "Already a variable with this name in this scope.");
    }

    #[test]
    fn resolver_global_duplicate_allowed() {
        resolve_src("var a = 1; var a = 2;").expect("duplicate globals are allowed");
    }

    #[test]
    fn resolver_return_at_top_level_errors() {
        let errs = resolve_src("return 1;").unwrap_err();
        assert_err_contains(&errs, "Can't return from top-level code.");
    }

    #[test]
    fn resolver_return_value_in_initializer_errors() {
        let errs = resolve_src("class A { init() { return 1; } }").unwrap_err();
        assert_err_contains(&errs, "Can't return a value from an initializer.");
    }

    #[test]
    fn resolver_return_nil_in_initializer_allowed() {
        resolve_src("class A { init() { return; } }").expect("bare return in init() is fine");
    }

    #[test]
    fn resolver_this_outside_class_errors() {
        let errs = resolve_src("print this;").unwrap_err();
        assert_err_contains(&errs, "Can't use 'this' outside of a class.");
    }

    #[test]
    fn resolver_super_outside_class_errors() {
        let errs = resolve_src("print super.x;").unwrap_err();
        assert_err_contains(&errs, "Can't use 'super' outside of a class.");
    }

    #[test]
    fn resolver_super_in_class_with_no_superclass_errors() {
        let errs = resolve_src("class A { foo() { super.bar(); } }").unwrap_err();
        assert_err_contains(&errs, "Can't use 'super' in a class with no superclass.");
    }

    #[test]
    fn resolver_super_in_subclass_allowed() {
        resolve_src("class A {} class B < A { foo() { super.bar(); } }")
            .expect("super in a subclass method is allowed");
    }

    #[test]
    fn resolver_class_inherits_from_itself_errors() {
        let errs = resolve_src("class A < A {}").unwrap_err();
        assert_err_contains(&errs, "A class can't inherit from itself.");
    }

    #[test]
    fn resolver_local_variable_depth_is_recorded() {
        // `{ var a = 1; { print a; } }`
        //   - outermost block = scope 0 (declares `a`)
        //   - inner block     = scope 1 (the Variable(a) lookup site)
        // `resolve_local` computes depth = len - 1 - i, so depth for scope 0
        // from a lookup at scope 1 is 2 - 1 - 0 = 1.
        let src = "{ var a = 1; { print a; } }";
        let tokens = Scanner::new(src).scan_tokens().expect("scan cleanly");
        let stmts = Parser::new(tokens).parse().expect("parse cleanly");

        // Dig out the Variable expr inside the inner `print a;`.
        let Stmt::Block(outer) = &stmts[0] else {
            panic!("expected outer Stmt::Block");
        };
        let Stmt::Block(inner) = &outer[1] else {
            panic!("expected inner Stmt::Block at index 1");
        };
        let Stmt::Print(Expr::Variable { id, .. }) = &inner[0] else {
            panic!(
                "expected Print(Variable) in inner block, got {:?}",
                inner[0]
            );
        };
        let target_id = *id;

        let locals = Resolver::new().resolve(&stmts).expect("should resolve");
        assert_eq!(
            locals.get(&target_id).copied(),
            Some(1),
            "expected depth 1 for inner lookup, full table: {:?}",
            locals
        );
    }

    // --- extra coverage beyond the required 13 -----------------------------

    #[test]
    fn resolver_use_before_declaration_sees_inner_binding_shadow() {
        // `{ var a = 1; { var a = a; } }` — inner `a` is being read while its
        // scope entry is still `false`, so this should trip the self-reference
        // rule, matching the book's example.
        let errs = resolve_src("{ var a = 1; { var a = a; } }").unwrap_err();
        assert_err_contains(&errs, "Can't read local variable in its own initializer.");
    }

    #[test]
    fn resolver_this_inside_method_ok() {
        resolve_src("class A { foo() { this.x; } }").expect("this inside a method should resolve");
    }
}
