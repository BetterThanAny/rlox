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
    // Scan even on failure so the parser can still report follow-up errors
    // (jlox-style: compiler halts only once every stage has emitted its
    // errors). We merge scan + parse errors into a single `Syntax` payload.
    let (tokens, scan_errors) = Scanner::new(src).scan_tokens_and_errors();
    let parse_result = Parser::new(tokens).parse();
    let stmts = match (scan_errors.is_empty(), parse_result) {
        (true, Ok(s)) => s,
        (true, Err(errs)) => return Err(RunError::Syntax(errs)),
        (false, Ok(_)) => return Err(RunError::Syntax(scan_errors)),
        (false, Err(mut parse_errs)) => {
            let mut all = scan_errors;
            all.append(&mut parse_errs);
            return Err(RunError::Syntax(all));
        }
    };

    let locals = Resolver::new().resolve(&stmts).map_err(RunError::Resolve)?;
    interp.install_locals(locals);

    interp.interpret(&stmts).map_err(RunError::Runtime)
}
