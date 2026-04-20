//! Single-pass Pratt compiler producing bytecode chunks. Port of
//! *Crafting Interpreters* chapters 17-25 (chapter 26+ classes land in M6).
//!
//! Design notes:
//! * The compiler owns a stack of `CompilerState` frames, one per function
//!   currently being compiled. Nested function declarations push a new frame
//!   via [`Compiler::push_function`]; the frame pops when the body closes.
//! * Scope-depth tracking and local-resolution algorithms are verbatim book.
//! * Upvalue resolution walks frames from innermost outward, adding upvalues
//!   to each enclosing function as necessary (book chapter 25).
//! * Error handling mirrors the book's panic-mode + synchronize flow; errors
//!   accumulate inside `Parser::errors` and are returned as a `Vec<String>`
//!   from [`compile`].

use std::rc::Rc;

use crate::chunk::{Chunk, OpCode};
use crate::scanner::{Scanner, Token, TokenType};
use crate::value::{ObjFunction, Value};

/// Precedence levels, in ascending order (book table 17.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[repr(u8)]
enum Precedence {
    None,
    Assignment, // =
    Or,         // or
    And,        // and
    Equality,   // == !=
    Comparison, // < > <= >=
    Term,       // + -
    Factor,     // * /
    Unary,      // ! -
    Call,       // . ()
    Primary,
}

impl Precedence {
    fn next(self) -> Precedence {
        match self {
            Precedence::None => Precedence::Assignment,
            Precedence::Assignment => Precedence::Or,
            Precedence::Or => Precedence::And,
            Precedence::And => Precedence::Equality,
            Precedence::Equality => Precedence::Comparison,
            Precedence::Comparison => Precedence::Term,
            Precedence::Term => Precedence::Factor,
            Precedence::Factor => Precedence::Unary,
            Precedence::Unary => Precedence::Call,
            Precedence::Call => Precedence::Primary,
            Precedence::Primary => Precedence::Primary,
        }
    }
}

/// Kind of the function currently being compiled. Drives slot-zero handling
/// and return-statement validity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FunctionType {
    /// Top-level `<script>` pseudo-function.
    Script,
    /// Regular `fun` declaration.
    Function,
}

/// A local variable entry inside the current function's locals array.
#[derive(Debug, Clone)]
struct Local {
    name: Token,
    /// -1 = declared but not yet initialized; >=0 = scope depth at which the
    /// local is visible.
    depth: i32,
    /// True when some enclosed function has captured this local — on scope
    /// exit we emit `OP_CLOSE_UPVALUE` instead of `OP_POP`. Not wired in M5
    /// (upvalues use Rc cells resolved at runtime via the OP_CLOSURE operand),
    /// retained for book parity and future use.
    is_captured: bool,
}

/// A single upvalue descriptor resolved during compilation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Upvalue {
    index: u8,
    is_local: bool,
}

/// Per-function compiler state.
struct CompilerState {
    function: ObjFunction,
    fn_type: FunctionType,
    locals: Vec<Local>,
    scope_depth: i32,
    upvalues: Vec<Upvalue>,
}

impl CompilerState {
    fn new(fn_type: FunctionType, name: Option<Rc<String>>) -> Self {
        let mut state = Self {
            function: ObjFunction::new(name),
            fn_type,
            locals: Vec::with_capacity(8),
            scope_depth: 0,
            upvalues: Vec::new(),
        };
        // Slot zero is reserved for the callee itself (book chapter 24). We
        // model it with an empty-named synthetic local at depth 0 so locals
        // indexing stays aligned with book semantics.
        state.locals.push(Local {
            name: Token {
                ttype: TokenType::Identifier,
                lexeme: String::new(),
                line: 0,
            },
            depth: 0,
            is_captured: false,
        });
        state
    }

    fn chunk_mut(&mut self) -> &mut Chunk {
        &mut self.function.chunk
    }
}

/// Parse-state record. Wraps the scanner and carries book-equivalent flags.
struct Parser<'src> {
    scanner: Scanner<'src>,
    current: Token,
    previous: Token,
    had_error: bool,
    panic_mode: bool,
    errors: Vec<String>,
}

impl<'src> Parser<'src> {
    fn new(source: &'src str) -> Self {
        let mut scanner = Scanner::new(source);
        // Prime `current` with the first real token; `previous` is a placeholder.
        let first = scanner.scan_token();
        Self {
            scanner,
            previous: Token {
                ttype: TokenType::Eof,
                lexeme: String::new(),
                line: 0,
            },
            current: first,
            had_error: false,
            panic_mode: false,
            errors: Vec::new(),
        }
    }
}

/// Top-level compiler. Owns the frame stack (`states`) plus a parser. The
/// innermost frame is `states.last_mut()`.
pub struct Compiler<'src> {
    parser: Parser<'src>,
    states: Vec<CompilerState>,
}

/// Public entry point: compile `source` into an `ObjFunction` wrapping the
/// top-level chunk. Returns every accumulated error message on failure.
pub fn compile(source: &str) -> Result<Rc<ObjFunction>, Vec<String>> {
    let mut c = Compiler::new(source);
    c.compile_script()
}

impl<'src> Compiler<'src> {
    fn new(source: &'src str) -> Self {
        let parser = Parser::new(source);
        let script_state = CompilerState::new(FunctionType::Script, None);
        Self {
            parser,
            states: vec![script_state],
        }
    }

    fn compile_script(mut self) -> Result<Rc<ObjFunction>, Vec<String>> {
        // Skip any leading `Error` token pre-first-declaration.
        self.advance_if_error();

        while !self.check(TokenType::Eof) {
            self.declaration();
        }

        self.emit_return();

        if self.parser.had_error {
            Err(std::mem::take(&mut self.parser.errors))
        } else {
            let state = self.states.pop().expect("script state present");
            Ok(Rc::new(state.function))
        }
    }

    // ---------- parser driver ----------

    fn advance(&mut self) {
        self.parser.previous = std::mem::replace(
            &mut self.parser.current,
            Token {
                ttype: TokenType::Eof,
                lexeme: String::new(),
                line: 0,
            },
        );
        loop {
            self.parser.current = self.parser.scanner.scan_token();
            if self.parser.current.ttype != TokenType::Error {
                break;
            }
            let msg = self.parser.current.lexeme.clone();
            let line = self.parser.current.line;
            self.error_at_current(&msg, line);
        }
    }

    fn advance_if_error(&mut self) {
        while self.parser.current.ttype == TokenType::Error {
            let msg = self.parser.current.lexeme.clone();
            let line = self.parser.current.line;
            self.error_at_current(&msg, line);
            self.parser.current = self.parser.scanner.scan_token();
        }
    }

    fn consume(&mut self, ttype: TokenType, msg: &str) {
        if self.parser.current.ttype == ttype {
            self.advance();
            return;
        }
        let line = self.parser.current.line;
        self.error_at_current(msg, line);
    }

    fn check(&self, ttype: TokenType) -> bool {
        self.parser.current.ttype == ttype
    }

    fn match_tok(&mut self, ttype: TokenType) -> bool {
        if !self.check(ttype) {
            return false;
        }
        self.advance();
        true
    }

    // ---------- error reporting ----------

    fn error_at_current(&mut self, msg: &str, line: usize) {
        if self.parser.panic_mode {
            return;
        }
        self.parser.panic_mode = true;
        self.parser.had_error = true;
        self.parser
            .errors
            .push(format!("[line {line}] Error: {msg}"));
    }

    fn error(&mut self, msg: &str) {
        let line = self.parser.previous.line;
        if self.parser.panic_mode {
            return;
        }
        self.parser.panic_mode = true;
        self.parser.had_error = true;
        self.parser
            .errors
            .push(format!("[line {line}] Error: {msg}"));
    }

    // ---------- emit helpers ----------

    fn state_mut(&mut self) -> &mut CompilerState {
        self.states.last_mut().expect("compiler state non-empty")
    }

    fn state(&self) -> &CompilerState {
        self.states.last().expect("compiler state non-empty")
    }

    fn emit_byte(&mut self, byte: u8) {
        let line = self.parser.previous.line;
        self.state_mut().chunk_mut().write_byte(byte, line);
    }

    fn emit_op(&mut self, op: OpCode) {
        self.emit_byte(op.as_byte());
    }

    fn emit_bytes(&mut self, b1: u8, b2: u8) {
        self.emit_byte(b1);
        self.emit_byte(b2);
    }

    fn emit_op_byte(&mut self, op: OpCode, operand: u8) {
        self.emit_op(op);
        self.emit_byte(operand);
    }

    fn emit_return(&mut self) {
        // Implicit nil-then-return.
        self.emit_op(OpCode::Nil);
        self.emit_op(OpCode::Return);
    }

    fn make_constant(&mut self, value: Value) -> u8 {
        let idx = self.state_mut().chunk_mut().add_constant(value);
        if idx > u8::MAX as usize {
            self.error("Too many constants in one chunk.");
            return 0;
        }
        idx as u8
    }

    fn emit_constant(&mut self, value: Value) {
        let idx = self.make_constant(value);
        self.emit_op_byte(OpCode::Constant, idx);
    }

    fn emit_jump(&mut self, op: OpCode) -> usize {
        self.emit_op(op);
        // Two-byte placeholder patched in `patch_jump`.
        self.emit_byte(0xff);
        self.emit_byte(0xff);
        self.state_mut().chunk_mut().code.len() - 2
    }

    fn patch_jump(&mut self, offset: usize) {
        // `-2` to compensate for the jump offset bytes themselves.
        let jump = self.state_mut().chunk_mut().code.len() - offset - 2;
        if jump > u16::MAX as usize {
            self.error("Too much code to jump over.");
        }
        let chunk = self.state_mut().chunk_mut();
        chunk.code[offset] = ((jump >> 8) & 0xff) as u8;
        chunk.code[offset + 1] = (jump & 0xff) as u8;
    }

    fn emit_loop(&mut self, loop_start: usize) {
        self.emit_op(OpCode::Loop);
        let offset = self.state_mut().chunk_mut().code.len() - loop_start + 2;
        if offset > u16::MAX as usize {
            self.error("Loop body too large.");
        }
        self.emit_byte(((offset >> 8) & 0xff) as u8);
        self.emit_byte((offset & 0xff) as u8);
    }

    // ---------- declaration dispatch ----------

    fn declaration(&mut self) {
        if self.match_tok(TokenType::Fun) {
            self.fun_declaration();
        } else if self.match_tok(TokenType::Var) {
            self.var_declaration();
        } else {
            self.statement();
        }

        if self.parser.panic_mode {
            self.synchronize();
        }
    }

    fn statement(&mut self) {
        if self.match_tok(TokenType::Print) {
            self.print_statement();
        } else if self.match_tok(TokenType::If) {
            self.if_statement();
        } else if self.match_tok(TokenType::While) {
            self.while_statement();
        } else if self.match_tok(TokenType::For) {
            self.for_statement();
        } else if self.match_tok(TokenType::Return) {
            self.return_statement();
        } else if self.match_tok(TokenType::LeftBrace) {
            self.begin_scope();
            self.block();
            self.end_scope();
        } else {
            self.expression_statement();
        }
    }

    fn print_statement(&mut self) {
        self.expression();
        self.consume(TokenType::Semicolon, "Expect ';' after value.");
        self.emit_op(OpCode::Print);
    }

    fn expression_statement(&mut self) {
        self.expression();
        self.consume(TokenType::Semicolon, "Expect ';' after expression.");
        self.emit_op(OpCode::Pop);
    }

    fn if_statement(&mut self) {
        self.consume(TokenType::LeftParen, "Expect '(' after 'if'.");
        self.expression();
        self.consume(TokenType::RightParen, "Expect ')' after condition.");

        let then_jump = self.emit_jump(OpCode::JumpIfFalse);
        self.emit_op(OpCode::Pop);
        self.statement();
        let else_jump = self.emit_jump(OpCode::Jump);

        self.patch_jump(then_jump);
        self.emit_op(OpCode::Pop);

        if self.match_tok(TokenType::Else) {
            self.statement();
        }
        self.patch_jump(else_jump);
    }

    fn while_statement(&mut self) {
        let loop_start = self.state().function.chunk.code.len();
        self.consume(TokenType::LeftParen, "Expect '(' after 'while'.");
        self.expression();
        self.consume(TokenType::RightParen, "Expect ')' after condition.");

        let exit_jump = self.emit_jump(OpCode::JumpIfFalse);
        self.emit_op(OpCode::Pop);
        self.statement();
        self.emit_loop(loop_start);

        self.patch_jump(exit_jump);
        self.emit_op(OpCode::Pop);
    }

    fn for_statement(&mut self) {
        self.begin_scope();
        self.consume(TokenType::LeftParen, "Expect '(' after 'for'.");

        // Initializer.
        if self.match_tok(TokenType::Semicolon) {
            // No initializer.
        } else if self.match_tok(TokenType::Var) {
            self.var_declaration();
        } else {
            self.expression_statement();
        }

        let mut loop_start = self.state().function.chunk.code.len();

        // Condition (optional).
        let mut exit_jump: Option<usize> = None;
        if !self.match_tok(TokenType::Semicolon) {
            self.expression();
            self.consume(TokenType::Semicolon, "Expect ';' after loop condition.");
            exit_jump = Some(self.emit_jump(OpCode::JumpIfFalse));
            self.emit_op(OpCode::Pop);
        }

        // Increment (optional).
        if !self.match_tok(TokenType::RightParen) {
            let body_jump = self.emit_jump(OpCode::Jump);
            let increment_start = self.state().function.chunk.code.len();
            self.expression();
            self.emit_op(OpCode::Pop);
            self.consume(TokenType::RightParen, "Expect ')' after for clauses.");

            self.emit_loop(loop_start);
            loop_start = increment_start;
            self.patch_jump(body_jump);
        }

        self.statement();
        self.emit_loop(loop_start);

        if let Some(exit) = exit_jump {
            self.patch_jump(exit);
            self.emit_op(OpCode::Pop);
        }

        self.end_scope();
    }

    fn return_statement(&mut self) {
        if self.state().fn_type == FunctionType::Script {
            self.error("Can't return from top-level code.");
        }
        if self.match_tok(TokenType::Semicolon) {
            self.emit_return();
        } else {
            self.expression();
            self.consume(TokenType::Semicolon, "Expect ';' after return value.");
            self.emit_op(OpCode::Return);
        }
    }

    fn block(&mut self) {
        while !self.check(TokenType::RightBrace) && !self.check(TokenType::Eof) {
            self.declaration();
        }
        self.consume(TokenType::RightBrace, "Expect '}' after block.");
    }

    // ---------- var decl ----------

    fn var_declaration(&mut self) {
        let global = self.parse_variable("Expect variable name.");

        if self.match_tok(TokenType::Equal) {
            self.expression();
        } else {
            self.emit_op(OpCode::Nil);
        }
        self.consume(
            TokenType::Semicolon,
            "Expect ';' after variable declaration.",
        );

        self.define_variable(global);
    }

    fn parse_variable(&mut self, msg: &str) -> u8 {
        self.consume(TokenType::Identifier, msg);
        self.declare_variable();
        if self.state().scope_depth > 0 {
            return 0;
        }
        let name = self.parser.previous.lexeme.clone();
        self.identifier_constant(&name)
    }

    fn identifier_constant(&mut self, name: &str) -> u8 {
        self.make_constant(Value::Str(Rc::new(name.to_string())))
    }

    fn declare_variable(&mut self) {
        if self.state().scope_depth == 0 {
            return;
        }
        let name = self.parser.previous.clone();
        let depth = self.state().scope_depth;

        // Check for a duplicate declaration in the same scope.
        let mut duplicate = false;
        for local in self.state().locals.iter().rev() {
            if local.depth != -1 && local.depth < depth {
                break;
            }
            if local.name.lexeme == name.lexeme {
                duplicate = true;
                break;
            }
        }
        if duplicate {
            self.error("Already a variable with this name in this scope.");
            return;
        }
        self.add_local(name);
    }

    fn add_local(&mut self, name: Token) {
        if self.state().locals.len() >= u8::MAX as usize + 1 {
            self.error("Too many local variables in function.");
            return;
        }
        self.state_mut().locals.push(Local {
            name,
            depth: -1,
            is_captured: false,
        });
    }

    fn define_variable(&mut self, global: u8) {
        if self.state().scope_depth > 0 {
            self.mark_initialized();
            return;
        }
        self.emit_op_byte(OpCode::DefineGlobal, global);
    }

    fn mark_initialized(&mut self) {
        if self.state().scope_depth == 0 {
            return;
        }
        let depth = self.state().scope_depth;
        let locals = &mut self.state_mut().locals;
        if let Some(last) = locals.last_mut() {
            last.depth = depth;
        }
    }

    // ---------- function decl ----------

    fn fun_declaration(&mut self) {
        let global = self.parse_variable("Expect function name.");
        self.mark_initialized();
        self.function_body(FunctionType::Function);
        self.define_variable(global);
    }

    fn function_body(&mut self, fn_type: FunctionType) {
        let name = self.parser.previous.lexeme.clone();
        let state = CompilerState::new(fn_type, Some(Rc::new(name)));
        self.states.push(state);
        self.begin_scope();

        self.consume(TokenType::LeftParen, "Expect '(' after function name.");
        if !self.check(TokenType::RightParen) {
            loop {
                if self.state().function.arity >= 255 {
                    let line = self.parser.current.line;
                    self.error_at_current("Can't have more than 255 parameters.", line);
                }
                self.state_mut().function.arity += 1;
                let constant = self.parse_variable("Expect parameter name.");
                self.define_variable(constant);
                if !self.match_tok(TokenType::Comma) {
                    break;
                }
            }
        }
        self.consume(TokenType::RightParen, "Expect ')' after parameters.");
        self.consume(TokenType::LeftBrace, "Expect '{' before function body.");
        self.block();

        // Close out the function. The book emits OP_NIL + OP_RETURN, and we
        // leave the scope implicitly (closing the frame below).
        self.emit_return();

        let compiled = self.states.pop().expect("function frame present");
        let upvalues = compiled.upvalues.clone();
        let mut function = compiled.function;
        function.upvalue_count = upvalues.len();

        let func_rc = Rc::new(function);
        let const_idx = self.make_constant(Value::Function(func_rc));
        self.emit_op_byte(OpCode::Closure, const_idx);
        for up in &upvalues {
            self.emit_byte(if up.is_local { 1 } else { 0 });
            self.emit_byte(up.index);
        }
    }

    // ---------- scope ----------

    fn begin_scope(&mut self) {
        self.state_mut().scope_depth += 1;
    }

    fn end_scope(&mut self) {
        self.state_mut().scope_depth -= 1;
        loop {
            let (should_pop, captured) = {
                let state = self.state_mut();
                match state.locals.last() {
                    Some(local) if local.depth > state.scope_depth => (true, local.is_captured),
                    _ => (false, false),
                }
            };
            if !should_pop {
                break;
            }
            self.state_mut().locals.pop();
            if captured {
                // Reserved for M6 (OP_CLOSE_UPVALUE semantics).
                self.emit_op(OpCode::CloseUpvalue);
            } else {
                self.emit_op(OpCode::Pop);
            }
        }
    }

    // ---------- expression Pratt driver ----------

    fn expression(&mut self) {
        self.parse_precedence(Precedence::Assignment);
    }

    fn parse_precedence(&mut self, precedence: Precedence) {
        self.advance();
        let prefix_kind = self.parser.previous.ttype;
        let prefix_rule = get_rule(prefix_kind).prefix;
        let Some(prefix_fn) = prefix_rule else {
            self.error("Expect expression.");
            return;
        };

        let can_assign = precedence <= Precedence::Assignment;
        prefix_fn(self, can_assign);

        while precedence <= get_rule(self.parser.current.ttype).precedence {
            self.advance();
            let infix_fn = get_rule(self.parser.previous.ttype)
                .infix
                .expect("infix rule present by precedence table");
            infix_fn(self, can_assign);
        }

        if can_assign && self.match_tok(TokenType::Equal) {
            self.error("Invalid assignment target.");
        }
    }

    // ---------- prefix rules ----------

    fn grouping(&mut self, _can_assign: bool) {
        self.expression();
        self.consume(TokenType::RightParen, "Expect ')' after expression.");
    }

    fn number(&mut self, _can_assign: bool) {
        let lex = &self.parser.previous.lexeme;
        let n: f64 = lex.parse().expect("scanner produced a valid number");
        self.emit_constant(Value::Number(n));
    }

    fn string(&mut self, _can_assign: bool) {
        let lex = &self.parser.previous.lexeme;
        // Strip surrounding quotes.
        let trimmed = &lex[1..lex.len() - 1];
        let s = Rc::new(trimmed.to_string());
        self.emit_constant(Value::Str(s));
    }

    fn literal(&mut self, _can_assign: bool) {
        match self.parser.previous.ttype {
            TokenType::False => self.emit_op(OpCode::False),
            TokenType::True => self.emit_op(OpCode::True),
            TokenType::Nil => self.emit_op(OpCode::Nil),
            _ => unreachable!("literal called on non-literal token"),
        }
    }

    fn unary(&mut self, _can_assign: bool) {
        let op_kind = self.parser.previous.ttype;
        self.parse_precedence(Precedence::Unary);
        match op_kind {
            TokenType::Minus => self.emit_op(OpCode::Negate),
            TokenType::Bang => self.emit_op(OpCode::Not),
            _ => unreachable!("unary called on non-unary op"),
        }
    }

    fn variable(&mut self, can_assign: bool) {
        let name = self.parser.previous.clone();
        self.named_variable(&name, can_assign);
    }

    fn named_variable(&mut self, name: &Token, can_assign: bool) {
        let (get_op, set_op, arg) = if let Some(slot) = self.resolve_local(name) {
            (OpCode::GetLocal, OpCode::SetLocal, slot)
        } else if let Some(up) = self.resolve_upvalue(self.states.len() - 1, name) {
            (OpCode::GetUpvalue, OpCode::SetUpvalue, up)
        } else {
            let idx = self.identifier_constant(&name.lexeme);
            (OpCode::GetGlobal, OpCode::SetGlobal, idx)
        };

        if can_assign && self.match_tok(TokenType::Equal) {
            self.expression();
            self.emit_op_byte(set_op, arg);
        } else {
            self.emit_op_byte(get_op, arg);
        }
    }

    fn resolve_local(&mut self, name: &Token) -> Option<u8> {
        // Snapshot the decision before touching `self.error` (borrow-checker
        // dance — we can't hold `&self.state()` across the error call).
        enum Outcome {
            Found(u8),
            Uninitialised,
            NotFound,
        }
        let outcome = {
            let state = self.state();
            let mut out = Outcome::NotFound;
            for (i, local) in state.locals.iter().enumerate().rev() {
                if local.name.lexeme == name.lexeme {
                    if local.depth == -1 {
                        out = Outcome::Uninitialised;
                    } else {
                        out = Outcome::Found(i as u8);
                    }
                    break;
                }
            }
            out
        };
        match outcome {
            Outcome::Found(slot) => Some(slot),
            Outcome::Uninitialised => {
                self.error("Can't read local variable in its own initializer.");
                None
            }
            Outcome::NotFound => None,
        }
    }

    /// Resolve an upvalue for the compiler state at `state_idx`. Returns the
    /// upvalue slot in that state, adding intermediate upvalues recursively
    /// in enclosing frames.
    fn resolve_upvalue(&mut self, state_idx: usize, name: &Token) -> Option<u8> {
        if state_idx == 0 {
            return None;
        }
        let enclosing_idx = state_idx - 1;

        // First: is `name` a local in the immediately enclosing frame?
        let local_slot = {
            let enc = &self.states[enclosing_idx];
            let mut found = None;
            for (i, local) in enc.locals.iter().enumerate().rev() {
                if local.name.lexeme == name.lexeme {
                    found = Some(i as u8);
                    break;
                }
            }
            found
        };

        if let Some(slot) = local_slot {
            // Mark the local as captured so scope-exit doesn't OP_POP it out
            // from under the closure. (Cosmetic in M5 — OP_POP/OP_CLOSE_UPVALUE
            // behave identically given our Rc-cell runtime representation.)
            self.states[enclosing_idx].locals[slot as usize].is_captured = true;
            return Some(self.add_upvalue(state_idx, slot, true));
        }

        // Second: does an enclosing frame's enclosing frame have it?
        let recursed = self.resolve_upvalue(enclosing_idx, name);
        if let Some(up) = recursed {
            return Some(self.add_upvalue(state_idx, up, false));
        }

        None
    }

    fn add_upvalue(&mut self, state_idx: usize, index: u8, is_local: bool) -> u8 {
        // Check for an existing match first.
        {
            let state = &self.states[state_idx];
            for (i, uv) in state.upvalues.iter().enumerate() {
                if uv.index == index && uv.is_local == is_local {
                    return i as u8;
                }
            }
        }
        if self.states[state_idx].upvalues.len() == u8::MAX as usize + 1 {
            self.error("Too many closure variables in function.");
            return 0;
        }
        let state = &mut self.states[state_idx];
        state.upvalues.push(Upvalue { index, is_local });
        (state.upvalues.len() - 1) as u8
    }

    // ---------- infix rules ----------

    fn binary(&mut self, _can_assign: bool) {
        let op_kind = self.parser.previous.ttype;
        let rule = get_rule(op_kind);
        self.parse_precedence(rule.precedence.next());

        match op_kind {
            TokenType::BangEqual => {
                self.emit_op(OpCode::Equal);
                self.emit_op(OpCode::Not);
            }
            TokenType::EqualEqual => self.emit_op(OpCode::Equal),
            TokenType::Greater => self.emit_op(OpCode::Greater),
            TokenType::GreaterEqual => {
                self.emit_op(OpCode::Less);
                self.emit_op(OpCode::Not);
            }
            TokenType::Less => self.emit_op(OpCode::Less),
            TokenType::LessEqual => {
                self.emit_op(OpCode::Greater);
                self.emit_op(OpCode::Not);
            }
            TokenType::Plus => self.emit_op(OpCode::Add),
            TokenType::Minus => self.emit_op(OpCode::Subtract),
            TokenType::Star => self.emit_op(OpCode::Multiply),
            TokenType::Slash => self.emit_op(OpCode::Divide),
            _ => unreachable!("binary called on non-binary op"),
        }
    }

    fn call(&mut self, _can_assign: bool) {
        let arg_count = self.argument_list();
        self.emit_op_byte(OpCode::Call, arg_count);
    }

    fn argument_list(&mut self) -> u8 {
        let mut count: u8 = 0;
        if !self.check(TokenType::RightParen) {
            loop {
                self.expression();
                if count == 255 {
                    self.error("Can't have more than 255 arguments.");
                }
                count = count.saturating_add(1);
                if !self.match_tok(TokenType::Comma) {
                    break;
                }
            }
        }
        self.consume(TokenType::RightParen, "Expect ')' after arguments.");
        count
    }

    fn and_(&mut self, _can_assign: bool) {
        // Left operand already on stack. If false, short-circuit.
        let end_jump = self.emit_jump(OpCode::JumpIfFalse);
        self.emit_op(OpCode::Pop);
        self.parse_precedence(Precedence::And);
        self.patch_jump(end_jump);
    }

    fn or_(&mut self, _can_assign: bool) {
        // If left is false, fall through; if true, short-circuit to end.
        let else_jump = self.emit_jump(OpCode::JumpIfFalse);
        let end_jump = self.emit_jump(OpCode::Jump);
        self.patch_jump(else_jump);
        self.emit_op(OpCode::Pop);
        self.parse_precedence(Precedence::Or);
        self.patch_jump(end_jump);
    }

    // ---------- synchronize ----------

    fn synchronize(&mut self) {
        self.parser.panic_mode = false;
        while self.parser.current.ttype != TokenType::Eof {
            if self.parser.previous.ttype == TokenType::Semicolon {
                return;
            }
            match self.parser.current.ttype {
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

// ---------- Pratt rule table ----------

type PrefixFn = fn(&mut Compiler<'_>, bool);
type InfixFn = fn(&mut Compiler<'_>, bool);

struct ParseRule {
    prefix: Option<PrefixFn>,
    infix: Option<InfixFn>,
    precedence: Precedence,
}

fn get_rule(ttype: TokenType) -> ParseRule {
    use TokenType::*;
    match ttype {
        LeftParen => ParseRule {
            prefix: Some(|c, ca| c.grouping(ca)),
            infix: Some(|c, ca| c.call(ca)),
            precedence: Precedence::Call,
        },
        RightParen => none_rule(),
        LeftBrace => none_rule(),
        RightBrace => none_rule(),
        Comma => none_rule(),
        Dot => ParseRule {
            prefix: None,
            infix: None, // Class/method access lands in M6.
            precedence: Precedence::Call,
        },
        Minus => ParseRule {
            prefix: Some(|c, ca| c.unary(ca)),
            infix: Some(|c, ca| c.binary(ca)),
            precedence: Precedence::Term,
        },
        Plus => ParseRule {
            prefix: None,
            infix: Some(|c, ca| c.binary(ca)),
            precedence: Precedence::Term,
        },
        Semicolon => none_rule(),
        Slash => ParseRule {
            prefix: None,
            infix: Some(|c, ca| c.binary(ca)),
            precedence: Precedence::Factor,
        },
        Star => ParseRule {
            prefix: None,
            infix: Some(|c, ca| c.binary(ca)),
            precedence: Precedence::Factor,
        },
        Bang => ParseRule {
            prefix: Some(|c, ca| c.unary(ca)),
            infix: None,
            precedence: Precedence::None,
        },
        BangEqual => ParseRule {
            prefix: None,
            infix: Some(|c, ca| c.binary(ca)),
            precedence: Precedence::Equality,
        },
        Equal => none_rule(),
        EqualEqual => ParseRule {
            prefix: None,
            infix: Some(|c, ca| c.binary(ca)),
            precedence: Precedence::Equality,
        },
        Greater => ParseRule {
            prefix: None,
            infix: Some(|c, ca| c.binary(ca)),
            precedence: Precedence::Comparison,
        },
        GreaterEqual => ParseRule {
            prefix: None,
            infix: Some(|c, ca| c.binary(ca)),
            precedence: Precedence::Comparison,
        },
        Less => ParseRule {
            prefix: None,
            infix: Some(|c, ca| c.binary(ca)),
            precedence: Precedence::Comparison,
        },
        LessEqual => ParseRule {
            prefix: None,
            infix: Some(|c, ca| c.binary(ca)),
            precedence: Precedence::Comparison,
        },
        Identifier => ParseRule {
            prefix: Some(|c, ca| c.variable(ca)),
            infix: None,
            precedence: Precedence::None,
        },
        String => ParseRule {
            prefix: Some(|c, ca| c.string(ca)),
            infix: None,
            precedence: Precedence::None,
        },
        Number => ParseRule {
            prefix: Some(|c, ca| c.number(ca)),
            infix: None,
            precedence: Precedence::None,
        },
        And => ParseRule {
            prefix: None,
            infix: Some(|c, ca| c.and_(ca)),
            precedence: Precedence::And,
        },
        Class => none_rule(),
        Else => none_rule(),
        False => ParseRule {
            prefix: Some(|c, ca| c.literal(ca)),
            infix: None,
            precedence: Precedence::None,
        },
        Fun => none_rule(),
        For => none_rule(),
        If => none_rule(),
        Nil => ParseRule {
            prefix: Some(|c, ca| c.literal(ca)),
            infix: None,
            precedence: Precedence::None,
        },
        Or => ParseRule {
            prefix: None,
            infix: Some(|c, ca| c.or_(ca)),
            precedence: Precedence::Or,
        },
        Print => none_rule(),
        Return => none_rule(),
        Super => none_rule(),
        This => none_rule(),
        True => ParseRule {
            prefix: Some(|c, ca| c.literal(ca)),
            infix: None,
            precedence: Precedence::None,
        },
        Var => none_rule(),
        While => none_rule(),
        Error => none_rule(),
        Eof => none_rule(),
    }
}

fn none_rule() -> ParseRule {
    ParseRule {
        prefix: None,
        infix: None,
        precedence: Precedence::None,
    }
}

#[cfg(test)]
mod compile_tests {
    use super::*;
    use crate::chunk::OpCode;

    fn ops_of(func: &ObjFunction) -> Vec<OpCode> {
        let mut out = Vec::new();
        let mut i = 0;
        while i < func.chunk.code.len() {
            let byte = func.chunk.code[i];
            if let Some(op) = OpCode::from_byte(byte) {
                out.push(op);
                i += operand_width(op, &func.chunk, i) + 1;
            } else {
                i += 1;
            }
        }
        out
    }

    fn operand_width(op: OpCode, chunk: &Chunk, offset: usize) -> usize {
        use OpCode::*;
        match op {
            Nil | True | False | Pop | Equal | Greater | Less | Add | Subtract | Multiply
            | Divide | Not | Negate | Print | CloseUpvalue | Return | Inherit => 0,
            Constant | GetLocal | SetLocal | GetGlobal | DefineGlobal | SetGlobal | GetUpvalue
            | SetUpvalue | GetProperty | SetProperty | GetSuper | Class | Method | Call => 1,
            Jump | JumpIfFalse | Loop => 2,
            Invoke | SuperInvoke => 2,
            Closure => {
                // 1 byte const + per-upvalue pairs read from fn's upvalue_count.
                let const_idx = chunk.code[offset + 1] as usize;
                let upvalue_count = match chunk.constants.get(const_idx) {
                    Some(Value::Function(f)) => f.upvalue_count,
                    _ => 0,
                };
                1 + upvalue_count * 2
            }
        }
    }

    #[test]
    fn compile_constant_expression() {
        let func = compile("1.5;").expect("compiles");
        let ops = ops_of(&func);
        assert_eq!(
            ops,
            vec![OpCode::Constant, OpCode::Pop, OpCode::Nil, OpCode::Return]
        );
    }

    #[test]
    fn compile_arithmetic_precedence() {
        // 1 + 2 * 3 -> 1, 2, 3, Multiply, Add, Print
        let func = compile("print 1 + 2 * 3;").expect("compiles");
        let ops = ops_of(&func);
        assert_eq!(
            ops,
            vec![
                OpCode::Constant,
                OpCode::Constant,
                OpCode::Constant,
                OpCode::Multiply,
                OpCode::Add,
                OpCode::Print,
                OpCode::Nil,
                OpCode::Return,
            ]
        );
    }

    #[test]
    fn compile_negation_unary() {
        let func = compile("print -1;").expect("compiles");
        let ops = ops_of(&func);
        assert_eq!(
            ops,
            vec![
                OpCode::Constant,
                OpCode::Negate,
                OpCode::Print,
                OpCode::Nil,
                OpCode::Return,
            ]
        );
    }

    #[test]
    fn compile_global_variable_declaration() {
        let func = compile("var x = 1;").expect("compiles");
        let ops = ops_of(&func);
        assert!(ops.contains(&OpCode::DefineGlobal));
        assert!(ops.contains(&OpCode::Constant));
    }

    #[test]
    fn compile_global_variable_read() {
        let func = compile("var x = 1; print x;").expect("compiles");
        let ops = ops_of(&func);
        assert!(ops.contains(&OpCode::GetGlobal));
    }

    #[test]
    fn compile_local_scope_resolves_to_slot() {
        let func = compile("{ var a = 1; print a; }").expect("compiles");
        let ops = ops_of(&func);
        assert!(
            ops.contains(&OpCode::GetLocal),
            "expected GetLocal in {ops:?}"
        );
    }

    #[test]
    fn compile_if_else_has_two_jumps() {
        let func = compile("if (true) print 1; else print 2;").expect("compiles");
        let ops = ops_of(&func);
        let jumps = ops
            .iter()
            .filter(|o| matches!(o, OpCode::Jump | OpCode::JumpIfFalse))
            .count();
        assert_eq!(jumps, 2, "expected Jump + JumpIfFalse, got {ops:?}");
    }

    #[test]
    fn compile_while_emits_loop_back() {
        let func = compile("while (true) print 1;").expect("compiles");
        let ops = ops_of(&func);
        assert!(ops.contains(&OpCode::Loop));
        assert!(ops.contains(&OpCode::JumpIfFalse));
    }

    #[test]
    fn compile_for_desugared_emits_while_structure() {
        let func = compile("for (var i = 0; i < 3; i = i + 1) print i;").expect("compiles");
        let ops = ops_of(&func);
        assert!(ops.contains(&OpCode::Loop));
        assert!(ops.contains(&OpCode::JumpIfFalse));
    }

    #[test]
    fn compile_logical_or_short_circuits() {
        let func = compile("print true or false;").expect("compiles");
        let ops = ops_of(&func);
        let jumps = ops
            .iter()
            .filter(|o| matches!(o, OpCode::Jump | OpCode::JumpIfFalse))
            .count();
        assert_eq!(jumps, 2, "or desugars to two jumps, got {ops:?}");
    }

    #[test]
    fn compile_logical_and_short_circuits() {
        let func = compile("print true and false;").expect("compiles");
        let ops = ops_of(&func);
        assert!(ops.contains(&OpCode::JumpIfFalse));
    }

    #[test]
    fn compile_function_declaration_emits_closure_op() {
        let src = "fun f() { return 1; } f();";
        let func = compile(src).expect("compiles");
        let ops = ops_of(&func);
        assert!(
            ops.contains(&OpCode::Closure),
            "expected Closure in {ops:?}"
        );
        assert!(ops.contains(&OpCode::Call), "expected Call in {ops:?}");
    }

    #[test]
    fn compile_parse_error_accumulates_messages_and_synchronizes() {
        // First `1 1 +` is nonsense; the synchronizer should still let us
        // see the good second statement. But here we care that `compile`
        // returns errors rather than panicking.
        let errs = compile("var = 1;").expect_err("fails");
        assert!(
            errs.iter().any(|m| m.contains("variable name")),
            "got {errs:?}"
        );
    }

    #[test]
    fn compile_return_at_top_level_errors() {
        let errs = compile("return 1;").expect_err("fails");
        assert!(
            errs.iter().any(|m| m.contains("top-level")),
            "got {errs:?}"
        );
    }

    #[test]
    fn compile_string_literal_stored_as_str_constant() {
        let func = compile("print \"hi\";").expect("compiles");
        let has_str = func
            .chunk
            .constants
            .iter()
            .any(|v| matches!(v, Value::Str(s) if &**s == "hi"));
        assert!(
            has_str,
            "expected 'hi' string constant in {:?}",
            func.chunk.constants
        );
    }
}
