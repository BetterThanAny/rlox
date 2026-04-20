# rlox — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: superpowers:subagent-driven-development (recommended) or superpowers:executing-plans. Tasks use `- [ ]` checkboxes for progress tracking.

**Goal:** Ship two Rust implementations of the Lox language from *Crafting Interpreters* — a tree-walking interpreter (`rlox-tree`) and a bytecode VM (`rlox-vm`) — plus a shared test runner that drives the official Lox test suite against both with ≥95% pass rate.

**Architecture:** Single Cargo workspace with three crate members. Each language implementation mirrors the book's chapter structure (scanner → parser → resolver → interpreter for tree; scanner → compiler → chunk → vm → gc for vm). Test-suite crate shells out to compiled binaries and diffs stdout against `// expect:` directives in vendored `.lox` scripts.

**Tech Stack:** Rust edition 2021; `thiserror` + `anyhow` for errors; `rustyline` for REPL; stdlib only for core logic. No async, no macros, no serde.

**Spec:** `docs/specs/2026-04-20-lox-rust-design.md`.

---

## Milestone Table

| # | Title | Acceptance (one command) | Parallel? |
|---|---|---|---|
| M1 | Scanner + Token + AST + Error (tree) | `cargo test -p rlox-tree --lib` ≥ 20 tests pass | Wave 1 |
| M2 | Parser + Resolver (tree) | `cargo test -p rlox-tree` ≥ 50 tests pass | Wave 2 |
| M3 | Interpreter + Env + Value + REPL (tree) | `cargo run -p rlox-tree -- examples/fib.lox` prints `55`; `examples/class.lox` succeeds | Serial |
| M4 | Chunk + OpCodes + Value + Disassembler (vm) | `cargo test -p rlox-vm chunk_ debug_` ≥ 10 tests pass | Wave 3 |
| M5 | Scanner + Pratt Compiler + VM core (vm) | `cargo run -p rlox-vm -- examples/fib.lox` prints `55` | Serial |
| M6 | Heap objects + GC + classes (vm) | `cargo test -p rlox-vm --features gc_stress` passes | Wave 4 + serial |
| M7 | `test-suite` crate, vendor + runner | `cargo run -p test-suite -- --target both` reports ≥95% pass per target | Serial |
| M8 | fmt + clippy + reviewer subagent gate | `cargo fmt --check && cargo clippy --workspace --all-targets -- -D warnings` clean + reviewer no blockers | Serial |

## Parallelization Schedule

```
[S0 scaffold]
     │
     ▼
[Wave 1] T1 scanner │ T2 ast │ T3 error  (parallel)
     │
     ▼
[S1 M1 integration + cargo test]
     │
     ▼
[Wave 2] T4 parser │ T5 resolver │ T6 environment skeleton  (parallel)
     │
     ▼
[S2 M2 integration]
     │
     ▼
[S3 M3 interpreter + value + native fns + REPL]  (serial, single agent)
     │
     ▼
[Code review subagent pass on rlox-tree]
     │
     ▼
[S4 rlox-vm scaffold]
     │
     ▼
[Wave 3] V1 chunk+debug │ V2 value+scanner-vm  (parallel)
     │
     ▼
[S5 M5 compiler (single agent)]
     │
     ▼
[S6 M5 vm core (single agent)]
     │
     ▼
[Wave 4] V3 object+string interning │ V4 gc  (parallel)
     │
     ▼
[S7 M6 classes + inheritance in compiler/vm (serial)]
     │
     ▼
[Code review subagent pass on rlox-vm]
     │
     ▼
[S8 M7 test-suite: vendor + runner (serial)]
     │
     ▼
[S9 M8 quality gate (serial)]
     │
     ▼
[DONE]
```

## Test Matrix

| Layer | Kind | Where | Run with |
|---|---|---|---|
| Scanner | unit | `rlox-tree/src/scanner.rs` mod tests | `cargo test -p rlox-tree scanner_` |
| Parser | unit | `rlox-tree/tests/parser_tests.rs` | `cargo test -p rlox-tree parser_` |
| Resolver | unit | `rlox-tree/tests/resolver_tests.rs` | `cargo test -p rlox-tree resolver_` |
| Tree interpreter | integration | `rlox-tree/tests/interpret_tests.rs` | `cargo test -p rlox-tree interpret_` |
| VM chunk/debug | unit | `rlox-vm/tests/chunk_tests.rs` | `cargo test -p rlox-vm chunk_` |
| VM compile | unit | `rlox-vm/tests/compile_tests.rs` | `cargo test -p rlox-vm compile_` |
| VM exec | integration | `rlox-vm/tests/vm_tests.rs` | `cargo test -p rlox-vm vm_` |
| GC stress | integration | `rlox-vm/tests/gc_tests.rs` (feature-gated) | `cargo test -p rlox-vm --features gc_stress gc_` |
| E2E Lox scripts | acceptance | `test-suite/cases/` (vendored) | `cargo run -p test-suite -- --target both` |
| Lint | static | `cargo clippy` | `cargo clippy --workspace --all-targets -- -D warnings` |
| Format | static | `cargo fmt` | `cargo fmt --check` |

---

## Milestone 0 — Workspace scaffold (serial, runs once)

**Files:** `Cargo.toml`, `rlox-tree/Cargo.toml`, `rlox-tree/src/lib.rs`, `rlox-tree/src/main.rs`, `rlox-vm/Cargo.toml`, `rlox-vm/src/lib.rs`, `rlox-vm/src/main.rs`, `test-suite/Cargo.toml`, `test-suite/src/main.rs`

- [ ] **Step 1: Create root Cargo.toml**

```toml
[workspace]
resolver = "2"
members = ["rlox-tree", "rlox-vm", "test-suite"]

[workspace.package]
edition = "2021"
rust-version = "1.75"

[workspace.dependencies]
thiserror = "1"
anyhow = "1"
rustyline = "14"
```

- [ ] **Step 2: Create `rlox-tree/Cargo.toml`**

```toml
[package]
name = "rlox-tree"
version = "0.1.0"
edition.workspace = true

[lib]
name = "rlox_tree"
path = "src/lib.rs"

[[bin]]
name = "rlox-tree"
path = "src/main.rs"

[dependencies]
thiserror.workspace = true
anyhow.workspace = true
rustyline.workspace = true
```

- [ ] **Step 3: Create `rlox-vm/Cargo.toml`** (same shape, name `rlox-vm`, feature `gc_stress = []`)

```toml
[package]
name = "rlox-vm"
version = "0.1.0"
edition.workspace = true

[lib]
name = "rlox_vm"
path = "src/lib.rs"

[[bin]]
name = "rlox-vm"
path = "src/main.rs"

[features]
default = []
gc_stress = []

[dependencies]
thiserror.workspace = true
anyhow.workspace = true
rustyline.workspace = true
```

- [ ] **Step 4: Create `test-suite/Cargo.toml`**

```toml
[package]
name = "rlox-test-suite"
version = "0.1.0"
edition.workspace = true

[[bin]]
name = "test-suite"
path = "src/main.rs"

[dependencies]
anyhow.workspace = true
```

- [ ] **Step 5: Stub sources**

`rlox-tree/src/lib.rs`:
```rust
//! Tree-walking Lox interpreter.
```
`rlox-tree/src/main.rs`:
```rust
fn main() {
    println!("rlox-tree stub");
}
```
Mirror for `rlox-vm` and `test-suite`.

- [ ] **Step 6: Verify workspace builds**

Run: `cargo check --workspace`
Expected: no errors, all three crates resolve.

- [ ] **Step 7: Commit**

```bash
git add -A
git commit -m "chore(scaffold): init rlox Cargo workspace (tree/vm/test-suite)"
```

---

## Milestone 1 — Scanner + Token + AST + Error (rlox-tree)

**Deliverables:** full lexer for Lox, Token types, AST node enums (Expr + Stmt), shared error type.
**Files:**
- Create: `rlox-tree/src/token.rs`, `rlox-tree/src/scanner.rs`, `rlox-tree/src/ast.rs`, `rlox-tree/src/error.rs`, `rlox-tree/src/lib.rs` (wire modules)

### Task M1.1 — `token.rs`: Token + TokenType  [parallel: agent T-scan]

- [ ] **Write `rlox-tree/src/token.rs`** with:
  - `pub enum TokenType` — 40 variants covering Lox (single-char: LeftParen, RightParen, LeftBrace, RightBrace, Comma, Dot, Minus, Plus, Semicolon, Slash, Star; one/two-char: Bang, BangEqual, Equal, EqualEqual, Greater, GreaterEqual, Less, LessEqual; literals: Identifier, String, Number; keywords: And, Class, Else, False, Fun, For, If, Nil, Or, Print, Return, Super, This, True, Var, While; terminal: Eof).
  - `pub enum Literal { Str(String), Num(f64), Bool(bool), Nil }` (used by scanner + value module).
  - `pub struct Token { pub ttype: TokenType, pub lexeme: String, pub literal: Option<Literal>, pub line: usize }`.
  - `#[cfg(test)] mod token_tests` with at least: `token_display_format`, `token_type_equality`, `literal_number_ordering`.

- [ ] **Run:** `cargo test -p rlox-tree token_` → 3 tests pass.
- [ ] **Commit:** `feat(tree): define Token, TokenType, Literal`.

### Task M1.2 — `scanner.rs`: lexer  [parallel: agent T-scan, same file-owner]

Follow book Chapter 4 semantics exactly.

- [ ] **Write failing test** `rlox-tree/src/scanner.rs` (inline `#[cfg(test)]`):

```rust
#[test]
fn scanner_single_char_tokens() {
    let src = "(){},.-+;*/";
    let tokens = Scanner::new(src).scan_tokens().unwrap();
    let types: Vec<_> = tokens.iter().map(|t| t.ttype.clone()).collect();
    use TokenType::*;
    assert_eq!(
        types,
        vec![LeftParen, RightParen, LeftBrace, RightBrace,
             Comma, Dot, Minus, Plus, Semicolon, Star, Slash, Eof]
    );
}
```

- [ ] **Implement `Scanner`**:
  - `pub struct Scanner<'a> { source: &'a str, chars: std::str::CharIndices<'a>, tokens: Vec<Token>, start: usize, current: usize, line: usize }`
  - `pub fn new(source: &'a str) -> Self`
  - `pub fn scan_tokens(mut self) -> Result<Vec<Token>, LoxError>`
  - Internal: `scan_token`, `advance`, `peek`, `peek_next`, `match_char`, `is_at_end`, `add_token(TokenType)`, `add_token_with_literal`
  - Handle: single-char, two-char (`!=`, `==`, `<=`, `>=`), `/` vs `//` line comment, whitespace, strings (multi-line allowed), numbers (incl. decimals), identifiers+keywords map, unterminated string error, unexpected char error.

- [ ] **Add tests** (inline): `scanner_operators_two_char`, `scanner_line_comment_ignored`, `scanner_string_literal`, `scanner_unterminated_string_errors`, `scanner_number_int_and_float`, `scanner_identifier_and_keyword`, `scanner_multi_line_tracks_line`, `scanner_rejects_stray_char_with_line`.
- [ ] **Run:** `cargo test -p rlox-tree scanner_` → ≥ 9 tests pass.
- [ ] **Commit:** `feat(tree): implement Lox scanner with literals, keywords, and errors`.

### Task M1.3 — `error.rs`: shared error type  [parallel: agent T-err]

- [ ] **Write `rlox-tree/src/error.rs`**:

```rust
use thiserror::Error;

#[derive(Debug, Error, Clone, PartialEq)]
pub enum LoxError {
    #[error("[line {line}] Error{loc}: {msg}")]
    Syntax { line: usize, loc: String, msg: String },
    #[error("[line {line}] RuntimeError: {msg}")]
    Runtime { line: usize, msg: String },
    #[error("[line {line}] ResolveError: {msg}")]
    Resolve { line: usize, msg: String },
}

impl LoxError {
    pub fn syntax(line: usize, loc: impl Into<String>, msg: impl Into<String>) -> Self {
        Self::Syntax { line, loc: loc.into(), msg: msg.into() }
    }
    pub fn runtime(line: usize, msg: impl Into<String>) -> Self {
        Self::Runtime { line, msg: msg.into() }
    }
    pub fn resolve(line: usize, msg: impl Into<String>) -> Self {
        Self::Resolve { line, msg: msg.into() }
    }
}
```

- [ ] **Inline tests** verify Display format matches book (`[line 3] Error at '(': foo`).
- [ ] **Run:** `cargo test -p rlox-tree error_` → tests pass.
- [ ] **Commit:** `feat(tree): add LoxError with book-matching display format`.

### Task M1.4 — `ast.rs`: Expr and Stmt enums  [parallel: agent T-ast]

- [ ] **Write `rlox-tree/src/ast.rs`**:

```rust
use crate::token::{Literal, Token};

#[derive(Debug, Clone)]
pub enum Expr {
    Assign { name: Token, value: Box<Expr> },
    Binary { left: Box<Expr>, op: Token, right: Box<Expr> },
    Call { callee: Box<Expr>, paren: Token, args: Vec<Expr> },
    Get { object: Box<Expr>, name: Token },
    Grouping(Box<Expr>),
    Literal(Literal),
    Logical { left: Box<Expr>, op: Token, right: Box<Expr> },
    Set { object: Box<Expr>, name: Token, value: Box<Expr> },
    Super { keyword: Token, method: Token },
    This(Token),
    Unary { op: Token, right: Box<Expr> },
    Variable(Token),
}

#[derive(Debug, Clone)]
pub enum Stmt {
    Block(Vec<Stmt>),
    Class { name: Token, superclass: Option<Expr>, methods: Vec<Stmt> },
    Expression(Expr),
    Function { name: Token, params: Vec<Token>, body: Vec<Stmt> },
    If { cond: Expr, then_branch: Box<Stmt>, else_branch: Option<Box<Stmt>> },
    Print(Expr),
    Return { keyword: Token, value: Option<Expr> },
    Var { name: Token, initializer: Option<Expr> },
    While { cond: Expr, body: Box<Stmt> },
}
```

- [ ] **Inline tests:** `ast_expr_clone`, `ast_stmt_block_holds_children`.
- [ ] **Run:** `cargo test -p rlox-tree ast_` → tests pass.
- [ ] **Commit:** `feat(tree): define AST Expr and Stmt enums`.

### Task M1.5 — wire `lib.rs` + M1 acceptance  [serial: after all M1 agents]

- [ ] **Write `rlox-tree/src/lib.rs`**:

```rust
pub mod ast;
pub mod error;
pub mod scanner;
pub mod token;
```

- [ ] **Run full M1 gate:**
  - `cargo fmt --check -p rlox-tree`
  - `cargo clippy -p rlox-tree --lib -- -D warnings`
  - `cargo test -p rlox-tree --lib` → ≥ 20 tests pass.
- [ ] **Commit:** `chore(tree): wire M1 modules in lib.rs`.

---

## Milestone 2 — Parser + Resolver (rlox-tree)

**Deliverables:** recursive-descent parser producing `Vec<Stmt>`; resolver side-table (`HashMap<usize, usize>` keyed by expression id).
**Files:** `rlox-tree/src/parser.rs`, `rlox-tree/src/resolver.rs`, test files `rlox-tree/tests/parser_tests.rs`, `rlox-tree/tests/resolver_tests.rs`.

### Task M2.1 — Parser expressions (Pratt-ish recursive descent)  [parallel: agent T-parse]

- [ ] **Write failing test** `tests/parser_tests.rs`:

```rust
use rlox_tree::{parser::Parser, scanner::Scanner};

fn parse_expr(src: &str) -> String {
    let tokens = Scanner::new(src).scan_tokens().unwrap();
    let mut p = Parser::new(tokens);
    format!("{:?}", p.parse().unwrap())
}

#[test]
fn parser_precedence_mul_over_add() {
    let ast = parse_expr("1 + 2 * 3;");
    assert!(ast.contains("Binary"));
    assert!(ast.contains("Star"));
}
```

- [ ] **Implement `Parser`** per book Chapter 6 grammar:
  - `new(tokens: Vec<Token>)`, `parse() -> Result<Vec<Stmt>, LoxError>`.
  - Expression chain: `expression → assignment → logical_or → logical_and → equality → comparison → term → factor → unary → call → primary`.
  - Statements: `declaration → var_decl | fun_decl | class_decl | statement`; `statement → print | expression_stmt | block | if | while | for | return`.
  - Error recovery: `synchronize()` after error, consume until statement boundary.
  - Emit `LoxError::Syntax` with line + location ("at 'foo'" / "at end").
- [ ] **Tests:** at minimum 15 cases covering: precedence, associativity, grouping, unary, equality, logical, assignment target validation, var decl, block scope, if/else, while, for (desugared), function decl + call, class decl + method + super + this.
- [ ] **Run:** `cargo test -p rlox-tree parser_` → ≥ 15 tests pass.
- [ ] **Commit:** `feat(tree): recursive-descent parser for full Lox grammar`.

### Task M2.2 — Resolver  [parallel: agent T-res]

- [ ] **Write `rlox-tree/src/resolver.rs`**:
  - `pub struct Resolver { scopes: Vec<HashMap<String, bool>>, locals: HashMap<usize, usize>, current_fn: FunctionType, current_class: ClassType }`
  - `pub fn resolve(stmts: &[Stmt]) -> Result<HashMap<usize, usize>, LoxError>`
  - Assign an `id: usize` to each `Expr::Variable`/`Assign`/`This`/`Super` (use atomic counter or resolve via pointer identity — prefer counter inside parser: parser writes `ExprId` into an auxiliary side-table at construction).
  - **Decision:** extend `Expr` with an implicit `id` field on `Variable | Assign | This | Super` variants; parser mints via `next_id()`.
  - Rules: double-declare error, self-reference in initializer error, return-outside-function error, this-outside-class error, super without parent error, return-with-value in initializer error.
- [ ] **Write tests** `tests/resolver_tests.rs` (≥ 10): each error case above + one happy closure case.
- [ ] **Run:** `cargo test -p rlox-tree resolver_` → ≥ 10 tests pass.
- [ ] **Commit:** `feat(tree): static resolver with scope and error diagnostics`.

### Task M2.3 — ExprId plumbing  [serial, blocks M2.2]

- [ ] Extend `ast.rs` with `id: usize` on variable-like variants and `pub fn expr_id(e: &Expr) -> Option<usize>`.
- [ ] Update parser to mint ids; add test ensuring ids are unique.
- [ ] **Commit:** `feat(tree): stable ExprId for resolver side-table`.

### Task M2.4 — M2 acceptance gate  [serial]

- [ ] `cargo test -p rlox-tree` → ≥ 50 tests pass (M1 + M2 combined).
- [ ] `cargo clippy -p rlox-tree --all-targets -- -D warnings` clean.

---

## Milestone 3 — Interpreter + Environment + Value + REPL (rlox-tree)

**Deliverables:** runtime values, env chain, statement/expression evaluation, classes+closures+inheritance, REPL + file runner binary.
**Files:** `rlox-tree/src/environment.rs`, `rlox-tree/src/value.rs`, `rlox-tree/src/interpreter.rs`, update `main.rs`, `examples/*.lox`.

### Task M3.1 — `environment.rs`  [serial]

- [ ] Implement `pub struct Environment { values: HashMap<String, LoxValue>, enclosing: Option<Rc<RefCell<Environment>>> }` with `new`, `with_enclosing`, `define`, `get(name: &Token)`, `assign(name: &Token, v: LoxValue)`, `get_at(depth, name)`, `assign_at(depth, name, v)`.
- [ ] Tests: define+get, shadow in nested scope, undefined var error, get_at with depth 2.
- [ ] **Commit:** `feat(tree): environment with scope chain and resolver-aware lookups`.

### Task M3.2 — `value.rs`  [serial]

- [ ] `pub enum LoxValue { Nil, Bool(bool), Number(f64), Str(Rc<String>), Callable(Rc<dyn LoxCallable>), Class(Rc<LoxClass>), Instance(Rc<RefCell<LoxInstance>>) }`
- [ ] `pub trait LoxCallable { fn arity(&self) -> usize; fn call(&self, interp: &mut Interpreter, args: Vec<LoxValue>) -> Result<LoxValue, LoxError>; fn name(&self) -> &str; }`
- [ ] Structs: `LoxFunction { decl: Stmt::Function, closure: Rc<RefCell<Environment>>, is_initializer: bool }`, `LoxClass { name: String, superclass: Option<Rc<LoxClass>>, methods: HashMap<String, Rc<LoxFunction>> }`, `LoxInstance { class: Rc<LoxClass>, fields: HashMap<String, LoxValue> }`.
- [ ] Implement `Display` per book: `nil`, `true/false`, number without trailing `.0` when integral, strings unquoted in print.
- [ ] Native `clock` function (returns seconds as f64 since epoch).
- [ ] Tests: display formatting of each variant; instance method binding; class.find_method traverses superclass.
- [ ] **Commit:** `feat(tree): LoxValue + LoxCallable + Function/Class/Instance`.

### Task M3.3 — `interpreter.rs`  [serial]

- [ ] `pub struct Interpreter { globals: Rc<RefCell<Environment>>, env: Rc<RefCell<Environment>>, locals: HashMap<usize, usize> }`
- [ ] Methods: `new()`, `interpret(stmts: &[Stmt]) -> Result<(), LoxError>`, `execute(stmt)`, `evaluate(expr)`, `execute_block(stmts, env)`, `look_up_variable(name, id)`, `resolve(id, depth)`.
- [ ] Wire all Expr/Stmt variants. Matches book Chapter 7–13 semantics.
- [ ] Integration tests `tests/interpret_tests.rs` (≥ 15): arithmetic, string concat, logical short-circuit, print, var scope, closures (counter example), class+method, this, inheritance + super call, return in initializer error, native clock callable.
- [ ] **Commit:** `feat(tree): tree-walking interpreter with classes and closures`.

### Task M3.4 — `main.rs`: REPL + file runner  [serial]

- [ ] Argv handling: `rlox-tree [script]` → file mode; no args → REPL via `rustyline`.
- [ ] File mode: exit 65 on syntax/resolve, 70 on runtime.
- [ ] REPL: each line parsed standalone, errors recover, `.exit` quits.
- [ ] Example scripts `examples/hello.lox`, `examples/fib.lox`, `examples/closure.lox`, `examples/class.lox`. Each file contains expected output in `// expect:` comments.
- [ ] **Acceptance:**
  - `cargo run -p rlox-tree -- examples/fib.lox` prints `55`.
  - `cargo run -p rlox-tree -- examples/class.lox` prints expected lines.
- [ ] **Commit:** `feat(tree): REPL + file runner + smoke examples`.

### Task M3.5 — rlox-tree reviewer pass  [serial]

- [ ] Dispatch `superpowers:code-reviewer` subagent over `rlox-tree/`; fix blockers before proceeding.
- [ ] **Commit fixes as:** `fix(tree): address code review findings`.

---

## Milestone 4 — Chunk + OpCodes + Value + Disassembler (rlox-vm)

**Files:** `rlox-vm/src/chunk.rs`, `rlox-vm/src/value.rs`, `rlox-vm/src/debug.rs`, `rlox-vm/src/lib.rs`.

### Task M4.1 — `value.rs` (vm)  [parallel: agent V-val]

- [ ] `pub enum Value { Nil, Bool(bool), Number(f64), Obj(*mut Obj) }` — `Obj` type is forward-declared in `object.rs` (M6). For M4, gate `Obj` variant behind `#[allow(dead_code)]` or use a placeholder `Obj = ();` until object module lands.
- [ ] Methods: `is_falsey()`, `equals(&Value) -> bool`, `Display` per book.
- [ ] Tests: number equality, nil equality, bool equality.
- [ ] **Commit:** `feat(vm): Value tagged union with book display format`.

### Task M4.2 — `chunk.rs`: OpCode + Chunk  [parallel: agent V-chunk]

- [ ] `pub enum OpCode { Return, Constant, ConstantLong, Nil, True, False, Pop, GetLocal, SetLocal, GetGlobal, DefineGlobal, SetGlobal, GetUpvalue, SetUpvalue, GetProperty, SetProperty, GetSuper, Equal, Greater, Less, Add, Subtract, Multiply, Divide, Not, Negate, Print, Jump, JumpIfFalse, Loop, Call, Invoke, SuperInvoke, Closure, CloseUpvalue, Class, Inherit, Method }` (align with book chapter order).
- [ ] `pub struct Chunk { pub code: Vec<u8>, pub lines: Vec<usize>, pub constants: Vec<Value> }`
- [ ] Methods: `new`, `write(byte, line)`, `write_op(op, line)`, `add_constant(v) -> usize`.
- [ ] Inline tests: write byte sequence, constant table.
- [ ] **Commit:** `feat(vm): Chunk with OpCode enum and constant pool`.

### Task M4.3 — `debug.rs`: Disassembler  [parallel: agent V-chunk]

- [ ] `pub fn disassemble_chunk(chunk: &Chunk, name: &str) -> String`
- [ ] `pub fn disassemble_instruction(chunk: &Chunk, offset: usize) -> (String, usize)`
- [ ] Format exactly per book Chapter 14 output (offset, line, opcode name, operand index, operand value).
- [ ] Tests: disassemble a 4-instruction chunk, diff against expected multiline string.
- [ ] **Commit:** `feat(vm): disassembler matches book output format`.

### Task M4.4 — M4 gate  [serial]

- [ ] `cargo test -p rlox-vm chunk_ debug_ value_` ≥ 10 tests.
- [ ] **Commit (if needed):** `chore(vm): wire M4 modules in lib.rs`.

---

## Milestone 5 — Scanner + Pratt Compiler + VM core (rlox-vm)

**Files:** `rlox-vm/src/scanner.rs`, `rlox-vm/src/compiler.rs`, `rlox-vm/src/vm.rs`.

### Task M5.1 — `scanner.rs` (vm)  [serial]

- [ ] On-demand scanner (book chap 16): `pub struct Scanner<'a> { source: &'a str, start: usize, current: usize, line: usize }` with `scan_token(&mut self) -> Token` returning one token per call.
- [ ] Reuse TokenType from parallel copy (duplicated crate-local). Tests mirror tree scanner.
- [ ] **Commit:** `feat(vm): on-demand Lox scanner`.

### Task M5.2 — `compiler.rs` (Pratt)  [serial]

- [ ] Port book Chapter 17-25 compiler: `ParseRule` table, `Parser` state (current/previous tokens + error flags), `Compiler` linked-list for function scopes + local-var array + upvalue array.
- [ ] Emit bytecode directly into target `Chunk`.
- [ ] Local-variable resolution (depth-based), jump patching for `if/while/for/and/or`, closure/upvalue capture.
- [ ] Tests `tests/compile_tests.rs` (≥ 10): constant expr, arithmetic precedence, var decl + global, local scope + shadowing, if/else jumps correct, while loop loop back, function decl emits Closure, closure captures upvalue.
- [ ] **Commit:** `feat(vm): single-pass Pratt compiler with closures`.

### Task M5.3 — `vm.rs` core execution loop  [serial]

- [ ] `pub struct Vm { frames: Vec<CallFrame>, stack: Vec<Value>, globals: HashMap<String, Value>, open_upvalues: Option<*mut ObjUpvalue> }` (open_upvalues placeholder for M6).
- [ ] `CallFrame { closure: *mut ObjClosure, ip: usize, slots_base: usize }` — for M5 without GC, use simple `Box::leak` for ObjFunction (single global closure), refine in M6.
- [ ] `pub fn interpret(source: &str) -> InterpretResult` — compiles, runs dispatch loop.
- [ ] Dispatch loop handles all non-class, non-method opcodes.
- [ ] Integration test: Fibonacci script prints 55; arithmetic script; closure counter.
- [ ] **Acceptance:** `cargo run -p rlox-vm -- examples/fib.lox` → `55`.
- [ ] **Commit:** `feat(vm): stack-based VM executes closures and arithmetic`.

---

## Milestone 6 — Heap objects + mark-sweep GC + classes (rlox-vm)

**Files:** `rlox-vm/src/object.rs`, `rlox-vm/src/gc.rs`, extend `compiler.rs` and `vm.rs` for classes/inheritance.

### Task M6.1 — `object.rs`: heap object hierarchy  [parallel: agent V-obj]

- [ ] Common header `#[repr(C)] pub struct Obj { pub kind: ObjKind, pub is_marked: bool, pub next: *mut Obj }` with variant structs prefixed by `Obj` header: `ObjString`, `ObjFunction`, `ObjClosure`, `ObjUpvalue`, `ObjClass`, `ObjInstance`, `ObjBoundMethod`, `ObjNative`.
- [ ] `pub enum ObjKind { String, Function, Closure, Upvalue, Class, Instance, BoundMethod, Native }`.
- [ ] String interning table inside GC (see M6.2): `HashMap<StringKey, *mut ObjString>` keyed by hash + length.
- [ ] Helpers: `new_string`, `new_function`, `new_closure`, `new_class`, `new_instance`, `new_bound_method`, `new_native`.
- [ ] Tests: allocate + read back string; function chunk access; closure captures upvalue pointer.
- [ ] **Commit:** `feat(vm): heap object types with shared Obj header`.

### Task M6.2 — `gc.rs`: mark-sweep + stress mode  [parallel: agent V-gc]

- [ ] `pub struct Gc { head: *mut Obj, strings: HashMap<u64, *mut ObjString>, bytes_allocated: usize, next_gc: usize, gray_stack: Vec<*mut Obj> }`
- [ ] `pub fn allocate<T>(&mut self, kind: ObjKind, init: T) -> *mut T`
- [ ] `pub fn collect_garbage(&mut self, vm: &mut Vm)` — mark roots (stack + frames + globals + open upvalues + compiler state), trace references, sweep unmarked.
- [ ] `pub fn intern_string(&mut self, s: &str) -> *mut ObjString`.
- [ ] Feature `gc_stress`: call `collect_garbage` before every allocation.
- [ ] Tests (`gc_tests.rs`, feature-gated): allocate 100 strings under stress mode, assert no crash; closure retains captured upvalue across GC; class with method table survives.
- [ ] **Commit:** `feat(vm): mark-sweep GC with stress feature flag`.

### Task M6.3 — classes + inheritance in compiler & vm  [serial]

- [ ] Extend `compiler.rs`: `class_declaration`, `method`, `this`, `super_`.
- [ ] Extend `vm.rs`: OP_CLASS, OP_INHERIT, OP_METHOD, OP_INVOKE, OP_SUPER_INVOKE, OP_GET_PROPERTY, OP_SET_PROPERTY, OP_GET_SUPER.
- [ ] Tests (`vm_tests.rs`): simple class instance field round-trip; method call; `this` binding; inheritance method override; `super.method()` call.
- [ ] **Commit:** `feat(vm): classes, methods, inheritance, super`.

### Task M6.4 — M6 gate + reviewer subagent  [serial]

- [ ] `cargo test -p rlox-vm` all green.
- [ ] `cargo test -p rlox-vm --features gc_stress` all green.
- [ ] `cargo run -p rlox-vm -- examples/class.lox` matches expected.
- [ ] Dispatch `superpowers:code-reviewer` subagent over `rlox-vm/`; fix blockers.
- [ ] **Commit fixes:** `fix(vm): address review findings`.

---

## Milestone 7 — Test-suite integration

**Files:** `test-suite/src/main.rs`, `test-suite/cases/` (vendored), `test-suite/cases/UPSTREAM.md`, `test-suite/cases/LICENSE`.

### Task M7.1 — Vendor upstream test scripts  [serial]

- [ ] Shell commands (agent runs):
  ```bash
  git clone --depth 1 https://github.com/munificent/craftinginterpreters /tmp/ci-upstream
  cd /tmp/ci-upstream && git rev-parse HEAD  # record SHA
  cp -R /tmp/ci-upstream/test ~/Work/rlox/test-suite/cases/
  cp /tmp/ci-upstream/LICENSE ~/Work/rlox/test-suite/cases/LICENSE
  ```
- [ ] Write `test-suite/cases/UPSTREAM.md` with upstream URL + SHA + MIT attribution.
- [ ] **Commit:** `chore(test-suite): vendor craftinginterpreters test scripts @ <SHA>`.

### Task M7.2 — Runner (`test-suite/src/main.rs`)  [serial]

- [ ] CLI: `test-suite --target {tree|vm|both} [--filter <pattern>]`.
- [ ] For each `.lox` in `cases/` recursively (skip `benchmark/`, `limit/` per book conventions):
  - Parse `// expect: <line>` and `// expect runtime error: <msg>` and `// [line N] Error ...` annotations.
  - Invoke `target/debug/rlox-<target> <file>` via `std::process::Command`, capture stdout + stderr + exit code.
  - Diff against expectations.
  - Tally pass/fail, print summary.
- [ ] Tree implementation is expected to skip some advanced cases (book is explicit — `benchmark/`, limit tests). Hard-coded skip list keyed to upstream path.
- [ ] Exit code: 0 if ≥95% pass on selected target(s), else 1.
- [ ] **Acceptance:** `cargo run -p test-suite --release -- --target both` prints:
  - `rlox-tree: XXX/YYY passed (ZZ.Z%)` where ZZ.Z ≥ 95.0
  - `rlox-vm:   XXX/YYY passed (ZZ.Z%)` where ZZ.Z ≥ 95.0
  - Exits 0.
- [ ] **Commit:** `feat(test-suite): Lox test runner with expect-directive parsing`.

### Task M7.3 — Fix regressions surfaced by test suite  [serial, iterative]

- [ ] Enumerate failing cases from M7.2 output. For each failing bucket, fix root cause in `rlox-tree` or `rlox-vm`, re-run suite.
- [ ] Commit per bucket: `fix(tree|vm): <failing-case-description>`.

---

## Milestone 8 — Quality gate

### Task M8.1 — Fmt + clippy clean  [serial]

- [ ] `cargo fmt --all` then `cargo fmt --check` exit 0.
- [ ] `cargo clippy --workspace --all-targets -- -D warnings` exit 0 (address every warning).
- [ ] **Commit:** `style: cargo fmt + clippy clean across workspace`.

### Task M8.2 — Final reviewer subagent pass  [serial]

- [ ] Dispatch `superpowers:code-reviewer` over full workspace with the spec as ground truth.
- [ ] Fix all blocking items; commit as `fix: address final review`.

### Task M8.3 — Update PLAN.md status + README smoke log  [serial]

- [ ] Mark every milestone completed with date + commit SHA.
- [ ] Append to README: "Final smoke" section showing `cargo run -p test-suite -- --target both` output.
- [ ] **Commit:** `docs: finalize PLAN.md and README smoke log`.

---

## Installed Tools

| Tool | Install command | Installed | Reason | Uninstall |
|---|---|---|---|---|
| rust (rustc + cargo + rustfmt + clippy) | `brew install rust` | 2026-04-20 | Rust toolchain required by all three crates | `brew uninstall rust` |

(append rows here as tools are added during execution)

---

## Risks

| Risk | Probability | Impact | Mitigation |
|---|---|---|---|
| GC mis-traces a root → UAF in VM | M | H | `gc_stress` mandatory test; reviewer subagent audits root set |
| Parser grammar drift from book | L | M | Cross-reference book chapter numbers in comments; cover with tests mirroring book examples |
| Upstream test format quirks (CRLF, trailing newline) | M | L | Runner normalizes line endings; golden parsing tested on 3 sample files first |
| Scope creep (temptation to add lambdas/arrays) | M | M | Spec locks scope; re-entering brainstorming required for extension |
| `unsafe` pointer arithmetic bugs | M | H | Each `unsafe` block has `// SAFETY:` comment; miri run on a subset of vm tests recommended |

---

## Change Log

- 2026-04-20: PLAN.md drafted from approved spec `docs/specs/2026-04-20-lox-rust-design.md`.
