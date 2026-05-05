//! Stack-based bytecode VM. Ports *Crafting Interpreters* chapters 15
//! (execution loop), 18–22 (types, globals, locals), 23 (jumps / control
//! flow), 24 (functions + calls), 25 (closures), and 27–29 (classes,
//! methods, inheritance, `this`/`super`).
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
//!   `>= frame.slots_base` by dropping the entry. The cell already is the
//!   shared storage while open, so closing must not overwrite it from a stale
//!   stack slot.
//! * Classes use `Rc<ObjClass>` + `Rc<ObjInstance>` + `Rc<ObjBoundMethod>`
//!   as the heap representation. Methods are plain `Rc<Closure>` entries in
//!   `ObjClass.methods`. `OP_INHERIT` copies the parent's methods into the
//!   child so method resolution stays a single hash-map probe. (M6-GC: the
//!   follow-up milestone replaces these `Rc`s with GC-managed raw pointers.)

use std::cell::RefCell;
use std::collections::HashMap;
use std::io::{self, Write};
use std::rc::Rc;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::chunk::OpCode;
use crate::compiler::compile;
use crate::value::{
    Closure, NativeFn, NativeFunction, ObjBoundMethod, ObjClass, ObjInstance, UpvalueCell, Value,
};

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
    /// Pre-interned `"init"` string used to short-circuit `OP_CALL` on a
    /// class value (book chapter 28). M6-GC: replaces with raw ptr into the
    /// GC-managed string table.
    init_string: Rc<String>,
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
            init_string: Rc::new("init".to_string()),
            output: Box::new(io::stdout()),
        };
        vm.define_native("clock", 0, clock_native);
        vm
    }

    /// Redirect printed output to `w`. Useful for tests and REPL hosting.
    pub fn set_output(&mut self, w: Box<dyn Write>) {
        self.output = w;
    }

    fn define_native(&mut self, name: &str, arity: usize, function: NativeFn) {
        let name = Rc::new(name.to_string());
        self.globals.insert(
            name.clone(),
            Value::Native(Rc::new(NativeFunction {
                name,
                arity,
                function,
            })),
        );
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
                OpCode::Class => {
                    let name = match self.read_constant() {
                        Value::Str(s) => s,
                        _ => {
                            self.runtime_error("Class name constant is not a string.");
                            return InterpretResult::RuntimeError;
                        }
                    };
                    self.stack.push(Value::Class(Rc::new(ObjClass::new(name))));
                }
                OpCode::Inherit => {
                    // Stack shape: [.., superclass, subclass (TOS)]. Book
                    // chapter 29.
                    let n = self.stack.len();
                    if n < 2 {
                        self.runtime_error("Internal: OP_INHERIT underflow.");
                        return InterpretResult::RuntimeError;
                    }
                    let superclass = match &self.stack[n - 2] {
                        Value::Class(c) => c.clone(),
                        _ => {
                            self.runtime_error("Superclass must be a class.");
                            return InterpretResult::RuntimeError;
                        }
                    };
                    let subclass = match &self.stack[n - 1] {
                        Value::Class(c) => c.clone(),
                        _ => {
                            self.runtime_error("Internal: OP_INHERIT expects a class.");
                            return InterpretResult::RuntimeError;
                        }
                    };
                    // Copy all entries — the child inherits the parent's
                    // method table so future `find_method` calls don't have
                    // to walk a chain. Per-method overrides happen as later
                    // OP_METHOD instructions write to the same table.
                    {
                        let src = superclass.methods.borrow();
                        let mut dst = subclass.methods.borrow_mut();
                        for (k, v) in src.iter() {
                            dst.insert(k.clone(), v.clone());
                        }
                    }
                    // Leave the superclass on the stack (the compiler stored
                    // it in a synthetic local named `"super"` earlier), pop
                    // the subclass.
                    self.stack.pop();
                }
                OpCode::Method => {
                    let name = match self.read_constant() {
                        Value::Str(s) => s,
                        _ => {
                            self.runtime_error("Method name constant is not a string.");
                            return InterpretResult::RuntimeError;
                        }
                    };
                    let n = self.stack.len();
                    if n < 2 {
                        self.runtime_error("Internal: OP_METHOD underflow.");
                        return InterpretResult::RuntimeError;
                    }
                    let closure = match &self.stack[n - 1] {
                        Value::Closure(c) => c.clone(),
                        _ => {
                            self.runtime_error("Internal: OP_METHOD expects a closure.");
                            return InterpretResult::RuntimeError;
                        }
                    };
                    let class = match &self.stack[n - 2] {
                        Value::Class(c) => c.clone(),
                        _ => {
                            self.runtime_error("Internal: OP_METHOD expects a class below.");
                            return InterpretResult::RuntimeError;
                        }
                    };
                    class.methods.borrow_mut().insert(name, closure);
                    // Pop only the closure — the class stays on the stack
                    // for the next OP_METHOD (or the trailing OP_POP).
                    self.stack.pop();
                }
                OpCode::GetProperty => {
                    let name = match self.read_constant() {
                        Value::Str(s) => s,
                        _ => {
                            self.runtime_error("Property name constant is not a string.");
                            return InterpretResult::RuntimeError;
                        }
                    };
                    let instance = match self.stack.last() {
                        Some(Value::Instance(i)) => i.clone(),
                        _ => {
                            self.runtime_error("Only instances have properties.");
                            return InterpretResult::RuntimeError;
                        }
                    };
                    // Field lookup first — shadows methods (book semantics).
                    let field = instance.fields.borrow().get(&name).cloned();
                    if let Some(v) = field {
                        self.stack.pop();
                        self.stack.push(v);
                    } else if let Some(method) = instance.class.find_method(&name) {
                        // Bind the method to the receiver.
                        let receiver = self.stack.pop().unwrap_or(Value::Nil);
                        let bm = Rc::new(ObjBoundMethod { receiver, method });
                        self.stack.push(Value::BoundMethod(bm));
                    } else {
                        self.runtime_error(&format!("Undefined property '{}'.", name));
                        return InterpretResult::RuntimeError;
                    }
                }
                OpCode::SetProperty => {
                    let name = match self.read_constant() {
                        Value::Str(s) => s,
                        _ => {
                            self.runtime_error("Property name constant is not a string.");
                            return InterpretResult::RuntimeError;
                        }
                    };
                    let n = self.stack.len();
                    if n < 2 {
                        self.runtime_error("Internal: OP_SET_PROPERTY underflow.");
                        return InterpretResult::RuntimeError;
                    }
                    let instance = match &self.stack[n - 2] {
                        Value::Instance(i) => i.clone(),
                        _ => {
                            self.runtime_error("Only instances have fields.");
                            return InterpretResult::RuntimeError;
                        }
                    };
                    let value = self.stack.pop().unwrap_or(Value::Nil);
                    instance.fields.borrow_mut().insert(name, value.clone());
                    // Pop the instance, leave the assigned value as the
                    // expression's result (book behaviour).
                    self.stack.pop();
                    self.stack.push(value);
                }
                OpCode::GetSuper => {
                    // Stack: [.., this, superclass (TOS)]. Book ch. 29.
                    let name = match self.read_constant() {
                        Value::Str(s) => s,
                        _ => {
                            self.runtime_error("Super method name constant is not a string.");
                            return InterpretResult::RuntimeError;
                        }
                    };
                    let superclass = match self.stack.pop() {
                        Some(Value::Class(c)) => c,
                        _ => {
                            self.runtime_error("Internal: OP_GET_SUPER expects a class.");
                            return InterpretResult::RuntimeError;
                        }
                    };
                    let receiver = self.stack.pop().unwrap_or(Value::Nil);
                    match superclass.find_method(&name) {
                        Some(method) => {
                            let bm = Rc::new(ObjBoundMethod { receiver, method });
                            self.stack.push(Value::BoundMethod(bm));
                        }
                        None => {
                            self.runtime_error(&format!("Undefined property '{}'.", name));
                            return InterpretResult::RuntimeError;
                        }
                    }
                }
                OpCode::Invoke => {
                    let name = match self.read_constant() {
                        Value::Str(s) => s,
                        _ => {
                            self.runtime_error("Method name constant is not a string.");
                            return InterpretResult::RuntimeError;
                        }
                    };
                    let argc = self.read_byte() as usize;
                    if !self.invoke(&name, argc) {
                        return InterpretResult::RuntimeError;
                    }
                }
                OpCode::SuperInvoke => {
                    // Stack layout right before this opcode (compiler-side):
                    //   [.., receiver (this), arg1, ..., argN, superclass (TOS)].
                    // OP_SUPER_INVOKE pops the superclass, looks up `name` on
                    // it, and calls the resulting closure with `receiver` +
                    // the argN-deep argument window.
                    let name = match self.read_constant() {
                        Value::Str(s) => s,
                        _ => {
                            self.runtime_error("Super method name constant is not a string.");
                            return InterpretResult::RuntimeError;
                        }
                    };
                    let argc = self.read_byte() as usize;
                    let superclass = match self.stack.pop() {
                        Some(Value::Class(c)) => c,
                        _ => {
                            self.runtime_error("Internal: OP_SUPER_INVOKE expects a class.");
                            return InterpretResult::RuntimeError;
                        }
                    };
                    match superclass.find_method(&name) {
                        Some(method) => {
                            if !self.call_closure(method, argc) {
                                return InterpretResult::RuntimeError;
                            }
                        }
                        None => {
                            self.runtime_error(&format!("Undefined property '{}'.", name));
                            return InterpretResult::RuntimeError;
                        }
                    }
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

    /// Close every upvalue at absolute index `>= last_kept`.
    fn close_upvalues(&mut self, last_kept: usize) {
        let mut i = 0;
        while i < self.open_upvalues.len() {
            let (idx, _) = &self.open_upvalues[i];
            if *idx >= last_kept {
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
            Value::Native(native) => {
                if argc != native.arity {
                    self.runtime_error(&format!(
                        "Expected {} arguments but got {}.",
                        native.arity, argc
                    ));
                    return false;
                }
                let start = self.stack.len() - argc;
                let result = (native.function)(&self.stack[start..]);
                // Pop args + callee; push result.
                self.stack.truncate(callee_idx);
                self.stack.push(result);
                true
            }
            Value::Class(class) => {
                // Constructor call. Slot the new instance where the class
                // was so `this` lands in slot 0 of the init frame (or of
                // the caller's result slot when there's no init).
                let instance = Rc::new(ObjInstance::new(class.clone()));
                self.stack[callee_idx] = Value::Instance(instance);

                if let Some(initializer) = class.find_method(&self.init_string) {
                    self.call_closure(initializer, argc)
                } else if argc != 0 {
                    self.runtime_error(&format!("Expected 0 arguments but got {}.", argc));
                    false
                } else {
                    true
                }
            }
            Value::BoundMethod(bm) => {
                // Replace the bound-method slot with the receiver so the
                // method's slot 0 holds `this` when the frame starts.
                self.stack[callee_idx] = bm.receiver.clone();
                self.call_closure(bm.method.clone(), argc)
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

    /// Book chapter 28's `invoke` helper: fast path for `receiver.method(...)`.
    /// If the receiver has a field named `name`, fall back to calling the
    /// field (user may have stashed a closure there). Otherwise look up
    /// `name` in the class method table and invoke it without materialising
    /// a bound method.
    fn invoke(&mut self, name: &Rc<String>, argc: usize) -> bool {
        let receiver_idx = self.stack.len() - argc - 1;
        let receiver = self.stack[receiver_idx].clone();
        let instance = match receiver {
            Value::Instance(i) => i,
            _ => {
                self.runtime_error("Only instances have methods.");
                return false;
            }
        };

        // Field takes precedence over methods — if the user stored a
        // callable in a field, `inst.field(x)` must invoke it.
        if let Some(field) = instance.fields.borrow().get(name).cloned() {
            self.stack[receiver_idx] = field;
            return self.call_value(argc);
        }

        match instance.class.find_method(name) {
            Some(method) => self.call_closure(method, argc),
            None => {
                self.runtime_error(&format!("Undefined property '{}'.", name));
                false
            }
        }
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
