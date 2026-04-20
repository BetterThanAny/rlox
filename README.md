# rlox

Two Rust implementations of Lox from Robert Nystrom's *Crafting Interpreters*:

- `rlox-tree` — tree-walking interpreter (book's jlox equivalent).
- `rlox-vm` — single-pass bytecode compiler + stack VM with mark-sweep GC (book's clox equivalent).
- `test-suite` — runs the official Lox test scripts against both.

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
cargo test -p rlox-vm --features gc_stress
cargo run -p test-suite -- --target both
```

## Status

See `PLAN.md` for milestone progress. Design spec lives at `docs/specs/2026-04-20-lox-rust-design.md`.
