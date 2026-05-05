# Review and Fix Report

## Changes
- Changed VM native functions to carry name, arity, and function pointer instead of a bare pointer.
- Added native arity checking so `clock(1)` reports `Expected 0 arguments but got 1.`
- Stopped `close_upvalues()` from overwriting a shared upvalue cell with an old stack slot value.
- Added VM regression tests for upvalue mutation persistence and native arity.

## Verification
- `cargo test -p rlox-vm` passed.
- Worker also ran manual `cargo run` reproductions for upvalue mutation and `clock(1)`.
- `git diff --check` passed.

## Remaining
- The tree-walk REPL resolver ID collision was not fixed; it needs a broader parser/resolver state design.
