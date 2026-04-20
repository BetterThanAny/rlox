# Vendored Test Suite

These `.lox` scripts come from Robert Nystrom's [Crafting Interpreters](https://github.com/munificent/craftinginterpreters) companion repository.

- **Upstream URL:** https://github.com/munificent/craftinginterpreters
- **Upstream path:** `test/`
- **Upstream commit SHA:** `4a840f70f69c6ddd17cfef4f6964f8e1bcd8c3d4`
- **Vendored on:** 2026-04-20
- **File count:** 265 `.lox` scripts across ~20 categories

Each script contains inline directives the runner parses:

- `// expect: <stdout line>` — the following line must appear on stdout in order.
- `// expect runtime error: <msg>` — stderr must contain `<msg>` and the interpreter must exit with code 70.
- `// [line N] Error <something>` or `// [line N] <interpreter-specific>` — compile-/resolve-error line annotations (exit code 65).

The upstream `LICENSE` is MIT; it is preserved alongside this file in `LICENSE`.

## Scope notes for rlox

Some categories test features that belong to only one of the two book implementations:

- `limit/` — hard limits (256 constants, 256 params, 256 upvalues). We expect these to pass on `rlox-vm` (clox-equivalent) and do NOT run them against `rlox-tree` (the book's jlox doesn't enforce all of them).
- `benchmark/` — micro-benchmarks, not correctness tests. Skipped by default.
- `scanning/` — isolates scanner output; skipped by default (our scanners are exercised by per-crate unit tests).

The runner honors these skips via a hard-coded `SKIP_PREFIXES` table documented in `test-suite/src/main.rs`.
