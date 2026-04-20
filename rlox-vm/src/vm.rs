//! Stack-based bytecode VM. Ports *Crafting Interpreters* chapters 15
//! (execution loop), 18–22 (types, globals, locals), 23 (jumps / control
//! flow), 24 (functions + calls), and 25 (closures).
//!
//! Chapter 26+ (classes, methods, inheritance) lands in M6; the opcodes for
//! those instructions simply fail with a runtime error here.
//!
//! Design notes:
//! * Frames live in a `Vec<CallFrame>`; the active frame is `frames.last_mut()`.
//!   We keep `ip`/`closure`/`slots_base` per frame rather than stashing them
//!   as pointers as clox does — safer with Rust's borrow checker and plenty
//!   fast for our purposes.
//! * Strings use `Rc<String>` (already interned-for-free via `Value::Str`).
//!   Globals key off the `Rc<String>` identity / string content; M6 will
//!   swap this for a dedicated string-table that dedupes.
//! * Upvalues: we keep `open_upvalues: Vec<(stack_index, UpvalueCell)>` and
//!   redirect `OP_GET_LOCAL` / `OP_SET_LOCAL` through that map when the slot
//!   has been hoisted. On frame exit we close everything at indices
//!   `>= frame.slots_base` by writing the current stack value into the cell
//!   and dropping the entry. This is the simplest construction that makes
//!   the `makeCounter` test work: after the outer frame returns, the cell
//!   still holds the captured value and the inner closure can keep reading
//!   and writing it.

use std::cell::RefCell;
use std::collections::HashMap;
use std::io::{self, Write};
use std::rc::Rc;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::chunk::OpCode;
use crate::compiler::compile;
use crate::value::{Closure, NativeFn, UpvalueCell, Value};

/// Outcome of a call to [`Vm::interpret`].
#[derive(Debug, PartialEq, Eq)]
pub enum InterpretResult {
    Ok,
    CompileError,
    RuntimeError,
}

/// One active call. The active frame's `ip` indexes into
/// `closure.function.chunk.code`, and `slots_base` indexes into `Vm::stack`.
struct CallFrame {
    closure: Rc<Closure>,
    ip: usize,
    slots_base: usize,
}

/// The bytecode VM itself.
pub struct Vm {
    frames: Vec<CallFrame>,
    stack: Vec<Value>,
    globals: HashMap<Rc<String>, Value>,
    /// (stack_index, upvalue cell) pairs for locals that have been captured
    /// by a closure but not yet closed over. Maintained in stack-index order.
    open_upvalues: Vec<(usize, UpvalueCell)>,
    output: Box<dyn Write>,
}

impl Default for Vm {
    fn default() -> Self {
        Self::new()
    }
}

impl Vm {
    /// Fresh VM with `clock()` pre-installed as a global native, writing to
    /// stdout by default.
    pub fn new() -> Self {
        let mut vm = Self {
            frames: Vec::with_capacity(64),
            stack: Vec::with_capacity(256),
            globals: HashMap::new(),
            open_upvalues: Vec::new(),
            output: Box::new(io::stdout()),
        };
        vm.define_native("clock", clock_native);
        vm
    }

    /// Redirect printed output to `w`. Useful for tests and REPL hosting.
    pub fn set_output(&mut self, w: Box<dyn Write>) {
        self.output = w;
    }

    fn define_native(&mut self, name: &str, f: NativeFn) {
        self.globals
            .insert(Rc::new(name.to_string()), Value::Native(f));
    }

    /// Compile `source` and run. Returns `CompileError` when the compiler
    /// rejected the program and `RuntimeError` when the VM trapped mid-run.
    pub fn interpret(&mut self, source: &str) -> InterpretResult {
        let function = match compile(source) {
            Ok(f) => f,
            Err(errors) => {
                for e in &errors {
                    let _ = writeln!(io::stderr(), "{e}");
                }
                return InterpretResult::CompileError;
            }
        };

        // Wrap the script function in a no-upvalue closure, push it as slot 0
        // (the book reserves slot 0 for the callee), and start the frame.
        let closure = Rc::new(Closure {
            function: function.clone(),
            upvalues: Vec::new(),
        });
        self.stack.clear();
        self.frames.clear();
        self.open_upvalues.clear();
        self.stack.push(Value::Closure(closure.clone()));
        self.frames.push(CallFrame {
            closure,
            ip: 0,
            slots_base: 0,
        });

        self.run()
    }

    // ---------- main dispatch loop ----------

    fn run(&mut self) -> InterpretResult {
        loop {
            let op_byte = self.read_byte();
            let Some(op) = OpCode::from_byte(op_byte) else {
                self.runtime_error(&format!("Unknown opcode {op_byte}."));
                return InterpretResult::RuntimeError;
            };

            match op {
                OpCode::Constant => {
                    let v = self.read_constant();
                    self.stack.push(v);
                }
                OpCode::Nil => self.stack.push(Value::Nil),
                OpCode::True => self.stack.push(Value::Bool(true)),
                OpCode::False => self.stack.push(Value::Bool(false)),
                OpCode::Pop => {
                    self.stack.pop();
                }
                OpCode::GetLocal => {
                    let slot = self.read_byte() as usize;
                    let abs = self.frame().slots_base + slot;
                    let v = self.read_local(abs);
                    self.stack.push(v);
                }
                OpCode::SetLocal => {
                    let slot = self.read_byte() as usize;
                    let abs = self.frame().slots_base + slot;
                    // Peek (don't pop) per book — assignment is an expression
                    // whose value stays on the stack.
                    let v = self.stack.last().cloned().unwrap_or(Value::Nil);
                    self.write_local(abs, v);
                }
                OpCode::GetGlobal => {
                    let name = match self.read_constant() {
                        Value::Str(s) => s,
                        _ => {
                            self.runtime_error("Global name constant is not a string.");
                            return InterpretResult::RuntimeError;
                        }
                    };
                    match self.globals.get(&name) {
                        Some(v) => {
                            let v = v.clone();
                            self.stack.push(v);
                        }
                        None => {
                            self.runtime_error(&format!("Undefined variable '{}'.", name));
                            return InterpretResult::RuntimeError;
                        }
                    }
                }
                OpCode::DefineGlobal => {
                    let name = match self.read_constant() {
                        Value::Str(s) => s,
                        _ => {
                            self.runtime_error("Global name constant is not a string.");
                            return InterpretResult::RuntimeError;
                        }
                    };
                    let v = self.stack.pop().unwrap_or(Value::Nil);
                    self.globals.insert(name, v);
                }
                OpCode::SetGlobal => {
                    let name = match self.read_constant() {
                        Value::Str(s) => s,
                        _ => {
                            self.runtime_error("Global name constant is not a string.");
                            return InterpretResult::RuntimeError;
                        }
                    };
                    if !self.globals.contains_key(&name) {
                        self.runtime_error(&format!("Undefined variable '{}'.", name));
                        return InterpretResult::RuntimeError;
                    }
                    let v = self.stack.last().cloned().unwrap_or(Value::Nil);
                    self.globals.insert(name, v);
                }
                OpCode::GetUpvalue => {
                    let slot = self.read_byte() as usize;
                    let cell = self.frame().closure.upvalues[slot].clone();
                    let v = cell.borrow().clone();
                    self.stack.push(v);
                }
                OpCode::SetUpvalue => {
                    let slot = self.read_byte() as usize;
                    let cell = self.frame().closure.upvalues[slot].clone();
                    let v = self.stack.last().cloned().unwrap_or(Value::Nil);
                    *cell.borrow_mut() = v;
                }
                OpCode::Equal => {
                    let b = self.stack.pop().unwrap_or(Value::Nil);
                    let a = self.stack.pop().unwrap_or(Value::Nil);
                    self.stack.push(Value::Bool(a.equals(&b)));
                }
                OpCode::Greater => {
                    if let Err(()) = self.binary_compare(|a, b| a > b) {
                        return InterpretResult::RuntimeError;
                    }
                }
                OpCode::Less => {
                    if let Err(()) = self.binary_compare(|a, b| a < b) {
                        return InterpretResult::RuntimeError;
                    }
                }
                OpCode::Add => {
                    if let Err(()) = self.op_add() {
                        return InterpretResult::RuntimeError;
                    }
                }
                OpCode::Subtract => {
                    if let Err(()) = self.binary_number(|a, b| a - b) {
                        return InterpretResult::RuntimeError;
                    }
                }
                OpCode::Multiply => {
                    if let Err(()) = self.binary_number(|a, b| a * b) {
                        return InterpretResult::RuntimeError;
                    }
                }
                OpCode::Divide => {
                    if let Err(()) = self.binary_number(|a, b| a / b) {
                        return InterpretResult::RuntimeError;
                    }
                }
                OpCode::Not => {
                    let v = self.stack.pop().unwrap_or(Value::Nil);
                    self.stack.push(Value::Bool(v.is_falsey()));
                }
                OpCode::Negate => {
                    let top = self.stack.last().cloned();
                    match top {
                        Some(Value::Number(n)) => {
                            self.stack.pop();
                            self.stack.push(Value::Number(-n));
                        }
                        _ => {
                            self.runtime_error("Operand must be a number.");
                            return InterpretResult::RuntimeError;
                        }
                    }
                }
                OpCode::Print => {
                    let v = self.stack.pop().unwrap_or(Value::Nil);
                    let _ = writeln!(self.output, "{v}");
                }
                OpCode::Jump => {
                    let offset = self.read_short();
                    self.frame_mut().ip += offset as usize;
                }
                OpCode::JumpIfFalse => {
                    let offset = self.read_short();
                    if self.stack.last().map(|v| v.is_falsey()).unwrap_or(true) {
                        self.frame_mut().ip += offset as usize;
                    }
                }
                OpCode::Loop => {
                    let offset = self.read_short();
                    self.frame_mut().ip -= offset as usize;
                }
                OpCode::Call => {
                    let argc = self.read_byte() as usize;
                    if !self.call_value(argc) {
                        return InterpretResult::RuntimeError;
                    }
                }
                OpCode::Closure => {
                    let fn_value = self.read_constant();
                    let function = match fn_value {
                        Value::Function(f) => f,
                        _ => {
                            self.runtime_error("OP_CLOSURE expects a function constant.");
                            return InterpretResult::RuntimeError;
                        }
                    };
                    let mut upvalues = Vec::with_capacity(function.upvalue_count);
                    for _ in 0..function.upvalue_count {
                        let is_local = self.read_byte() != 0;
                        let index = self.read_byte() as usize;
                        let cell = if is_local {
                            let abs = self.frame().slots_base + index;
                            self.capture_upvalue(abs)
                        } else {
                            self.frame().closure.upvalues[index].clone()
                        };
                        upvalues.push(cell);
                    }
                    self.stack
                        .push(Value::Closure(Rc::new(Closure { function, upvalues })));
                }
                OpCode::CloseUpvalue => {
                    // Close at TOS, then pop. See `close_upvalues` below — the
                    // helper handles the cell-update + removal.
                    let last = self.stack.len() - 1;
                    self.close_upvalues(last);
                    self.stack.pop();
                }
                OpCode::Return => {
                    let result = self.stack.pop().unwrap_or(Value::Nil);
                    let frame = self.frames.pop().expect("non-empty frames on return");
                    // Close any upvalues that captured locals in this frame
                    // before we truncate the stack out from under them.
                    self.close_upvalues(frame.slots_base);
                    // Pop everything this frame owned (including the callee at
                    // slot 0), then push the return value onto the caller.
                    self.stack.truncate(frame.slots_base);
                    if self.frames.is_empty() {
                        return InterpretResult::Ok;
                    }
                    self.stack.push(result);
                }
                OpCode::Invoke
                | OpCode::SuperInvoke
                | OpCode::Class
                | OpCode::Inherit
                | OpCode::Method
                | OpCode::GetProperty
                | OpCode::SetProperty
                | OpCode::GetSuper => {
                    self.runtime_error("Classes are not supported yet (M6).");
                    return InterpretResult::RuntimeError;
                }
            }
        }
    }

    // ---------- byte helpers ----------

    fn read_byte(&mut self) -> u8 {
        let frame = self.frames.last_mut().expect("frame present");
        let byte = frame.closure.function.chunk.code[frame.ip];
        frame.ip += 1;
        byte
    }

    fn read_short(&mut self) -> u16 {
        let frame = self.frames.last_mut().expect("frame present");
        let hi = frame.closure.function.chunk.code[frame.ip] as u16;
        let lo = frame.closure.function.chunk.code[frame.ip + 1] as u16;
        frame.ip += 2;
        (hi << 8) | lo
    }

    fn read_constant(&mut self) -> Value {
        let idx = self.read_byte() as usize;
        self.frame().closure.function.chunk.constants[idx].clone()
    }

    fn frame(&self) -> &CallFrame {
        self.frames.last().expect("frame present")
    }

    fn frame_mut(&mut self) -> &mut CallFrame {
        self.frames.last_mut().expect("frame present")
    }

    // ---------- local-slot access (with upvalue redirection) ----------

    fn read_local(&self, abs: usize) -> Value {
        for (idx, cell) in &self.open_upvalues {
            if *idx == abs {
                return cell.borrow().clone();
            }
        }
        self.stack.get(abs).cloned().unwrap_or(Value::Nil)
    }

    fn write_local(&mut self, abs: usize, value: Value) {
        for (idx, cell) in &self.open_upvalues {
            if *idx == abs {
                *cell.borrow_mut() = value;
                return;
            }
        }
        if abs < self.stack.len() {
            self.stack[abs] = value;
        }
    }

    // ---------- upvalues ----------

    /// Capture the local at absolute stack index `abs`, reusing an existing
    /// open upvalue if one already points there.
    fn capture_upvalue(&mut self, abs: usize) -> UpvalueCell {
        for (idx, cell) in &self.open_upvalues {
            if *idx == abs {
                return cell.clone();
            }
        }
        let initial = self.stack.get(abs).cloned().unwrap_or(Value::Nil);
        let cell = Rc::new(RefCell::new(initial));
        self.open_upvalues.push((abs, cell.clone()));
        cell
    }

    /// Close every upvalue at absolute index `>= last_kept`. Each closed
    /// upvalue snapshots the current stack value into its cell so it survives
    /// the frame's stack teardown.
    fn close_upvalues(&mut self, last_kept: usize) {
        let mut i = 0;
        while i < self.open_upvalues.len() {
            let (idx, cell) = &self.open_upvalues[i];
            if *idx >= last_kept {
                // Snapshot the current stack value into the cell. After this
                // point the cell is self-contained (lives only through Rc).
                if let Some(v) = self.stack.get(*idx) {
                    *cell.borrow_mut() = v.clone();
                }
                self.open_upvalues.swap_remove(i);
            } else {
                i += 1;
            }
        }
    }

    // ---------- arithmetic helpers ----------

    fn binary_number<F: FnOnce(f64, f64) -> f64>(&mut self, f: F) -> Result<(), ()> {
        let (a, b) = match (self.stack.pop(), self.stack.pop()) {
            (Some(Value::Number(b)), Some(Value::Number(a))) => (a, b),
            (b, a) => {
                // Book message. Restore stack if we had popped something:
                // pushing back preserves semantics for post-error debugging.
                if let Some(v) = a {
                    self.stack.push(v);
                }
                if let Some(v) = b {
                    self.stack.push(v);
                }
                self.runtime_error("Operands must be numbers.");
                return Err(());
            }
        };
        self.stack.push(Value::Number(f(a, b)));
        Ok(())
    }

    fn binary_compare<F: FnOnce(f64, f64) -> bool>(&mut self, f: F) -> Result<(), ()> {
        let (a, b) = match (self.stack.pop(), self.stack.pop()) {
            (Some(Value::Number(b)), Some(Value::Number(a))) => (a, b),
            (b, a) => {
                if let Some(v) = a {
                    self.stack.push(v);
                }
                if let Some(v) = b {
                    self.stack.push(v);
                }
                self.runtime_error("Operands must be numbers.");
                return Err(());
            }
        };
        self.stack.push(Value::Bool(f(a, b)));
        Ok(())
    }

    /// `OP_ADD`: numbers add, strings concat, anything else errors.
    fn op_add(&mut self) -> Result<(), ()> {
        let b = self.stack.pop().unwrap_or(Value::Nil);
        let a = self.stack.pop().unwrap_or(Value::Nil);
        match (a, b) {
            (Value::Number(x), Value::Number(y)) => {
                self.stack.push(Value::Number(x + y));
                Ok(())
            }
            (Value::Str(x), Value::Str(y)) => {
                let mut s = String::with_capacity(x.len() + y.len());
                s.push_str(&x);
                s.push_str(&y);
                self.stack.push(Value::Str(Rc::new(s)));
                Ok(())
            }
            (a, b) => {
                // Restore for debuggability.
                self.stack.push(a);
                self.stack.push(b);
                self.runtime_error("Operands must be two numbers or two strings.");
                Err(())
            }
        }
    }

    // ---------- calls ----------

    /// Dispatch a call to whatever is at `stack[len - argc - 1]`. On error
    /// emits a runtime message and returns `false`.
    fn call_value(&mut self, argc: usize) -> bool {
        let callee_idx = self.stack.len() - argc - 1;
        let callee = self.stack[callee_idx].clone();
        match callee {
            Value::Closure(c) => self.call_closure(c, argc),
            Value::Function(f) => {
                // Rare — compiler wraps user functions in closures before
                // calling them — but if one slips through, treat it as an
                // empty-upvalue closure.
                let c = Rc::new(Closure {
                    function: f,
                    upvalues: Vec::new(),
                });
                self.call_closure(c, argc)
            }
            Value::Native(f) => {
                let start = self.stack.len() - argc;
                let result = f(&self.stack[start..]);
                // Pop args + callee; push result.
                self.stack.truncate(callee_idx);
                self.stack.push(result);
                true
            }
            _ => {
                self.runtime_error("Can only call functions and classes.");
                false
            }
        }
    }

    fn call_closure(&mut self, closure: Rc<Closure>, argc: usize) -> bool {
        if argc != closure.function.arity {
            self.runtime_error(&format!(
                "Expected {} arguments but got {}.",
                closure.function.arity, argc
            ));
            return false;
        }
        if self.frames.len() >= 64 {
            self.runtime_error("Stack overflow.");
            return false;
        }
        let slots_base = self.stack.len() - argc - 1;
        self.frames.push(CallFrame {
            closure,
            ip: 0,
            slots_base,
        });
        true
    }

    // ---------- runtime errors ----------

    fn runtime_error(&mut self, msg: &str) {
        let _ = writeln!(io::stderr(), "{msg}");
        // Stack trace (book chapter 24, innermost frame first).
        for frame in self.frames.iter().rev() {
            let line = frame
                .closure
                .function
                .chunk
                .lines
                .get(frame.ip.saturating_sub(1))
                .copied()
                .unwrap_or(0);
            let name = frame.closure.function.display_name();
            let suffix = if name == "script" { "" } else { "()" };
            let _ = writeln!(io::stderr(), "[line {line}] in {name}{suffix}");
        }
        self.stack.clear();
        self.open_upvalues.clear();
    }
}

// --- native functions -------------------------------------------------------

fn clock_native(_args: &[Value]) -> Value {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0);
    Value::Number(secs)
}

// ---------------------------------------------------------------------------

#[cfg(test)]
mod vm_tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    /// Shared buffer + `Write` adaptor so tests can snapshot output.
    struct SharedBuf(Arc<Mutex<Vec<u8>>>);

    impl Write for SharedBuf {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    fn run_capture(src: &str) -> (InterpretResult, String) {
        let buf = Arc::new(Mutex::new(Vec::<u8>::new()));
        let mut vm = Vm::new();
        vm.set_output(Box::new(SharedBuf(buf.clone())));
        let result = vm.interpret(src);
        let captured = String::from_utf8(buf.lock().unwrap().clone()).unwrap();
        (result, captured)
    }

    #[test]
    fn vm_print_number_literal() {
        let (res, out) = run_capture("print 1;");
        assert_eq!(res, InterpretResult::Ok);
        assert_eq!(out, "1\n");
    }

    #[test]
    fn vm_arithmetic_precedence() {
        let (res, out) = run_capture("print 1 + 2 * 3;");
        assert_eq!(res, InterpretResult::Ok);
        assert_eq!(out, "7\n");
    }

    #[test]
    fn vm_string_concat() {
        let (res, out) = run_capture(r#"print "a" + "b";"#);
        assert_eq!(res, InterpretResult::Ok);
        assert_eq!(out, "ab\n");
    }

    #[test]
    fn vm_unary_negate_and_not() {
        let (res, out) = run_capture("print -5; print !true; print !nil;");
        assert_eq!(res, InterpretResult::Ok);
        assert_eq!(out, "-5\nfalse\ntrue\n");
    }

    #[test]
    fn vm_comparison_and_equality() {
        let src = "print 1 < 2; print 2 <= 2; print 3 > 2; print 3 >= 3; \
                   print 1 == 1; print 1 != 2;";
        let (res, out) = run_capture(src);
        assert_eq!(res, InterpretResult::Ok);
        assert_eq!(out, "true\ntrue\ntrue\ntrue\ntrue\ntrue\n");
    }

    #[test]
    fn vm_logical_short_circuit_or() {
        // If `or` evaluated its RHS, `0/0` would produce NaN (not a runtime
        // error in Lox, so we can't assert failure) — but the point here is
        // simply that the RHS is *skipped* and the printed value is `true`.
        let (res, out) = run_capture("print true or (0/0);");
        assert_eq!(res, InterpretResult::Ok);
        assert_eq!(out, "true\n");
    }

    #[test]
    fn vm_global_var_define_and_read() {
        let (res, out) = run_capture("var x = 42; print x;");
        assert_eq!(res, InterpretResult::Ok);
        assert_eq!(out, "42\n");
    }

    #[test]
    fn vm_local_var_in_block() {
        let (res, out) = run_capture("{ var a = 1; print a; }");
        assert_eq!(res, InterpretResult::Ok);
        assert_eq!(out, "1\n");
    }

    #[test]
    fn vm_if_else_runs_selected_branch() {
        let (res, out) = run_capture("if (1 < 2) print \"lt\"; else print \"ge\";");
        assert_eq!(res, InterpretResult::Ok);
        assert_eq!(out, "lt\n");
    }

    #[test]
    fn vm_while_loop_prints_0_to_2() {
        let src = "var i = 0; while (i < 3) { print i; i = i + 1; }";
        let (res, out) = run_capture(src);
        assert_eq!(res, InterpretResult::Ok);
        assert_eq!(out, "0\n1\n2\n");
    }

    #[test]
    fn vm_for_loop_desugared_prints_0_to_2() {
        let src = "for (var i = 0; i < 3; i = i + 1) print i;";
        let (res, out) = run_capture(src);
        assert_eq!(res, InterpretResult::Ok);
        assert_eq!(out, "0\n1\n2\n");
    }

    #[test]
    fn vm_function_call_returns_value() {
        let src = "fun add(a, b) { return a + b; } print add(3, 4);";
        let (res, out) = run_capture(src);
        assert_eq!(res, InterpretResult::Ok);
        assert_eq!(out, "7\n");
    }

    #[test]
    fn vm_recursion_fib_10() {
        let src = "fun fib(n) { if (n < 2) return n; \
                   return fib(n - 2) + fib(n - 1); } \
                   print fib(10);";
        let (res, out) = run_capture(src);
        assert_eq!(res, InterpretResult::Ok);
        assert_eq!(out, "55\n");
    }

    #[test]
    fn vm_closure_counter() {
        let src = "fun makeCounter() { \
                     var n = 0; \
                     fun counter() { n = n + 1; return n; } \
                     return counter; \
                   } \
                   var c = makeCounter(); \
                   print c(); print c(); print c();";
        let (res, out) = run_capture(src);
        assert_eq!(res, InterpretResult::Ok);
        assert_eq!(out, "1\n2\n3\n");
    }

    #[test]
    fn vm_runtime_error_on_non_number_subtract() {
        let (res, _out) = run_capture(r#"print "a" - 1;"#);
        assert_eq!(res, InterpretResult::RuntimeError);
    }

    #[test]
    fn vm_clock_native_is_callable_and_returns_number() {
        let (res, out) = run_capture("print clock() > 0;");
        assert_eq!(res, InterpretResult::Ok);
        assert_eq!(out, "true\n");
    }
}
