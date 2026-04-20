# rlox

Two Rust implementations of Lox from Robert Nystrom's *Crafting Interpreters*:

- `rlox-tree` — tree-walking interpreter (book's jlox equivalent).
- `rlox-vm` — single-pass bytecode compiler + stack VM with `Rc`-managed heap (book's clox equivalent; mark-sweep GC refactor deferred — see `PLAN.md`).
- `test-suite` — runs the official 265-script Lox test suite against both.

## Build

```
cargo build --workspace --release
```

## Run

```
cargo run -p rlox-tree -- examples/fib.lox
cargo run -p rlox-vm   -- examples/fib.lox
cargo run -p rlox-tree                        # REPL
```

## Test

```
cargo test --workspace
cargo run --release -p test-suite -- --target both
```

## Caveats

- **Tree-walker recursion depth is bounded by the Rust thread stack.**
  Deeply nested grouping or tail-recursive Lox programs that exceed ~1500
  levels of parser recursion / ~20 000 levels of interpreter recursion will
  abort with `fatal runtime error: stack overflow`. `rlox-vm` caps call
  frames at 64 and reports `Stack overflow.` cleanly instead — prefer it
  for adversarial input. See `PLAN.md` Change Log for the planned fix.
- GC: `Rc`-based reclamation (correct for all non-cyclic programs; leaks
  unreachable closure cycles, which the book's mark-sweep handles). The
  `gc_stress` Cargo feature is declared but inert pending the refactor.

## Status

All eight milestones shipped. See `PLAN.md` for the full table and deferred follow-ups. Design spec lives at `docs/specs/2026-04-20-lox-rust-design.md`.

## Final smoke

```
$ cargo build --workspace --release
    Finished `release` profile [optimized] target(s) in 1.11s

$ cargo test --workspace 2>&1 | grep "^test result:" | awk '{sum+=$4} END {print "TOTAL:", sum}'
TOTAL: 210

$ cargo clippy --workspace --all-targets -- -D warnings
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.18s

$ cargo fmt --check --all
# (silent, exit 0)

$ ./target/release/rlox-tree examples/class.lox
Rex says woof!
Rex makes a sound.

$ ./target/release/rlox-vm examples/class.lox
Rex says woof!
Rex makes a sound.

$ ./target/release/test-suite --target both | tail -2
Overall: rlox-tree 99.6% | rlox-vm 98.8% | threshold 95.0%
```

Identical output from the tree-walker and the bytecode VM across all four scripts in `examples/`. Vendored 265-script official test suite passes above the 95% gate on both targets.
