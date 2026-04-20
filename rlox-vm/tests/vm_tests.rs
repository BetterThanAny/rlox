//! End-to-end integration tests for the bytecode VM. Each scenario stitches
//! together several language features (arithmetic + variables + control flow
//! + functions/closures) and asserts the printed output byte-for-byte.

use std::io::{self, Write};
use std::sync::{Arc, Mutex};

use rlox_vm::vm::{InterpretResult, Vm};

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

fn run(src: &str) -> (InterpretResult, String) {
    let buf = Arc::new(Mutex::new(Vec::<u8>::new()));
    let mut vm = Vm::new();
    vm.set_output(Box::new(SharedBuf(buf.clone())));
    let res = vm.interpret(src);
    let captured = String::from_utf8(buf.lock().unwrap().clone()).unwrap();
    (res, captured)
}

#[test]
fn vm_integration_arithmetic_with_variables_and_print() {
    let src = "\
        var a = 1; \
        var b = 2; \
        var c = 3; \
        print a + b * c;";
    let (res, out) = run(src);
    assert_eq!(res, InterpretResult::Ok);
    assert_eq!(out, "7\n");
}

#[test]
fn vm_integration_while_accumulates_into_global() {
    let src = "\
        var sum = 0; \
        var i = 1; \
        while (i <= 5) { sum = sum + i; i = i + 1; } \
        print sum;";
    let (res, out) = run(src);
    assert_eq!(res, InterpretResult::Ok);
    assert_eq!(out, "15\n");
}

#[test]
fn vm_integration_for_loop_with_function_body() {
    let src = "\
        fun square(n) { return n * n; } \
        for (var i = 1; i <= 3; i = i + 1) print square(i);";
    let (res, out) = run(src);
    assert_eq!(res, InterpretResult::Ok);
    assert_eq!(out, "1\n4\n9\n");
}

#[test]
fn vm_integration_recursive_fib_matches_book() {
    let src = "\
        fun fib(n) { \
          if (n < 2) return n; \
          return fib(n - 2) + fib(n - 1); \
        } \
        print fib(10);";
    let (res, out) = run(src);
    assert_eq!(res, InterpretResult::Ok);
    assert_eq!(out, "55\n");
}

#[test]
fn vm_integration_closure_counter_persists_across_returns() {
    let src = "\
        fun makeCounter() { \
          var n = 0; \
          fun counter() { n = n + 1; return n; } \
          return counter; \
        } \
        var c = makeCounter(); \
        print c(); print c(); print c();";
    let (res, out) = run(src);
    assert_eq!(res, InterpretResult::Ok);
    assert_eq!(out, "1\n2\n3\n");
}

#[test]
fn vm_integration_nested_if_with_string_concat_and_locals() {
    let src = "\
        fun greet(name) { \
          var prefix = \"Hello, \"; \
          if (name == \"world\") return prefix + name + \"!\"; \
          return prefix + \"stranger.\"; \
        } \
        print greet(\"world\"); \
        print greet(\"claude\");";
    let (res, out) = run(src);
    assert_eq!(res, InterpretResult::Ok);
    assert_eq!(out, "Hello, world!\nHello, stranger.\n");
}

#[test]
fn vm_integration_two_counters_are_independent() {
    // Each call to makeCounter should allocate its own upvalue cell.
    let src = "\
        fun makeCounter() { \
          var n = 0; \
          fun counter() { n = n + 1; return n; } \
          return counter; \
        } \
        var a = makeCounter(); \
        var b = makeCounter(); \
        print a(); print a(); print b(); print a();";
    let (res, out) = run(src);
    assert_eq!(res, InterpretResult::Ok);
    assert_eq!(out, "1\n2\n1\n3\n");
}

#[test]
fn vm_integration_runtime_error_in_nested_call_reports() {
    // 'a' - 1 triggers a runtime error inside `inner`.
    let src = "\
        fun inner() { return \"a\" - 1; } \
        fun outer() { return inner(); } \
        print outer();";
    let (res, out) = run(src);
    assert_eq!(res, InterpretResult::RuntimeError);
    // Output goes to stderr; stdout should be empty.
    assert_eq!(out, "");
}

// ---------- Milestone 6 (classes) ----------

#[test]
fn vm_integration_class_declaration_and_instantiation() {
    let src = "\
        class A {} \
        print A; \
        var a = A(); \
        print a;";
    let (res, out) = run(src);
    assert_eq!(res, InterpretResult::Ok);
    assert_eq!(out, "A\nA instance\n");
}

#[test]
fn vm_integration_instance_field_set_and_get() {
    let src = "\
        class Box {} \
        var b = Box(); \
        b.x = 42; \
        print b.x;";
    let (res, out) = run(src);
    assert_eq!(res, InterpretResult::Ok);
    assert_eq!(out, "42\n");
}

#[test]
fn vm_integration_method_call() {
    let src = "\
        class Greeter { \
          hello() { print \"hi\"; } \
        } \
        Greeter().hello();";
    let (res, out) = run(src);
    assert_eq!(res, InterpretResult::Ok);
    assert_eq!(out, "hi\n");
}

#[test]
fn vm_integration_this_in_method() {
    let src = "\
        class Named { \
          init(n) { this.n = n; } \
          name() { return this.n; } \
        } \
        print Named(\"rlox\").name();";
    let (res, out) = run(src);
    assert_eq!(res, InterpretResult::Ok);
    assert_eq!(out, "rlox\n");
}

#[test]
fn vm_integration_init_receives_args_and_binds_fields() {
    let src = "\
        class Point { \
          init(x, y) { this.x = x; this.y = y; } \
        } \
        var p = Point(3, 4); \
        print p.x; print p.y;";
    let (res, out) = run(src);
    assert_eq!(res, InterpretResult::Ok);
    assert_eq!(out, "3\n4\n");
}

#[test]
fn vm_integration_inheritance_inherits_methods() {
    let src = "\
        class A { \
          foo() { print \"A.foo\"; } \
        } \
        class B < A {} \
        B().foo();";
    let (res, out) = run(src);
    assert_eq!(res, InterpretResult::Ok);
    assert_eq!(out, "A.foo\n");
}

#[test]
fn vm_integration_super_method_call() {
    let src = "\
        class A { foo() { print \"A\"; } } \
        class B < A { foo() { super.foo(); print \"B\"; } } \
        B().foo();";
    let (res, out) = run(src);
    assert_eq!(res, InterpretResult::Ok);
    assert_eq!(out, "A\nB\n");
}

#[test]
fn vm_integration_property_not_found_runtime_error() {
    let src = "\
        class A {} \
        var a = A(); \
        print a.missing;";
    let (res, out) = run(src);
    assert_eq!(res, InterpretResult::RuntimeError);
    // Error goes to stderr; stdout stays empty.
    assert_eq!(out, "");
}

#[test]
fn vm_integration_examples_class_lox() {
    let src = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../examples/class.lox",
    ))
    .expect("examples/class.lox must be readable");
    let (res, out) = run(&src);
    assert_eq!(res, InterpretResult::Ok);
    assert_eq!(out, "Rex says woof!\nRex makes a sound.\n");
}
