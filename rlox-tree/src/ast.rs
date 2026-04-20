//! AST node types for the tree-walking Lox interpreter.
//!
//! Mirrors the book's `Expr` and `Stmt` class hierarchies (Chapters 5 + 8–13),
//! expressed as Rust enums with `Box` recursion for owned subtrees.
//!
//! Variants carrying an `id: usize` field are the ones the resolver pass
//! (coming in M2) annotates with scope depth; parser mints these ids when it
//! constructs nodes.

use crate::token::{Literal, Token};

#[derive(Debug, Clone)]
pub enum Expr {
    Assign {
        name: Token,
        value: Box<Expr>,
        id: usize,
    },
    Binary {
        left: Box<Expr>,
        op: Token,
        right: Box<Expr>,
    },
    Call {
        callee: Box<Expr>,
        paren: Token,
        args: Vec<Expr>,
    },
    Get {
        object: Box<Expr>,
        name: Token,
    },
    Grouping(Box<Expr>),
    Literal(Literal),
    Logical {
        left: Box<Expr>,
        op: Token,
        right: Box<Expr>,
    },
    Set {
        object: Box<Expr>,
        name: Token,
        value: Box<Expr>,
    },
    Super {
        keyword: Token,
        method: Token,
        id: usize,
    },
    This {
        keyword: Token,
        id: usize,
    },
    Unary {
        op: Token,
        right: Box<Expr>,
    },
    Variable {
        name: Token,
        id: usize,
    },
}

#[derive(Debug, Clone)]
pub enum Stmt {
    Block(Vec<Stmt>),
    Class {
        name: Token,
        superclass: Option<Expr>,
        methods: Vec<Stmt>,
    },
    Expression(Expr),
    Function {
        name: Token,
        params: Vec<Token>,
        body: Vec<Stmt>,
    },
    If {
        cond: Expr,
        then_branch: Box<Stmt>,
        else_branch: Option<Box<Stmt>>,
    },
    Print(Expr),
    Return {
        keyword: Token,
        value: Option<Expr>,
    },
    Var {
        name: Token,
        initializer: Option<Expr>,
    },
    While {
        cond: Expr,
        body: Box<Stmt>,
    },
}

/// Returns the resolver-annotation id for Expr variants that carry one.
pub fn expr_id(e: &Expr) -> Option<usize> {
    match e {
        Expr::Assign { id, .. }
        | Expr::Super { id, .. }
        | Expr::This { id, .. }
        | Expr::Variable { id, .. } => Some(*id),
        _ => None,
    }
}

#[cfg(test)]
mod ast_tests {
    use super::*;
    use crate::token::{Literal, Token, TokenType};

    fn tok(ttype: TokenType, lexeme: &str) -> Token {
        Token::new(ttype, lexeme, None, 1)
    }

    #[test]
    fn ast_expr_literal_constructs_and_debug_prints() {
        let e = Expr::Literal(Literal::Num(42.0));
        let s = format!("{:?}", e);
        assert!(s.contains("Literal"), "debug should name the variant: {s}");
        assert!(s.contains("42"), "debug should include the number: {s}");
    }

    #[test]
    fn ast_stmt_block_holds_children() {
        let block = Stmt::Block(vec![Stmt::Print(Expr::Literal(Literal::Num(1.0)))]);
        match &block {
            Stmt::Block(inner) => assert_eq!(inner.len(), 1),
            _ => panic!("expected Stmt::Block"),
        }
    }

    #[test]
    fn ast_expr_id_returns_some_for_variable_and_none_for_literal() {
        let var = Expr::Variable {
            name: tok(TokenType::Identifier, "x"),
            id: 7,
        };
        assert_eq!(expr_id(&var), Some(7));

        let lit = Expr::Literal(Literal::Nil);
        assert_eq!(expr_id(&lit), None);
    }

    #[test]
    fn ast_stmt_if_else_branch_optional() {
        // Without an else branch.
        let no_else = Stmt::If {
            cond: Expr::Literal(Literal::Bool(true)),
            then_branch: Box::new(Stmt::Print(Expr::Literal(Literal::Num(1.0)))),
            else_branch: None,
        };
        match &no_else {
            Stmt::If { else_branch, .. } => assert!(else_branch.is_none()),
            _ => panic!("expected Stmt::If"),
        }

        // With an else branch.
        let with_else = Stmt::If {
            cond: Expr::Literal(Literal::Bool(false)),
            then_branch: Box::new(Stmt::Print(Expr::Literal(Literal::Num(1.0)))),
            else_branch: Some(Box::new(Stmt::Print(Expr::Literal(Literal::Num(2.0))))),
        };
        match &with_else {
            Stmt::If { else_branch, .. } => assert!(else_branch.is_some()),
            _ => panic!("expected Stmt::If"),
        }
    }

    #[test]
    fn ast_expr_id_covers_assign_super_this() {
        let assign = Expr::Assign {
            name: tok(TokenType::Identifier, "x"),
            value: Box::new(Expr::Literal(Literal::Num(1.0))),
            id: 1,
        };
        assert_eq!(expr_id(&assign), Some(1));

        let super_ = Expr::Super {
            keyword: tok(TokenType::Super, "super"),
            method: tok(TokenType::Identifier, "m"),
            id: 2,
        };
        assert_eq!(expr_id(&super_), Some(2));

        let this_ = Expr::This {
            keyword: tok(TokenType::This, "this"),
            id: 3,
        };
        assert_eq!(expr_id(&this_), Some(3));
    }
}
