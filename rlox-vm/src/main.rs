//! rlox-vm binary: REPL + file runner.
//!
//! Usage: `rlox-vm [script]`
//!   - No args: enter the rustyline REPL; each line is fed through the full
//!     compile+VM pipeline against a persistent `Vm` (so `var x = 1;` then
//!     `print x;` on two separate prompts works). `.exit` quits.
//!   - One arg: treat as a script path; run to completion. Exits 65 on a
//!     compile error and 70 on a runtime error.
//!   - Two-or-more args: print usage and exit 64.

use std::env;
use std::fs;
use std::process;

use anyhow::Result;
use rustyline::error::ReadlineError;
use rustyline::DefaultEditor;

use rlox_vm::vm::{InterpretResult, Vm};

fn main() -> Result<()> {
    let args: Vec<String> = env::args().skip(1).collect();
    match args.as_slice() {
        [] => repl(),
        [script] => run_file(script),
        _ => {
            eprintln!("Usage: rlox-vm [script]");
            process::exit(64);
        }
    }
}

fn run_file(path: &str) -> Result<()> {
    let src = fs::read_to_string(path)?;
    let mut vm = Vm::new();
    match vm.interpret(&src) {
        InterpretResult::Ok => Ok(()),
        InterpretResult::CompileError => process::exit(65),
        InterpretResult::RuntimeError => process::exit(70),
    }
}

fn repl() -> Result<()> {
    let mut rl = DefaultEditor::new()?;
    let mut vm = Vm::new();
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
                let _ = vm.interpret(&line);
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
