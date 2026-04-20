# Design Spec — rlox: Crafting Interpreters in Rust

**Date:** 2026-04-20
**Status:** Approved — ready for planning
**Owner:** Claude Code session

## 1. Goal

Implement the Lox programming language from Robert Nystrom's *Crafting Interpreters* book, in Rust, as two separate but coexisting implementations:

1. **`rlox-tree`** — tree-walking interpreter (equivalent to the book's `jlox`, originally Java).
2. **`rlox-vm`** — single-pass bytecode compiler + stack VM with mark-sweep GC (equivalent to the book's `clox`, originally C).

Both implementations must pass the official Lox test suite shipped in `munificent/craftinginterpreters` with ≥95% acceptance.

Scope is **strictly book-equivalent**: no language extensions (no modules, arrays, lambdas beyond closures, pattern matching, etc.). Deviations are limited to what Rust's memory model requires.

## 2. Non-Goals

- No JIT, no optimization passes, no incremental compilation.
- No language-server, no syntax-highlighting tooling.
- No public crate publication to crates.io.
- No GitHub push / PR workflow (local-only delivery).

## 3. Architecture

### 3.1 Repository layout

Single Cargo workspace at `~/Work/rlox/`:

```
rlox/
├── Cargo.toml                    # [workspace] with 3 members
├── .gitignore
├── README.md
├── PLAN.md                       # milestone tracker (created by writing-plans)
├── docs/
│   └── specs/
│       └── 2026-04-20-lox-rust-design.md
├── examples/                     # hand-crafted .lox scripts for smoke tests
│   ├── hello.lox
│   ├── fib.lox
│   ├── closure.lox
│   └── class.lox
├── rlox-tree/
│   ├── Cargo.toml
│   ├── src/
│   │   ├── main.rs               # binary: REPL + file runner
│   │   ├── lib.rs                # re-exports
│   │   ├── token.rs              # Token + TokenType enum
│   │   ├── scanner.rs            # source → Vec<Token>
│   │   ├── ast.rs                # Expr + Stmt enums (boxed AST)
│   │   ├── parser.rs             # tokens → Vec<Stmt>
│   │   ├── resolver.rs           # static resolution pass
│   │   ├── interpreter.rs        # Stmt/Expr evaluator
│   │   ├── environment.rs        # Rc<RefCell<Environment>>
│   │   ├── value.rs              # LoxValue + LoxFunction + LoxClass + LoxInstance
│   │   └── error.rs              # LoxError (thiserror)
│   └── tests/
│       ├── scanner_tests.rs
│       ├── parser_tests.rs
│       └── interpret_tests.rs
├── rlox-vm/
│   ├── Cargo.toml
│   ├── src/
│   │   ├── main.rs               # binary: REPL + file runner
│   │   ├── lib.rs
│   │   ├── scanner.rs            # duplicated (book keeps them separate; deliberate)
│   │   ├── chunk.rs              # Chunk + OpCode
│   │   ├── value.rs              # Value tagged union
│   │   ├── debug.rs              # disassembler
│   │   ├── object.rs             # Obj variants: String/Function/Closure/Class/Instance/BoundMethod/Upvalue
│   │   ├── compiler.rs           # Pratt parser → bytecode
│   │   ├── vm.rs                 # stack machine
│   │   └── gc.rs                 # mark-sweep with stress mode
│   └── tests/
│       ├── chunk_tests.rs
│       ├── compile_tests.rs
│       └── vm_tests.rs
└── test-suite/
    ├── Cargo.toml                # workspace member, binary only
    ├── src/main.rs               # CLI: --target=tree|vm|both
    └── cases/                    # vendored from munificent/craftinginterpreters /test
        ├── assignment/
        ├── block/
        ├── bool/
        ├── call/
        ├── class/
        ├── closure/
        ├── ... (full Lox test set)
        └── LICENSE               # MIT from upstream
```

### 3.2 Data flow

**rlox-tree:**
```
source (String)
  → scanner → Vec<Token>
  → parser  → Vec<Stmt>  (AST)
  → resolver (mutates a side-table: HashMap<ExprId, depth>)
  → interpreter (evaluates with Rc<RefCell<Environment>>)
```

**rlox-vm:**
```
source (String)
  → scanner (produces tokens on-demand, no full Vec<Token>)
  → compiler (Pratt parser emitting bytecode directly into a Chunk)
  → VM (fetches bytecode, executes on value stack, interacts with GC heap)
```

### 3.3 Key type decisions

| Concern | rlox-tree | rlox-vm |
|---|---|---|
| Values | `enum LoxValue` with `Rc` for heap types | `enum Value` with `*mut Obj` for heap (unsafe) |
| Env / scope | `Rc<RefCell<Environment>>` chain | resolved offsets into call frames + upvalues |
| Strings | `Rc<String>` | interned `*mut ObjString` via GC |
| Closures | capture env Rc | explicit upvalue arrays |
| Classes | `Rc<RefCell<LoxClass>>` with method `HashMap` | `*mut ObjClass` + method table |
| GC | none (Rc ref-counted) | mark-sweep, triggered by allocation threshold + stress mode |
| Error model | `thiserror` enum, early return | `InterpretResult::{Ok, CompileError, RuntimeError}` |

### 3.4 unsafe policy for rlox-vm

- `unsafe` is localized to: (a) raw pointer deref for heap objects, (b) GC mark/sweep traversal, (c) string interning table keyed by raw `*mut ObjString`.
- Every `unsafe` block has a `// SAFETY:` comment naming the invariant.
- `cargo test` runs with `RUSTFLAGS=-Zsanitizer=address` under a separate CI-style script (documented, not required for main acceptance).
- A `gc_stress` feature flag forces GC on every allocation; `cargo test --features gc_stress` must pass before M6 is closed.

## 4. Milestones

| # | Deliverable | Mechanical acceptance |
|---|---|---|
| M1 | Workspace + Scanner + Token + AST (tree) | `cargo test -p rlox-tree scanner_ ast_` passes ≥ 20 tests |
| M2 | Parser + Resolver (tree) | `cargo test -p rlox-tree parser_ resolver_` passes ≥ 30 tests |
| M3 | Interpreter + Env + Value + native `clock()` + REPL (tree) | all `examples/*.lox` produce expected output; `interpret_tests` ≥ 15 cases |
| M4 | Chunk + OpCodes + Value + Disassembler (vm) | `cargo test -p rlox-vm chunk_ debug_` ≥ 10 tests |
| M5 | Single-pass Pratt compiler + VM core (vm) | all `examples/*.lox` (pre-class subset) run under vm binary |
| M6 | Heap objects + mark-sweep GC + classes + inheritance (vm) | `cargo test -p rlox-vm --features gc_stress` passes; all `examples/*.lox` run |
| M7 | test-suite integration | `cargo run -p test-suite -- --target both` ≥ 95% pass on each target |
| M8 | Quality gate | `cargo fmt --check && cargo clippy --workspace --all-targets -- -D warnings` clean; code-reviewer subagent approves |

## 5. Parallelization plan

Work is dispatched in **three waves**, each preceded by a serial scaffolding step and followed by a serial integration step.

- **Serial S1 (blocks Wave 1):** Scaffold workspace + `rlox-tree/src/token.rs` stub + shared error module.
- **Wave 1 (parallel, after S1):**
  - Agent T1: `scanner.rs` + `scanner_tests.rs`
  - Agent T2: `ast.rs` + AST construction tests
  - Agent T3: `error.rs` hardening + value.rs skeleton
- **Serial S2:** integrate Wave 1, run `cargo test -p rlox-tree --lib`, fix glue.
- **Wave 2 (parallel, after S2):**
  - Agent T4: `parser.rs` + parser tests
  - Agent T5: `resolver.rs` + resolver tests
  - Agent T6: `environment.rs` + env tests
- **Serial S3:** integrate Wave 2.
- **Serial S4:** `interpreter.rs` (depends on all previous tree modules — single agent to keep coherent) + examples + REPL.
- **Checkpoint: M3 ships rlox-tree binary. Code-reviewer subagent pass.**
- **Serial S5:** scaffold `rlox-vm` crate + scanner duplicate.
- **Wave 3 (parallel, after S5):**
  - Agent V1: `chunk.rs` + `debug.rs` + chunk tests
  - Agent V2: `value.rs` skeleton + compile-time unit tests
- **Serial S6:** `compiler.rs` (depends on scanner + chunk) — single agent.
- **Serial S7:** `vm.rs` core (depends on compiler + chunk) — single agent.
- **Wave 4 (parallel, after S7):**
  - Agent V3: `object.rs` + string interning
  - Agent V4: `gc.rs` + stress feature + mark/sweep
- **Serial S8:** classes + inheritance in compiler/vm (single agent, touches multiple files).
- **Checkpoint: M6 ships rlox-vm binary. Code-reviewer subagent pass.**
- **Serial S9:** `test-suite` crate: vendor upstream tests + runner. Single agent.
- **Checkpoint: M7 + M8.**

## 6. Error handling

- User-facing errors match book output format: `[line N] Error[ at LEX]: message`.
- Internal plumbing via `Result<T, LoxError>` (tree) and `InterpretResult` (vm).
- Binaries exit with POSIX code 65 (compile error) / 70 (runtime error) per book.
- No `panic!` on user input. `unwrap`/`expect` permitted only on invariants the type system already guarantees.

## 7. Testing strategy

- **Unit tests:** colocated in each module via `#[cfg(test)]`. This is the
  chosen shape across the whole workspace — the `tests/<module>_tests.rs`
  files listed in §3.1 are descriptive of test *intent*, not physical layout.
- **Integration tests per crate:** `rlox-tree/tests/interpret_tests.rs` and
  `rlox-vm/tests/vm_tests.rs` run embedded `.lox` strings through the full
  pipeline.
- **End-to-end test suite:** `test-suite` crate runs vendored Lox scripts, diffs stdout against `// expect: <line>` comments in source (exact book convention).
- **GC stress:** `cargo test -p rlox-vm --features gc_stress`.
- **Lint:** `cargo fmt --check` + `cargo clippy --workspace --all-targets -- -D warnings`.

## 8. Vendored upstream test cases

- Source: `https://github.com/munificent/craftinginterpreters` (MIT).
- Procedure: `git clone` into `/tmp`, copy `test/` subtree into `test-suite/cases/`, commit `test-suite/cases/LICENSE` reproducing upstream MIT text.
- Expected counts: ~240 test scripts across ~20 categories (`assignment`, `block`, `bool`, `call`, `class`, `closure`, `comments`, `constructor`, `expressions`, `field`, `for`, `function`, `if`, `inheritance`, `logical_operator`, `method`, `nil`, `number`, `operator`, `print`, `regression`, `return`, `scanning`, `string`, `super`, `this`, `variable`, `while`).
- Some scripts are intentionally *expected to error*; runner must parse `// [line N] Error ...` directives too.

## 9. Dependencies

Rust `edition = "2021"`. External crates kept minimal:

| Crate | Purpose | Used by |
|---|---|---|
| `thiserror` | ergonomic error enums | both |
| `anyhow` | binary entry error glue | main.rs files |
| `rustyline` | REPL line editing | both main.rs |
| (stdlib only for core logic) | | scanner/parser/etc. |

No `serde`, no async, no macros beyond `thiserror`. `test-suite` uses stdlib `std::process::Command`.

## 10. Risks & mitigations

| Risk | Impact | Mitigation |
|---|---|---|
| GC bugs (use-after-free, missed marks) | Silent corruption, test flakiness | `gc_stress` feature + every new object type explicitly registered in mark traversal + review checklist |
| Borrow-checker fights in tree-walk `Environment` | Schedule slip | Use book's canonical `Rc<RefCell>` pattern; do not over-engineer |
| Upstream test suite format drift | Runner parses wrongly | Pin to a specific upstream commit SHA in vendoring script; record SHA in `test-suite/cases/UPSTREAM.md` |
| Workspace-level cyclic imports | Compile failure | Each crate strictly leaf; test-suite depends on neither (invokes via `std::process::Command` against built binaries) |
| Scope creep | Token/time waste | Spec locked; any extension requires re-entering brainstorming |

## 11. Success definition

All of the following are true simultaneously:

1. `cargo build --workspace --release` produces three binaries (`rlox-tree`, `rlox-vm`, `test-suite`) with zero warnings.
2. `cargo test --workspace` passes.
3. `cargo test -p rlox-vm --features gc_stress` passes.
4. `cargo run -p test-suite -- --target both` reports ≥95% pass on each target.
5. `cargo fmt --check && cargo clippy --workspace --all-targets -- -D warnings` exit 0.
6. Code-reviewer subagent final report names no blocking issues.
7. `examples/` scripts produce expected output under both binaries.

## 12. Out-of-scope deferrals (intentional)

- IDE tooling / LSP.
- Performance tuning beyond book chapters 24-30 (optimization is book-native, included).
- Windows-specific path handling (dev target is macOS/Linux; paths assume `/`).
- Concurrency / threading.
- JIT tier above bytecode VM.
