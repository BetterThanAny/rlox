//! rlox-tree binary: REPL + file runner.
//!
//! Usage: `rlox-tree [script]`
//!   - No args: enter the rustyline REPL; each line is scanned, parsed,
//!     resolved, and evaluated independently. `.exit` quits.
//!   - One arg: treat as a script path; run to completion. Exits 65 on a
//!     syntax/resolve error and 70 on a runtime error.
//!   - Two-or-more args: print usage and exit 64.

use std::env;
use std::fmt;
use std::fs;
use std::process;

use anyhow::Result;
use rustyline::error::ReadlineError;
use rustyline::DefaultEditor;

use rlox_tree::error::LoxError;
use rlox_tree::interpreter::Interpreter;
use rlox_tree::parser::Parser;
use rlox_tree::resolver::Resolver;
use rlox_tree::scanner::Scanner;

/// Aggregated failure out of `run` so `main` can decide the exit code.
enum RunError {
    Syntax(Vec<LoxError>),
    Resolve(Vec<LoxError>),
    Runtime(LoxError),
}

impl fmt::Display for RunError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RunError::Syntax(errs) | RunError::Resolve(errs) => {
                for (i, e) in errs.iter().enumerate() {
                    if i > 0 {
                        writeln!(f)?;
                    }
                    write!(f, "{e}")?;
                }
                Ok(())
            }
            RunError::Runtime(e) => write!(f, "{e}"),
        }
    }
}

fn main() -> Result<()> {
    let args: Vec<String> = env::args().skip(1).collect();
    match args.as_slice() {
        [] => run_prompt(),
        [script] => run_file(script),
        _ => {
            eprintln!("Usage: rlox-tree [script]");
            process::exit(64);
        }
    }
}

fn run_file(path: &str) -> Result<()> {
    let src = fs::read_to_string(path)?;
    let mut interp = Interpreter::new();
    match run(&mut interp, &src) {
        Ok(()) => Ok(()),
        Err(e) => {
            eprintln!("{e}");
            match e {
                RunError::Syntax(_) | RunError::Resolve(_) => process::exit(65),
                RunError::Runtime(_) => process::exit(70),
            }
        }
    }
}

fn run_prompt() -> Result<()> {
    let mut rl = DefaultEditor::new()?;
    let mut interp = Interpreter::new();
    loop {
        match rl.readline("> ") {
            Ok(line) => {
                let trimmed = line.trim();
                if trimmed == ".exit" {
                    break;
                }
                if trimmed.is_empty() {
                    continue;
                }
                rl.add_history_entry(line.as_str()).ok();
                if let Err(e) = run(&mut interp, &line) {
                    eprintln!("{e}");
                }
            }
            Err(ReadlineError::Interrupted) | Err(ReadlineError::Eof) => break,
            Err(err) => {
                eprintln!("readline error: {err}");
                break;
            }
        }
    }
    Ok(())
}

/// Run one chunk of source through the full pipeline. The interpreter state
/// (globals, local side-table) persists across calls, which is what the REPL
/// wants.
fn run(interp: &mut Interpreter, src: &str) -> std::result::Result<(), RunError> {
    // Scanner returns `Result<_, String>`; the string is of the form
    // `[line N] Error: ...`. Extract the line; fall back to 0.
    let tokens = Scanner::new(src).scan_tokens().map_err(|msg| {
        let line = parse_line_prefix(&msg).unwrap_or(0);
        let stripped = strip_line_prefix(&msg);
        RunError::Syntax(vec![LoxError::syntax(line, "", stripped)])
    })?;

    let mut parser = Parser::new(tokens);
    let stmts = parser.parse().map_err(RunError::Syntax)?;

    let locals = Resolver::new().resolve(&stmts).map_err(RunError::Resolve)?;
    interp.install_locals(locals);

    interp.interpret(&stmts).map_err(RunError::Runtime)
}

/// Extract the `N` from a `"[line N] ..."` prefix, if present.
fn parse_line_prefix(s: &str) -> Option<usize> {
    let rest = s.strip_prefix("[line ")?;
    let end = rest.find(']')?;
    rest[..end].parse().ok()
}

/// Strip the `"[line N] Error: "` prefix from a scanner error message so the
/// rewrapped `LoxError::Syntax` doesn't double up the framing when it prints.
fn strip_line_prefix(s: &str) -> String {
    if let Some(rest) = s.strip_prefix("[line ") {
        if let Some(end) = rest.find("] ") {
            let after = &rest[end + 2..];
            // Book scanner emits `Error: ...`; drop the leading tag too.
            let after = after.strip_prefix("Error: ").unwrap_or(after);
            return after.to_string();
        }
    }
    s.to_string()
}
