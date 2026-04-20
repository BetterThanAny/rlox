//! rlox test-suite runner — Milestone 7.
//!
//! Walks `test-suite/cases/` for every `.lox` file, extracts expectation
//! directives from comments, invokes the target binary (`rlox-tree` or
//! `rlox-vm`) and diffs captured stdout / stderr / exit code against the
//! directives.
//!
//! CLI: `test-suite [--target <tree|vm|both>] [--filter <substring>]
//!                  [--verbose] [--binary-dir <path>]`

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::{bail, Context, Result};

/// Hard-coded skip prefixes (relative to `cases/`).
const ALWAYS_SKIP: &[&str] = &[
    "benchmark/",               // perf, not correctness
    "scanning/",                // scanner-only; no parser/runtime
    "expressions/",             // unimplemented chapter-4 intermediates
    "limit/loop_too_large.lox", // needs a 10k-line test too slow for CI-ish
];

/// Limit tests are clox-specific: skip when running `tree` target.
const TREE_ONLY_SKIP: &[&str] = &["limit/"];
/// Categories specific to the vm target (currently none).
const VM_ONLY_SKIP: &[&str] = &[];

const PASS_THRESHOLD: f64 = 95.0;

// --------------------------------------------------------------------------
// parse_directives — extract `// expect: ...` and friends from Lox source.
// --------------------------------------------------------------------------

mod parse_directives {
    /// A single expectation pulled from a `.lox` source file.
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub enum Expect {
        /// `// expect: <text>` — stdout line (in source order).
        Stdout(String),
        /// `// expect runtime error: <msg>` — stderr substring + exit 70.
        RuntimeError(String),
        /// `// Error ...` / `// [line N] Error ...` — compile-time error.
        ///
        /// `msg` is the full matched directive payload starting with `Error`,
        /// e.g. `"Error at '=': Invalid assignment target."`.
        CompileError { line: usize, msg: String },
    }

    /// Scan the source for directive comments and return them in source order.
    ///
    /// This is the untagged convenience form used by inline tests; production
    /// code calls [`extract_tagged`] directly so it can filter by target.
    ///
    /// Rules implemented:
    /// * `// expect: <text>` → `Stdout`.
    /// * `// expect runtime error: <msg>` → `RuntimeError`.
    /// * `// [line N] Error <rest>` → explicit-line compile error.
    /// * `// [java line N] Error <rest>` → jlox-only compile error (keep;
    ///   caller filters by target).
    /// * `// [c line N] Error <rest>` → clox-only compile error (keep;
    ///   caller filters by target).
    /// * `// Error <rest>` or `// Error at '<x>': <msg>` → compile error on
    ///   the line the comment appears on.
    ///
    /// Lines that don't match any of the above are ignored.
    #[cfg(test)]
    pub fn extract(source: &str) -> Vec<Expect> {
        extract_tagged(source)
            .into_iter()
            .map(|(_, exp)| exp)
            .collect()
    }

    /// Tagged extract — returns `(tag, expect)` pairs so the runner can
    /// filter `java`/`c`-only directives per target. `tag` is `None` for
    /// untagged directives, `Some("java")` / `Some("c")` for tagged ones.
    pub fn extract_tagged(source: &str) -> Vec<(Option<&'static str>, Expect)> {
        let mut out = Vec::new();
        for (i, raw_line) in source.lines().enumerate() {
            let line_number = i + 1;
            // Find the first `//` that isn't inside a string. The book's test
            // suite never puts `//` inside a string that matters, so a plain
            // find is enough in practice — but guard against the obvious case
            // of an unterminated string by requiring the `//` to be preceded
            // by whitespace, `;`, `{`, `(`, `=` or column 0.
            let Some(idx) = find_comment(raw_line) else {
                continue;
            };
            let comment = raw_line[idx + 2..].trim_start();

            if let Some(rest) = comment.strip_prefix("expect: ") {
                out.push((None, Expect::Stdout(rest.to_string())));
                continue;
            }
            // Book files sometimes write `// expect:` with no trailing space
            // when the expected stdout line is empty.
            if let Some(rest) = comment.strip_prefix("expect:") {
                // Only accept if the next char is end-of-line — otherwise the
                // `:` would be part of some other word.
                if rest.is_empty() {
                    out.push((None, Expect::Stdout(String::new())));
                    continue;
                }
            }
            if let Some(rest) = comment.strip_prefix("expect runtime error: ") {
                out.push((None, Expect::RuntimeError(rest.to_string())));
                continue;
            }
            if let Some(rest) = comment.strip_prefix("[line ") {
                if let Some((line_str, after)) = rest.split_once("] ") {
                    if let Ok(n) = line_str.parse::<usize>() {
                        // after == "Error <rest>" or "Error at 'x': <msg>"
                        if after.starts_with("Error") {
                            out.push((
                                None,
                                Expect::CompileError {
                                    line: n,
                                    msg: after.to_string(),
                                },
                            ));
                            continue;
                        }
                    }
                }
            }
            if let Some(rest) = comment.strip_prefix("[java line ") {
                if let Some((line_str, after)) = rest.split_once("] ") {
                    if let Ok(n) = line_str.parse::<usize>() {
                        if after.starts_with("Error") {
                            out.push((
                                Some("java"),
                                Expect::CompileError {
                                    line: n,
                                    msg: after.to_string(),
                                },
                            ));
                            continue;
                        }
                    }
                }
            }
            if let Some(rest) = comment.strip_prefix("[c line ") {
                if let Some((line_str, after)) = rest.split_once("] ") {
                    if let Ok(n) = line_str.parse::<usize>() {
                        if after.starts_with("Error") {
                            out.push((
                                Some("c"),
                                Expect::CompileError {
                                    line: n,
                                    msg: after.to_string(),
                                },
                            ));
                            continue;
                        }
                    }
                }
            }
            if comment.starts_with("Error") {
                out.push((
                    None,
                    Expect::CompileError {
                        line: line_number,
                        msg: comment.to_string(),
                    },
                ));
                continue;
            }
        }
        out
    }

    /// Locate the start of a Lox line comment (`//`) while ignoring any `//`
    /// that appears inside a double-quoted string literal earlier on the
    /// line. Returns the byte index of the first `/` of the `//`, or `None`.
    fn find_comment(line: &str) -> Option<usize> {
        let bytes = line.as_bytes();
        let mut i = 0;
        let mut in_string = false;
        while i + 1 < bytes.len() {
            let c = bytes[i];
            if c == b'"' {
                in_string = !in_string;
            } else if !in_string && c == b'/' && bytes[i + 1] == b'/' {
                return Some(i);
            }
            i += 1;
        }
        None
    }
}

// --------------------------------------------------------------------------
// runner — invoke a binary and diff against Expect list.
// --------------------------------------------------------------------------

mod runner {
    use super::parse_directives::Expect;
    use std::path::Path;
    use std::process::Command;

    /// Result of running a single test case.
    pub struct Outcome {
        pub passed: bool,
        pub failures: Vec<String>,
    }

    /// Run the target binary against `lox_path`, compare the captured output
    /// to `expects`, and return the outcome.
    pub fn run_case(binary: &Path, lox_path: &Path, expects: &[Expect]) -> Outcome {
        let mut failures = Vec::new();
        let output = match Command::new(binary).arg(lox_path).output() {
            Ok(o) => o,
            Err(e) => {
                failures.push(format!("failed to spawn {}: {e}", binary.display()));
                return Outcome {
                    passed: false,
                    failures,
                };
            }
        };
        let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
        let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
        let exit = output.status.code().unwrap_or(-1);

        // Partition expectations by kind.
        let mut expected_stdout: Vec<&str> = Vec::new();
        let mut runtime_err: Option<&str> = None;
        let mut compile_errs: Vec<(usize, &str)> = Vec::new();
        for e in expects {
            match e {
                Expect::Stdout(s) => expected_stdout.push(s.as_str()),
                Expect::RuntimeError(m) => runtime_err = Some(m.as_str()),
                Expect::CompileError { line, msg } => compile_errs.push((*line, msg.as_str())),
            }
        }

        let has_expects = !expects.is_empty();
        let stdout_lines: Vec<&str> = stdout.lines().collect();

        // Compare stdout lines in order.
        for (i, want) in expected_stdout.iter().enumerate() {
            match stdout_lines.get(i) {
                Some(got) if got == want => {}
                Some(got) => {
                    failures.push(format!("stdout[{i}]: expected {want:?} got {got:?}"));
                }
                None => {
                    failures.push(format!("stdout[{i}]: expected {want:?} but stdout ended"));
                }
            }
        }
        // If there's no runtime/compile error expected but extra stdout was
        // produced, flag it only if we had explicit stdout expectations.
        if runtime_err.is_none()
            && compile_errs.is_empty()
            && !expected_stdout.is_empty()
            && stdout_lines.len() > expected_stdout.len()
        {
            failures.push(format!(
                "stdout: {} extra line(s), first extra: {:?}",
                stdout_lines.len() - expected_stdout.len(),
                stdout_lines[expected_stdout.len()]
            ));
        }

        // Runtime error expectations.
        if let Some(msg) = runtime_err {
            if exit != 70 {
                failures.push(format!("exit: expected 70 got {exit}"));
            }
            if !stderr.contains(msg) {
                failures.push(format!("stderr: expected to contain {msg:?}"));
            }
        }

        // Compile-error expectations.
        if !compile_errs.is_empty() {
            // clox/jlox exit 65 for compile-/resolve-time errors.
            if exit != 65 {
                failures.push(format!("exit: expected 65 got {exit}"));
            }
            for (line, msg) in &compile_errs {
                // The runner accepts either the full "[line N] Error ..."
                // form or the plainer "Error at ..." form, as long as the
                // line number and the message substring both appear.
                let want_framed = format!("[line {line}] {msg}");
                if !stderr.contains(&want_framed) {
                    failures.push(format!("stderr: expected to contain {want_framed:?}"));
                }
            }
        }

        // No directives at all → any exit 0 run is fine.
        if !has_expects {
            if exit != 0 {
                failures.push(format!(
                    "no directives: expected clean exit 0, got {exit}; stderr={:?}",
                    truncate(&stderr, 200)
                ));
            }
        } else if runtime_err.is_none() && compile_errs.is_empty() && exit != 0 {
            // Pure stdout expectations but the binary errored out.
            failures.push(format!(
                "exit: expected 0 got {exit}; stderr={:?}",
                truncate(&stderr, 200)
            ));
        }

        Outcome {
            passed: failures.is_empty(),
            failures,
        }
    }

    fn truncate(s: &str, n: usize) -> String {
        if s.len() <= n {
            s.to_string()
        } else {
            format!("{}…", &s[..n])
        }
    }
}

// --------------------------------------------------------------------------
// CLI + orchestration.
// --------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Target {
    Tree,
    Vm,
    Both,
}

#[derive(Debug)]
struct Args {
    target: Target,
    filter: Option<String>,
    verbose: bool,
    binary_dir: PathBuf,
}

impl Default for Args {
    fn default() -> Self {
        Self {
            target: Target::Both,
            filter: None,
            verbose: false,
            binary_dir: PathBuf::from("target/release"),
        }
    }
}

fn parse_args() -> Result<Args> {
    let mut args = Args::default();
    let mut it = std::env::args().skip(1);
    while let Some(a) = it.next() {
        match a.as_str() {
            "--target" => {
                let v = it.next().context("--target needs a value")?;
                args.target = match v.as_str() {
                    "tree" => Target::Tree,
                    "vm" => Target::Vm,
                    "both" => Target::Both,
                    other => bail!("unknown --target {other}; expected tree|vm|both"),
                };
            }
            "--filter" => {
                args.filter = Some(it.next().context("--filter needs a value")?);
            }
            "--verbose" | "-v" => {
                args.verbose = true;
            }
            "--binary-dir" => {
                args.binary_dir = PathBuf::from(it.next().context("--binary-dir needs a value")?);
            }
            "--help" | "-h" => {
                println!(
                    "test-suite [--target tree|vm|both] [--filter SUB] [--verbose] [--binary-dir DIR]"
                );
                std::process::exit(0);
            }
            other => bail!("unknown argument {other}"),
        }
    }
    Ok(args)
}

/// Repo root: two directories up from the binary's manifest dir (which points
/// to `test-suite/`). Fallback to cwd.
fn repo_root() -> PathBuf {
    // `CARGO_MANIFEST_DIR` is only set at build time. Instead, locate the
    // `test-suite/cases/` directory relative to cwd, walking upward if
    // necessary.
    let mut cur = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    for _ in 0..6 {
        if cur.join("test-suite/cases").is_dir() {
            return cur;
        }
        if !cur.pop() {
            break;
        }
    }
    std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
}

fn collect_cases(cases_root: &Path) -> Result<Vec<(String, PathBuf)>> {
    let mut out = Vec::new();
    walk(cases_root, cases_root, &mut out)?;
    out.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(out)
}

fn walk(root: &Path, dir: &Path, out: &mut Vec<(String, PathBuf)>) -> Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        let ft = entry.file_type()?;
        if ft.is_dir() {
            walk(root, &path, out)?;
        } else if ft.is_file() && path.extension().and_then(|s| s.to_str()) == Some("lox") {
            let rel = path
                .strip_prefix(root)
                .unwrap_or(&path)
                .to_string_lossy()
                .replace('\\', "/");
            out.push((rel, path));
        }
    }
    Ok(())
}

fn should_skip(rel_path: &str, target: Target) -> bool {
    if ALWAYS_SKIP.iter().any(|p| rel_path.starts_with(p)) {
        return true;
    }
    match target {
        Target::Tree => TREE_ONLY_SKIP.iter().any(|p| rel_path.starts_with(p)),
        Target::Vm => VM_ONLY_SKIP.iter().any(|p| rel_path.starts_with(p)),
        Target::Both => false, // handled per-sub-run
    }
}

/// Filter tag-specific directives: keep `None` always, keep `Some("java")`
/// only for tree, keep `Some("c")` only for vm.
fn filter_directives(
    raw: Vec<(Option<&'static str>, parse_directives::Expect)>,
    target_kind: TargetKind,
) -> Vec<parse_directives::Expect> {
    raw.into_iter()
        .filter(|(tag, _)| {
            matches!(
                (tag, target_kind),
                (None, _) | (Some("java"), TargetKind::Tree) | (Some("c"), TargetKind::Vm)
            )
        })
        .map(|(_, exp)| exp)
        .collect()
}

#[derive(Debug, Clone, Copy)]
enum TargetKind {
    Tree,
    Vm,
}

impl TargetKind {
    fn label(self) -> &'static str {
        match self {
            TargetKind::Tree => "rlox-tree",
            TargetKind::Vm => "rlox-vm",
        }
    }
    fn binary_name(self) -> &'static str {
        match self {
            TargetKind::Tree => "rlox-tree",
            TargetKind::Vm => "rlox-vm",
        }
    }
}

struct Summary {
    label: &'static str,
    total: usize,
    passed: usize,
    failures: Vec<(String, Vec<String>)>,
    per_dir: BTreeMap<String, (usize, usize)>, // (pass, total)
}

impl Summary {
    fn pct(&self) -> f64 {
        if self.total == 0 {
            0.0
        } else {
            100.0 * self.passed as f64 / self.total as f64
        }
    }
}

fn run_target(
    kind: TargetKind,
    binary: &Path,
    cases: &[(String, PathBuf)],
    filter: Option<&str>,
    verbose: bool,
) -> Result<Summary> {
    if !binary.exists() {
        bail!(
            "binary not found: {} — run `cargo build --release --workspace` first",
            binary.display()
        );
    }

    let target = match kind {
        TargetKind::Tree => Target::Tree,
        TargetKind::Vm => Target::Vm,
    };

    let mut summary = Summary {
        label: kind.label(),
        total: 0,
        passed: 0,
        failures: Vec::new(),
        per_dir: BTreeMap::new(),
    };

    for (rel, abs) in cases {
        if should_skip(rel, target) {
            continue;
        }
        if let Some(f) = filter {
            if !rel.contains(f) {
                continue;
            }
        }
        let source = match std::fs::read_to_string(abs) {
            Ok(s) => s,
            Err(e) => {
                summary.total += 1;
                summary
                    .failures
                    .push((rel.clone(), vec![format!("read error: {e}")]));
                bucket(&mut summary.per_dir, rel, false);
                continue;
            }
        };
        let tagged = parse_directives::extract_tagged(&source);
        let expects = filter_directives(tagged, kind);
        let outcome = runner::run_case(binary, abs, &expects);
        summary.total += 1;
        if outcome.passed {
            summary.passed += 1;
            bucket(&mut summary.per_dir, rel, true);
            if verbose {
                println!("PASS {} ({})", rel, kind.label());
            }
        } else {
            bucket(&mut summary.per_dir, rel, false);
            if verbose {
                println!("FAIL {} ({})", rel, kind.label());
                for f in &outcome.failures {
                    println!("  {f}");
                }
            }
            summary.failures.push((rel.clone(), outcome.failures));
        }
    }

    Ok(summary)
}

fn bucket(map: &mut BTreeMap<String, (usize, usize)>, rel: &str, passed: bool) {
    let key = match rel.split_once('/') {
        Some((head, _)) => head.to_string(),
        None => "<root>".to_string(),
    };
    let entry = map.entry(key).or_insert((0, 0));
    if passed {
        entry.0 += 1;
    }
    entry.1 += 1;
}

fn print_summary(s: &Summary) {
    println!("{}:", s.label);
    let dir_label_width = s.per_dir.keys().map(|k| k.len()).max().unwrap_or(0).max(16);
    for (dir, (pass, total)) in &s.per_dir {
        println!(
            "  {:<width$}  {}/{}",
            format!("{}/", dir),
            pass,
            total,
            width = dir_label_width + 1
        );
    }
    println!(
        "  Summary: {}/{} passed ({:.1}%)",
        s.passed,
        s.total,
        s.pct()
    );
    println!();
}

fn main() -> ExitCode {
    let args = match parse_args() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("error: {e:?}");
            return ExitCode::from(2);
        }
    };

    let root = repo_root();
    let cases_dir = root.join("test-suite/cases");
    if !cases_dir.is_dir() {
        eprintln!(
            "error: could not find test-suite/cases (cwd={}, probed={})",
            std::env::current_dir().unwrap_or_default().display(),
            cases_dir.display()
        );
        return ExitCode::from(2);
    }
    let cases = match collect_cases(&cases_dir) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("error: failed to walk cases: {e:?}");
            return ExitCode::from(2);
        }
    };
    let binary_dir = if args.binary_dir.is_absolute() {
        args.binary_dir.clone()
    } else {
        root.join(&args.binary_dir)
    };

    let targets: Vec<TargetKind> = match args.target {
        Target::Tree => vec![TargetKind::Tree],
        Target::Vm => vec![TargetKind::Vm],
        Target::Both => vec![TargetKind::Tree, TargetKind::Vm],
    };

    let mut summaries = Vec::new();
    for t in &targets {
        let bin = binary_dir.join(t.binary_name());
        match run_target(*t, &bin, &cases, args.filter.as_deref(), args.verbose) {
            Ok(s) => {
                print_summary(&s);
                summaries.push(s);
            }
            Err(e) => {
                eprintln!("error: {e:?}");
                return ExitCode::from(2);
            }
        }
    }

    // Overall line + gate.
    print!("Overall:");
    for (i, s) in summaries.iter().enumerate() {
        if i > 0 {
            print!(" |");
        }
        print!(" {} {:.1}%", s.label, s.pct());
    }
    println!(" | threshold {:.1}%", PASS_THRESHOLD);

    let all_pass = summaries.iter().all(|s| s.pct() >= PASS_THRESHOLD);
    if all_pass {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

// --------------------------------------------------------------------------
// Tests
// --------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::parse_directives::{extract, extract_tagged, Expect};

    #[test]
    fn directives_stdout_expect_parsed() {
        let src = "var a = 1;\nvar b = 2;\nvar c = 3;\nvar d = 4;\nprint 1; // expect: hi\n";
        let got = extract(src);
        assert_eq!(got, vec![Expect::Stdout("hi".to_string())]);
    }

    #[test]
    fn directives_runtime_error_parsed() {
        let src = "1 + \"x\"; // expect runtime error: foo\n";
        let got = extract(src);
        assert_eq!(got, vec![Expect::RuntimeError("foo".to_string())]);
    }

    #[test]
    fn directives_error_inline_line_from_comment() {
        let src = "var a;\nvar a; // Error at 'a': msg\n";
        let got = extract(src);
        assert_eq!(
            got,
            vec![Expect::CompileError {
                line: 2,
                msg: "Error at 'a': msg".to_string(),
            }]
        );
    }

    #[test]
    fn directives_explicit_line_prefix_parsed() {
        let src = "// [line 7] Error x\n";
        let got = extract(src);
        assert_eq!(
            got,
            vec![Expect::CompileError {
                line: 7,
                msg: "Error x".to_string(),
            }]
        );
    }

    #[test]
    fn directives_multiple_stdout_in_order() {
        let src = "print 1; // expect: one\nprint 2; // expect: two\n";
        let got = extract(src);
        assert_eq!(
            got,
            vec![
                Expect::Stdout("one".to_string()),
                Expect::Stdout("two".to_string()),
            ]
        );
    }

    #[test]
    fn directives_ignores_bare_comments() {
        let src = "// random comment\nvar x = 1;\n";
        let got = extract(src);
        assert!(got.is_empty(), "expected no directives, got {got:?}");
    }

    #[test]
    fn directives_tagged_java_and_c() {
        let src = "// [line 3] Error: Unexpected character.\n\
                   // [java line 3] Error at 'b': Expect ')' after arguments.\n\
                   foo(a | b);\n\
                   // [c line 4] Error at end: Expect '}' after block.\n";
        let tagged = extract_tagged(src);
        let tags: Vec<Option<&'static str>> = tagged.iter().map(|(t, _)| *t).collect();
        assert_eq!(tags, vec![None, Some("java"), Some("c")]);
    }

    #[test]
    fn directives_runtime_error_plus_stdout() {
        // stdout expects still apply up to the point of the error.
        let src = "print 1; // expect: 1\n\
                   bad; // expect runtime error: Undefined variable 'bad'.\n";
        let got = extract(src);
        assert_eq!(
            got,
            vec![
                Expect::Stdout("1".to_string()),
                Expect::RuntimeError("Undefined variable 'bad'.".to_string()),
            ]
        );
    }

    #[test]
    fn directives_bare_error_uses_comment_line() {
        let src =
            "{\n  class Foo < Foo {} // Error at 'Foo': A class can't inherit from itself.\n}\n";
        let got = extract(src);
        assert_eq!(
            got,
            vec![Expect::CompileError {
                line: 2,
                msg: "Error at 'Foo': A class can't inherit from itself.".to_string(),
            }]
        );
    }
}
