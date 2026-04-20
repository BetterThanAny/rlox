//! Recursive-descent parser for Lox.
//!
//! Mirrors the parser presented in *Crafting Interpreters* chapters 6, 8, 9,
//! 10, 12, and 13. Produces `Vec<Stmt>` on success, or accumulates every
//! syntax error encountered into `Vec<LoxError>` after synchronizing past
//! each failure.
//!
//! Error messages are kept byte-for-byte compatible with the book so that
//! future golden-file diff tests can be reused unchanged.

use crate::{
    ast::{Expr, Stmt},
    error::LoxError,
    token::{Literal, Token, TokenType},
};

/// Stateful recursive-descent parser.
pub struct Parser {
    tokens: Vec<Token>,
    /// Index of the next unconsumed token in `tokens`.
    current: usize,
    /// Monotonic id source used for resolver-annotated Expr nodes.
    next_id: usize,
}

impl Parser {
    pub fn new(tokens: Vec<Token>) -> Self {
        Self {
            tokens,
            current: 0,
            next_id: 0,
        }
    }

    /// Parse the entire token stream into a program.
    ///
    /// Accumulates every syntax error, synchronizing after each so that later
    /// independent declarations still get checked. Returns `Ok(stmts)` iff no
    /// errors were raised.
    pub fn parse(&mut self) -> Result<Vec<Stmt>, Vec<LoxError>> {
        let mut stmts = Vec::new();
        let mut errors = Vec::new();
        while !self.is_at_end() {
            match self.declaration() {
                Ok(s) => stmts.push(s),
                Err(e) => {
                    errors.push(e);
                    self.synchronize();
                }
            }
        }
        if errors.is_empty() {
            Ok(stmts)
        } else {
            Err(errors)
        }
    }

    // ---------- id minting ----------

    fn mint_id(&mut self) -> usize {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    // ---------- declarations ----------

    fn declaration(&mut self) -> Result<Stmt, LoxError> {
        if self.match_any(&[TokenType::Class]) {
            return self.class_declaration();
        }
        if self.match_any(&[TokenType::Fun]) {
            return self.function("function");
        }
        if self.match_any(&[TokenType::Var]) {
            return self.var_declaration();
        }
        self.statement()
    }

    fn class_declaration(&mut self) -> Result<Stmt, LoxError> {
        let name = self
            .consume(TokenType::Identifier, "Expect class name.")?
            .clone();

        let superclass = if self.match_any(&[TokenType::Less]) {
            let sc_name = self
                .consume(TokenType::Identifier, "Expect superclass name.")?
                .clone();
            let id = self.mint_id();
            Some(Expr::Variable { name: sc_name, id })
        } else {
            None
        };

        self.consume(TokenType::LeftBrace, "Expect '{' before class body.")?;
        let mut methods = Vec::new();
        while !self.check(&TokenType::RightBrace) && !self.is_at_end() {
            methods.push(self.function("method")?);
        }
        self.consume(TokenType::RightBrace, "Expect '}' after class body.")?;

        Ok(Stmt::Class {
            name,
            superclass,
            methods,
        })
    }

    fn var_declaration(&mut self) -> Result<Stmt, LoxError> {
        let name = self
            .consume(TokenType::Identifier, "Expect variable name.")?
            .clone();
        let initializer = if self.match_any(&[TokenType::Equal]) {
            Some(self.expression()?)
        } else {
            None
        };
        self.consume(
            TokenType::Semicolon,
            "Expect ';' after variable declaration.",
        )?;
        Ok(Stmt::Var { name, initializer })
    }

    /// Parse a function or method. `kind` is "function" or "method"; it feeds
    /// straight into error messages (`"Expect function name."`).
    fn function(&mut self, kind: &str) -> Result<Stmt, LoxError> {
        let name = self
            .consume(TokenType::Identifier, &format!("Expect {kind} name."))?
            .clone();
        self.consume(
            TokenType::LeftParen,
            &format!("Expect '(' after {kind} name."),
        )?;
        let mut params: Vec<Token> = Vec::new();
        if !self.check(&TokenType::RightParen) {
            loop {
                if params.len() >= 255 {
                    // Report but do not throw: mimic the book's `error()` helper.
                    let tok = self.peek().clone();
                    let err = Self::error_at(&tok, "Can't have more than 255 parameters.");
                    return Err(err);
                }
                let p = self
                    .consume(TokenType::Identifier, "Expect parameter name.")?
                    .clone();
                params.push(p);
                if !self.match_any(&[TokenType::Comma]) {
                    break;
                }
            }
        }
        self.consume(TokenType::RightParen, "Expect ')' after parameters.")?;
        self.consume(
            TokenType::LeftBrace,
            &format!("Expect '{{' before {kind} body."),
        )?;
        let body = self.block()?;
        Ok(Stmt::Function { name, params, body })
    }

    // ---------- statements ----------

    fn statement(&mut self) -> Result<Stmt, LoxError> {
        if self.match_any(&[TokenType::For]) {
            return self.for_statement();
        }
        if self.match_any(&[TokenType::If]) {
            return self.if_statement();
        }
        if self.match_any(&[TokenType::Print]) {
            return self.print_statement();
        }
        if self.match_any(&[TokenType::Return]) {
            return self.return_statement();
        }
        if self.match_any(&[TokenType::While]) {
            return self.while_statement();
        }
        if self.match_any(&[TokenType::LeftBrace]) {
            let stmts = self.block()?;
            return Ok(Stmt::Block(stmts));
        }
        self.expression_statement()
    }

    fn for_statement(&mut self) -> Result<Stmt, LoxError> {
        self.consume(TokenType::LeftParen, "Expect '(' after 'for'.")?;

        let initializer = if self.match_any(&[TokenType::Semicolon]) {
            None
        } else if self.match_any(&[TokenType::Var]) {
            Some(self.var_declaration()?)
        } else {
            Some(self.expression_statement()?)
        };

        let condition = if !self.check(&TokenType::Semicolon) {
            Some(self.expression()?)
        } else {
            None
        };
        self.consume(TokenType::Semicolon, "Expect ';' after loop condition.")?;

        let increment = if !self.check(&TokenType::RightParen) {
            Some(self.expression()?)
        } else {
            None
        };
        self.consume(TokenType::RightParen, "Expect ')' after for clauses.")?;

        let mut body = self.statement()?;

        // Desugar to a while loop, per book (ch. 9).
        if let Some(inc) = increment {
            body = Stmt::Block(vec![body, Stmt::Expression(inc)]);
        }
        let cond = condition.unwrap_or(Expr::Literal(Literal::Bool(true)));
        body = Stmt::While {
            cond,
            body: Box::new(body),
        };
        if let Some(init) = initializer {
            body = Stmt::Block(vec![init, body]);
        }

        Ok(body)
    }

    fn if_statement(&mut self) -> Result<Stmt, LoxError> {
        self.consume(TokenType::LeftParen, "Expect '(' after 'if'.")?;
        let cond = self.expression()?;
        self.consume(TokenType::RightParen, "Expect ')' after if condition.")?;
        let then_branch = Box::new(self.statement()?);
        let else_branch = if self.match_any(&[TokenType::Else]) {
            Some(Box::new(self.statement()?))
        } else {
            None
        };
        Ok(Stmt::If {
            cond,
            then_branch,
            else_branch,
        })
    }

    fn print_statement(&mut self) -> Result<Stmt, LoxError> {
        let value = self.expression()?;
        self.consume(TokenType::Semicolon, "Expect ';' after value.")?;
        Ok(Stmt::Print(value))
    }

    fn return_statement(&mut self) -> Result<Stmt, LoxError> {
        let keyword = self.previous().clone();
        let value = if !self.check(&TokenType::Semicolon) {
            Some(self.expression()?)
        } else {
            None
        };
        self.consume(TokenType::Semicolon, "Expect ';' after return value.")?;
        Ok(Stmt::Return { keyword, value })
    }

    fn while_statement(&mut self) -> Result<Stmt, LoxError> {
        self.consume(TokenType::LeftParen, "Expect '(' after 'while'.")?;
        let cond = self.expression()?;
        self.consume(TokenType::RightParen, "Expect ')' after condition.")?;
        let body = Box::new(self.statement()?);
        Ok(Stmt::While { cond, body })
    }

    fn expression_statement(&mut self) -> Result<Stmt, LoxError> {
        let expr = self.expression()?;
        self.consume(TokenType::Semicolon, "Expect ';' after expression.")?;
        Ok(Stmt::Expression(expr))
    }

    fn block(&mut self) -> Result<Vec<Stmt>, LoxError> {
        let mut stmts = Vec::new();
        while !self.check(&TokenType::RightBrace) && !self.is_at_end() {
            stmts.push(self.declaration()?);
        }
        self.consume(TokenType::RightBrace, "Expect '}' after block.")?;
        Ok(stmts)
    }

    // ---------- expressions ----------

    fn expression(&mut self) -> Result<Expr, LoxError> {
        self.assignment()
    }

    fn assignment(&mut self) -> Result<Expr, LoxError> {
        let expr = self.or()?;

        if self.match_any(&[TokenType::Equal]) {
            let equals = self.previous().clone();
            let value = self.assignment()?;

            match expr {
                Expr::Variable { name, .. } => {
                    let id = self.mint_id();
                    return Ok(Expr::Assign {
                        name,
                        value: Box::new(value),
                        id,
                    });
                }
                Expr::Get { object, name } => {
                    return Ok(Expr::Set {
                        object,
                        name,
                        value: Box::new(value),
                    });
                }
                _ => {
                    // Per book: report but do not throw; continue parsing.
                    return Err(Self::error_at(&equals, "Invalid assignment target."));
                }
            }
        }

        Ok(expr)
    }

    fn or(&mut self) -> Result<Expr, LoxError> {
        let mut expr = self.and()?;
        while self.match_any(&[TokenType::Or]) {
            let op = self.previous().clone();
            let right = self.and()?;
            expr = Expr::Logical {
                left: Box::new(expr),
                op,
                right: Box::new(right),
            };
        }
        Ok(expr)
    }

    fn and(&mut self) -> Result<Expr, LoxError> {
        let mut expr = self.equality()?;
        while self.match_any(&[TokenType::And]) {
            let op = self.previous().clone();
            let right = self.equality()?;
            expr = Expr::Logical {
                left: Box::new(expr),
                op,
                right: Box::new(right),
            };
        }
        Ok(expr)
    }

    fn equality(&mut self) -> Result<Expr, LoxError> {
        let mut expr = self.comparison()?;
        while self.match_any(&[TokenType::BangEqual, TokenType::EqualEqual]) {
            let op = self.previous().clone();
            let right = self.comparison()?;
            expr = Expr::Binary {
                left: Box::new(expr),
                op,
                right: Box::new(right),
            };
        }
        Ok(expr)
    }

    fn comparison(&mut self) -> Result<Expr, LoxError> {
        let mut expr = self.term()?;
        while self.match_any(&[
            TokenType::Greater,
            TokenType::GreaterEqual,
            TokenType::Less,
            TokenType::LessEqual,
        ]) {
            let op = self.previous().clone();
            let right = self.term()?;
            expr = Expr::Binary {
                left: Box::new(expr),
                op,
                right: Box::new(right),
            };
        }
        Ok(expr)
    }

    fn term(&mut self) -> Result<Expr, LoxError> {
        let mut expr = self.factor()?;
        while self.match_any(&[TokenType::Minus, TokenType::Plus]) {
            let op = self.previous().clone();
            let right = self.factor()?;
            expr = Expr::Binary {
                left: Box::new(expr),
                op,
                right: Box::new(right),
            };
        }
        Ok(expr)
    }

    fn factor(&mut self) -> Result<Expr, LoxError> {
        let mut expr = self.unary()?;
        while self.match_any(&[TokenType::Slash, TokenType::Star]) {
            let op = self.previous().clone();
            let right = self.unary()?;
            expr = Expr::Binary {
                left: Box::new(expr),
                op,
                right: Box::new(right),
            };
        }
        Ok(expr)
    }

    fn unary(&mut self) -> Result<Expr, LoxError> {
        if self.match_any(&[TokenType::Bang, TokenType::Minus]) {
            let op = self.previous().clone();
            let right = self.unary()?;
            return Ok(Expr::Unary {
                op,
                right: Box::new(right),
            });
        }
        self.call()
    }

    fn call(&mut self) -> Result<Expr, LoxError> {
        let mut expr = self.primary()?;
        loop {
            if self.match_any(&[TokenType::LeftParen]) {
                expr = self.finish_call(expr)?;
            } else if self.match_any(&[TokenType::Dot]) {
                let name = self
                    .consume(TokenType::Identifier, "Expect property name after '.'.")?
                    .clone();
                expr = Expr::Get {
                    object: Box::new(expr),
                    name,
                };
            } else {
                break;
            }
        }
        Ok(expr)
    }

    fn finish_call(&mut self, callee: Expr) -> Result<Expr, LoxError> {
        let mut args: Vec<Expr> = Vec::new();
        if !self.check(&TokenType::RightParen) {
            loop {
                if args.len() >= 255 {
                    let tok = self.peek().clone();
                    return Err(Self::error_at(&tok, "Can't have more than 255 arguments."));
                }
                args.push(self.expression()?);
                if !self.match_any(&[TokenType::Comma]) {
                    break;
                }
            }
        }
        let paren = self
            .consume(TokenType::RightParen, "Expect ')' after arguments.")?
            .clone();
        Ok(Expr::Call {
            callee: Box::new(callee),
            paren,
            args,
        })
    }

    fn primary(&mut self) -> Result<Expr, LoxError> {
        if self.match_any(&[TokenType::False]) {
            return Ok(Expr::Literal(Literal::Bool(false)));
        }
        if self.match_any(&[TokenType::True]) {
            return Ok(Expr::Literal(Literal::Bool(true)));
        }
        if self.match_any(&[TokenType::Nil]) {
            return Ok(Expr::Literal(Literal::Nil));
        }
        if self.match_any(&[TokenType::Number, TokenType::String]) {
            let lit = self.previous().literal.clone().unwrap_or(Literal::Nil);
            return Ok(Expr::Literal(lit));
        }
        if self.match_any(&[TokenType::Super]) {
            let keyword = self.previous().clone();
            self.consume(TokenType::Dot, "Expect '.' after 'super'.")?;
            let method = self
                .consume(TokenType::Identifier, "Expect superclass method name.")?
                .clone();
            let id = self.mint_id();
            return Ok(Expr::Super {
                keyword,
                method,
                id,
            });
        }
        if self.match_any(&[TokenType::This]) {
            let keyword = self.previous().clone();
            let id = self.mint_id();
            return Ok(Expr::This { keyword, id });
        }
        if self.match_any(&[TokenType::Identifier]) {
            let name = self.previous().clone();
            let id = self.mint_id();
            return Ok(Expr::Variable { name, id });
        }
        if self.match_any(&[TokenType::LeftParen]) {
            let expr = self.expression()?;
            self.consume(TokenType::RightParen, "Expect ')' after expression.")?;
            return Ok(Expr::Grouping(Box::new(expr)));
        }

        let tok = self.peek().clone();
        Err(Self::error_at(&tok, "Expect expression."))
    }

    // ---------- helpers ----------

    fn match_any(&mut self, types: &[TokenType]) -> bool {
        for t in types {
            if self.check(t) {
                self.advance();
                return true;
            }
        }
        false
    }

    fn check(&self, ttype: &TokenType) -> bool {
        if self.is_at_end() {
            return false;
        }
        &self.peek().ttype == ttype
    }

    fn advance(&mut self) -> &Token {
        if !self.is_at_end() {
            self.current += 1;
        }
        self.previous()
    }

    fn is_at_end(&self) -> bool {
        self.peek().ttype == TokenType::Eof
    }

    fn peek(&self) -> &Token {
        &self.tokens[self.current]
    }

    fn previous(&self) -> &Token {
        &self.tokens[self.current - 1]
    }

    fn consume(&mut self, ttype: TokenType, msg: &str) -> Result<&Token, LoxError> {
        if self.check(&ttype) {
            return Ok(self.advance());
        }
        let tok = self.peek().clone();
        Err(Self::error_at(&tok, msg))
    }

    fn error_at(tok: &Token, msg: &str) -> LoxError {
        let loc = if tok.ttype == TokenType::Eof {
            " at end".to_string()
        } else {
            format!(" at '{}'", tok.lexeme)
        };
        LoxError::syntax(tok.line, loc, msg)
    }

    /// Skip tokens until a likely statement boundary so parsing can resume
    /// after an error. Book chapter 6 §6.3.3.
    fn synchronize(&mut self) {
        self.advance();
        while !self.is_at_end() {
            if self.previous().ttype == TokenType::Semicolon {
                return;
            }
            match self.peek().ttype {
                TokenType::Class
                | TokenType::Fun
                | TokenType::Var
                | TokenType::For
                | TokenType::If
                | TokenType::While
                | TokenType::Print
                | TokenType::Return => return,
                _ => {}
            }
            self.advance();
        }
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod parser_tests {
    use super::*;
    use crate::ast::expr_id;
    use crate::scanner::Scanner;

    fn parse(src: &str) -> Result<Vec<Stmt>, Vec<LoxError>> {
        let tokens = Scanner::new(src)
            .scan_tokens()
            .expect("test source must scan cleanly");
        Parser::new(tokens).parse()
    }

    fn parse_ok(src: &str) -> Vec<Stmt> {
        parse(src).unwrap_or_else(|e| panic!("unexpected parse error: {:?}", e))
    }

    // --- precedence / associativity ---------------------------------------

    #[test]
    fn parser_arithmetic_precedence() {
        // 1 + 2 * 3  →  Binary(Plus, 1, Binary(Star, 2, 3))
        let stmts = parse_ok("1 + 2 * 3;");
        assert_eq!(stmts.len(), 1);
        let Stmt::Expression(Expr::Binary { op, right, .. }) = &stmts[0] else {
            panic!("expected Expression(Binary), got {:?}", stmts[0]);
        };
        assert_eq!(op.ttype, TokenType::Plus);
        let Expr::Binary { op: op2, .. } = right.as_ref() else {
            panic!("expected nested Binary on RHS");
        };
        assert_eq!(op2.ttype, TokenType::Star);
    }

    #[test]
    fn parser_grouping_overrides_precedence() {
        // (1 + 2) * 3  →  Binary(Star, Grouping(Binary(Plus,...)), 3)
        let stmts = parse_ok("(1 + 2) * 3;");
        let Stmt::Expression(Expr::Binary { op, left, .. }) = &stmts[0] else {
            panic!("expected Expression(Binary)");
        };
        assert_eq!(op.ttype, TokenType::Star);
        let Expr::Grouping(inner) = left.as_ref() else {
            panic!("expected Grouping on LHS");
        };
        let Expr::Binary { op: op2, .. } = inner.as_ref() else {
            panic!("expected Binary inside Grouping");
        };
        assert_eq!(op2.ttype, TokenType::Plus);
    }

    #[test]
    fn parser_equality_left_associative() {
        // 1 == 2 == 3  →  Binary(==, Binary(==, 1, 2), 3)
        let stmts = parse_ok("1 == 2 == 3;");
        let Stmt::Expression(Expr::Binary { op, left, .. }) = &stmts[0] else {
            panic!("expected Expression(Binary)");
        };
        assert_eq!(op.ttype, TokenType::EqualEqual);
        let Expr::Binary { op: op2, .. } = left.as_ref() else {
            panic!("expected nested Binary on LHS for left-associativity");
        };
        assert_eq!(op2.ttype, TokenType::EqualEqual);
    }

    #[test]
    fn parser_logical_or_and_short_circuit_structure() {
        // a or b and c  →  Logical(Or, a, Logical(And, b, c))
        let stmts = parse_ok("a or b and c;");
        let Stmt::Expression(Expr::Logical { op, right, .. }) = &stmts[0] else {
            panic!("expected Expression(Logical)");
        };
        assert_eq!(op.ttype, TokenType::Or);
        let Expr::Logical { op: op2, .. } = right.as_ref() else {
            panic!("expected nested Logical on RHS");
        };
        assert_eq!(op2.ttype, TokenType::And);
    }

    #[test]
    fn parser_unary_right_associative() {
        // !!x  →  Unary(!, Unary(!, x))
        let stmts = parse_ok("!!x;");
        let Stmt::Expression(Expr::Unary { op, right }) = &stmts[0] else {
            panic!("expected Expression(Unary)");
        };
        assert_eq!(op.ttype, TokenType::Bang);
        let Expr::Unary { op: op2, .. } = right.as_ref() else {
            panic!("expected nested Unary");
        };
        assert_eq!(op2.ttype, TokenType::Bang);
    }

    // --- declarations ------------------------------------------------------

    #[test]
    fn parser_var_declaration_with_initializer() {
        let stmts = parse_ok("var x = 1;");
        let Stmt::Var { name, initializer } = &stmts[0] else {
            panic!("expected Stmt::Var");
        };
        assert_eq!(name.lexeme, "x");
        assert!(initializer.is_some(), "expected initializer");
    }

    #[test]
    fn parser_var_declaration_without_initializer() {
        let stmts = parse_ok("var x;");
        let Stmt::Var { name, initializer } = &stmts[0] else {
            panic!("expected Stmt::Var");
        };
        assert_eq!(name.lexeme, "x");
        assert!(initializer.is_none());
    }

    #[test]
    fn parser_block_nested_statements() {
        let stmts = parse_ok("{ var a = 1; print a; }");
        let Stmt::Block(inner) = &stmts[0] else {
            panic!("expected Stmt::Block");
        };
        assert_eq!(inner.len(), 2);
        assert!(matches!(inner[0], Stmt::Var { .. }));
        assert!(matches!(inner[1], Stmt::Print(_)));
    }

    // --- control flow ------------------------------------------------------

    #[test]
    fn parser_if_else_optional_else() {
        // Without else.
        let stmts = parse_ok("if (a) print 1;");
        let Stmt::If { else_branch, .. } = &stmts[0] else {
            panic!("expected Stmt::If");
        };
        assert!(else_branch.is_none());

        // With else.
        let stmts = parse_ok("if (a) print 1; else print 2;");
        let Stmt::If { else_branch, .. } = &stmts[0] else {
            panic!("expected Stmt::If");
        };
        assert!(else_branch.is_some());
    }

    #[test]
    fn parser_while_loop() {
        let stmts = parse_ok("while (a) print a;");
        let Stmt::While { body, .. } = &stmts[0] else {
            panic!("expected Stmt::While");
        };
        assert!(matches!(body.as_ref(), Stmt::Print(_)));
    }

    #[test]
    fn parser_for_desugared_to_block_while() {
        // for (var i = 0; i < 3; i = i + 1) print i;
        // Desugars to:
        //   Block[
        //     Var(i = 0),
        //     While(i < 3, Block[Print(i), Expression(i = i + 1)])
        //   ]
        let stmts = parse_ok("for (var i = 0; i < 3; i = i + 1) print i;");
        let Stmt::Block(outer) = &stmts[0] else {
            panic!("expected outer Stmt::Block (desugared for)");
        };
        assert_eq!(outer.len(), 2);
        assert!(matches!(outer[0], Stmt::Var { .. }));
        let Stmt::While { body, .. } = &outer[1] else {
            panic!("expected Stmt::While in desugared for");
        };
        let Stmt::Block(loop_body) = body.as_ref() else {
            panic!("expected inner Block wrapping body + increment");
        };
        assert_eq!(loop_body.len(), 2);
        assert!(matches!(loop_body[0], Stmt::Print(_)));
        assert!(matches!(loop_body[1], Stmt::Expression(_)));
    }

    // --- functions / calls -------------------------------------------------

    #[test]
    fn parser_function_declaration() {
        let stmts = parse_ok("fun foo(a, b) { return a + b; }");
        let Stmt::Function { name, params, body } = &stmts[0] else {
            panic!("expected Stmt::Function");
        };
        assert_eq!(name.lexeme, "foo");
        assert_eq!(params.len(), 2);
        assert_eq!(params[0].lexeme, "a");
        assert_eq!(params[1].lexeme, "b");
        assert_eq!(body.len(), 1);
        assert!(matches!(body[0], Stmt::Return { .. }));
    }

    #[test]
    fn parser_call_expression() {
        let stmts = parse_ok("foo(1, 2);");
        let Stmt::Expression(Expr::Call { args, .. }) = &stmts[0] else {
            panic!("expected Expression(Call)");
        };
        assert_eq!(args.len(), 2);
    }

    // --- property access + assignment -------------------------------------

    #[test]
    fn parser_property_access() {
        // a.b.c  →  Get(Get(Variable(a), b), c)
        let stmts = parse_ok("a.b.c;");
        let Stmt::Expression(Expr::Get { object, name }) = &stmts[0] else {
            panic!("expected Expression(Get)");
        };
        assert_eq!(name.lexeme, "c");
        let Expr::Get { name: name2, .. } = object.as_ref() else {
            panic!("expected nested Get");
        };
        assert_eq!(name2.lexeme, "b");
    }

    #[test]
    fn parser_property_assignment() {
        // a.b = 1  →  Expression(Set { object=Variable(a), name=b, value=1 })
        let stmts = parse_ok("a.b = 1;");
        let Stmt::Expression(Expr::Set { name, .. }) = &stmts[0] else {
            panic!("expected Expression(Set)");
        };
        assert_eq!(name.lexeme, "b");
    }

    // --- classes ----------------------------------------------------------

    #[test]
    fn parser_class_declaration_with_methods() {
        let stmts = parse_ok("class A { foo() {} bar() {} }");
        let Stmt::Class {
            name,
            superclass,
            methods,
        } = &stmts[0]
        else {
            panic!("expected Stmt::Class");
        };
        assert_eq!(name.lexeme, "A");
        assert!(superclass.is_none());
        assert_eq!(methods.len(), 2);
        assert!(matches!(methods[0], Stmt::Function { .. }));
        assert!(matches!(methods[1], Stmt::Function { .. }));
    }

    #[test]
    fn parser_class_with_superclass() {
        let stmts = parse_ok("class B < A {}");
        let Stmt::Class {
            name, superclass, ..
        } = &stmts[0]
        else {
            panic!("expected Stmt::Class");
        };
        assert_eq!(name.lexeme, "B");
        let Some(Expr::Variable { name: sn, .. }) = superclass else {
            panic!("expected Some(Variable) superclass");
        };
        assert_eq!(sn.lexeme, "A");
    }

    #[test]
    fn parser_super_call() {
        // `super.init();` inside a method. Test just checks the expression
        // shape; resolver enforces the "must be in subclass" rule later.
        let stmts = parse_ok("class B < A { foo() { super.init(); } }");
        let Stmt::Class { methods, .. } = &stmts[0] else {
            panic!("expected Stmt::Class");
        };
        let Stmt::Function { body, .. } = &methods[0] else {
            panic!("expected Stmt::Function method");
        };
        let Stmt::Expression(Expr::Call { callee, .. }) = &body[0] else {
            panic!("expected Call stmt in method body");
        };
        let Expr::Super { method, .. } = callee.as_ref() else {
            panic!("expected Super callee, got {:?}", callee);
        };
        assert_eq!(method.lexeme, "init");
    }

    #[test]
    fn parser_this_expression() {
        // `this.x;` parses as Get { object: This, name: x }.
        let stmts = parse_ok("class A { foo() { this.x; } }");
        let Stmt::Class { methods, .. } = &stmts[0] else {
            panic!("expected Stmt::Class");
        };
        let Stmt::Function { body, .. } = &methods[0] else {
            panic!("expected Function method");
        };
        let Stmt::Expression(Expr::Get { object, name }) = &body[0] else {
            panic!("expected Expression(Get)");
        };
        assert_eq!(name.lexeme, "x");
        assert!(matches!(object.as_ref(), Expr::This { .. }));
    }

    // --- errors -----------------------------------------------------------

    #[test]
    fn parser_invalid_assignment_target_errors() {
        // `(a + b) = 1;` — LHS is a grouping, not assignable.
        let errs = parse("(a + b) = 1;").unwrap_err();
        assert!(
            errs.iter()
                .any(|e| e.to_string().contains("Invalid assignment target.")),
            "errors: {:?}",
            errs
        );
    }

    #[test]
    fn parser_missing_semicolon_errors_and_recovers() {
        // First statement missing `;`; parser should error then recover and
        // still parse the `print a;` that follows.
        let errs = parse("var a = 1\nprint a;").unwrap_err();
        assert!(!errs.is_empty(), "expected at least one error");
        // Error should reference semicolon expectation.
        assert!(
            errs.iter().any(|e| e.to_string().contains("';'")),
            "errors: {:?}",
            errs
        );
    }

    #[test]
    fn parser_too_many_arguments_errors() {
        // Build `foo(1, 1, ..., 1);` with 256 arguments.
        let args: Vec<&str> = (0..256).map(|_| "1").collect();
        let src = format!("foo({});", args.join(","));
        let errs = parse(&src).unwrap_err();
        assert!(
            errs.iter().any(|e| e
                .to_string()
                .contains("Can't have more than 255 arguments.")),
            "errors: {:?}",
            errs
        );
    }

    #[test]
    fn parser_mints_unique_expr_ids() {
        // `var a; a = a;`
        //   - stmt 0: Var { name=a }
        //   - stmt 1: Expression(Assign { name=a, value=Variable(a) })
        // We expect the Variable on the RHS and the Assign to both carry Some(id)
        // and to be distinct.
        let stmts = parse_ok("var a; a = a;");
        assert_eq!(stmts.len(), 2);
        let Stmt::Expression(Expr::Assign { id, value, .. }) = &stmts[1] else {
            panic!("expected Expression(Assign)");
        };
        let Expr::Variable { id: var_id, .. } = value.as_ref() else {
            panic!("expected Variable on RHS");
        };
        // Cross-check via expr_id helper too.
        let assign_id_opt = expr_id(&stmts_to_expr(&stmts[1]));
        assert_eq!(assign_id_opt, Some(*id));
        assert_ne!(id, var_id, "ids must be distinct");
    }

    fn stmts_to_expr(s: &Stmt) -> Expr {
        match s {
            Stmt::Expression(e) => e.clone(),
            _ => panic!("not an Expression stmt"),
        }
    }
}
