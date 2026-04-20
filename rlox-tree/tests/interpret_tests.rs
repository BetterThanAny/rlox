//! Integration tests for the tree-walking interpreter.
//!
//! Each test drives source through the full scanner → parser → resolver →
//! interpreter pipeline and asserts either on captured print output or on
//! the returned error. `print` output is captured by swapping the
//! interpreter's output writer for a shared `Vec<u8>` before running.

use std::cell::RefCell;
use std::io::{self, Write};
use std::rc::Rc;

use rlox_tree::error::LoxError;
use rlox_tree::interpreter::Interpreter;
use rlox_tree::parser::Parser;
use rlox_tree::resolver::Resolver;
use rlox_tree::scanner::Scanner;

// ---------- helpers ----------

/// A shared buffer we can both hand to `set_output` (boxed as `Box<dyn Write>`)
/// and read back in the test via the outer `Rc`.
#[derive(Clone, Default)]
struct SharedBuf(Rc<RefCell<Vec<u8>>>);

impl Write for SharedBuf {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.borrow_mut().extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// Run `src` end-to-end, returning captured stdout on success or the first
/// error encountered otherwise.
fn run_capture(src: &str) -> Result<String, String> {
    let tokens = Scanner::new(src)
        .scan_tokens()
        .map_err(|e| format!("SCAN: {e}"))?;
    let stmts = Parser::new(tokens)
        .parse()
        .map_err(|errs| format!("PARSE: {}", render(&errs)))?;
    let locals = Resolver::new()
        .resolve(&stmts)
        .map_err(|errs| format!("RESOLVE: {}", render(&errs)))?;

    let buf = SharedBuf::default();
    let mut interp = Interpreter::new();
    interp.set_output(Box::new(buf.clone()));
    interp.install_locals(locals);
    interp
        .interpret(&stmts)
        .map_err(|e| format!("RUNTIME: {e}"))?;
    let bytes = buf.0.borrow().clone();
    Ok(String::from_utf8(bytes).expect("print output is utf-8"))
}

fn render(errs: &[LoxError]) -> String {
    errs.iter()
        .map(|e| e.to_string())
        .collect::<Vec<_>>()
        .join("\n")
}

// ---------- tests ----------

#[test]
fn arithmetic_and_precedence() {
    let out = run_capture("print 1 + 2 * 3;").unwrap();
    assert_eq!(out, "7\n");
}

#[test]
fn string_concatenation() {
    let out = run_capture(r#"print "foo" + "bar";"#).unwrap();
    assert_eq!(out, "foobar\n");
}

#[test]
fn logical_short_circuit_or_returns_left_if_truthy() {
    // Only the left side prints because `or` short-circuits on truthy.
    let out = run_capture(
        r#"
        fun side() { print "side"; return true; }
        print 1 or side();
        "#,
    )
    .unwrap();
    // `1 or side()` must NOT call `side`; output is just `1`.
    assert_eq!(out, "1\n");
}

#[test]
fn logical_short_circuit_and_returns_left_if_falsey() {
    let out = run_capture(
        r#"
        fun side() { print "side"; return true; }
        print false and side();
        "#,
    )
    .unwrap();
    assert_eq!(out, "false\n");
}

#[test]
fn block_scope_shadows_outer() {
    let out = run_capture(
        r#"
        var a = "outer";
        {
          var a = "inner";
          print a;
        }
        print a;
        "#,
    )
    .unwrap();
    assert_eq!(out, "inner\nouter\n");
}

#[test]
fn closure_counter_matches_book_example() {
    // Straight from ch. 10 §10.4. Bare-minimum upvalue capture.
    let out = run_capture(
        r#"
        fun makeCounter() {
          var n = 0;
          fun count() {
            n = n + 1;
            return n;
          }
          return count;
        }
        var c = makeCounter();
        print c();
        print c();
        print c();
        "#,
    )
    .unwrap();
    assert_eq!(out, "1\n2\n3\n");
}

#[test]
fn recursive_function_fibonacci() {
    let out = run_capture(
        r#"
        fun fib(n) {
          if (n < 2) return n;
          return fib(n - 2) + fib(n - 1);
        }
        print fib(10);
        "#,
    )
    .unwrap();
    assert_eq!(out, "55\n");
}

#[test]
fn class_with_method_call_binds_this() {
    let out = run_capture(
        r#"
        class Greeter {
          init(name) { this.name = name; }
          greet() { print "Hi, " + this.name; }
        }
        var g = Greeter("Ada");
        g.greet();
        "#,
    )
    .unwrap();
    assert_eq!(out, "Hi, Ada\n");
}

#[test]
fn inheritance_super_call() {
    // Book ch. 13 super.method() pattern.
    let out = run_capture(
        r#"
        class A {
          speak() { print "A"; }
        }
        class B < A {
          speak() { super.speak(); print "B"; }
        }
        B().speak();
        "#,
    )
    .unwrap();
    assert_eq!(out, "A\nB\n");
}

#[test]
fn return_value_in_initializer_is_resolve_error() {
    // Resolver forbids returning a value from init(); this surfaces as a
    // resolve error, not a runtime one.
    let err = run_capture(
        r#"
        class A {
          init() { return 42; }
        }
        "#,
    )
    .unwrap_err();
    assert!(err.starts_with("RESOLVE: "), "got: {err}");
    assert!(err.contains("Can't return a value from an initializer."));
}

#[test]
fn init_auto_returns_this_even_with_bare_return() {
    // `init` with a bare `return;` must still produce the instance.
    let out = run_capture(
        r#"
        class Box {
          init() { return; }
          tag() { print "boxed"; }
        }
        var b = Box();
        b.tag();
        "#,
    )
    .unwrap();
    assert_eq!(out, "boxed\n");
}

#[test]
fn native_clock_is_callable_and_returns_number() {
    // We can't hardcode the value, but we can assert arithmetic works on it.
    let out = run_capture(
        r#"
        var t = clock();
        print t > 0;
        "#,
    )
    .unwrap();
    assert_eq!(out, "true\n");
}

#[test]
fn resolver_depth_captures_outer_var_not_global_shadow() {
    // Book ch. 11.5 — the resolver ensures `a` in `showA()` binds to the
    // outer local `a = "global"`, not the later global redeclaration.
    let out = run_capture(
        r#"
        var a = "global";
        {
          fun showA() { print a; }
          showA();
          var a = "block";
          showA();
        }
        "#,
    )
    .unwrap();
    // Both calls must print "global".
    assert_eq!(out, "global\nglobal\n");
}

#[test]
fn runtime_error_on_undefined_variable() {
    let err = run_capture("print x;").unwrap_err();
    assert!(err.starts_with("RUNTIME: "), "got: {err}");
    assert!(err.contains("Undefined variable 'x'."));
}

#[test]
fn property_access_and_set() {
    let out = run_capture(
        r#"
        class Bag {}
        var b = Bag();
        b.item = "apple";
        print b.item;
        "#,
    )
    .unwrap();
    assert_eq!(out, "apple\n");
}

#[test]
fn for_loop_prints_expected_sequence() {
    let out = run_capture(
        r#"
        for (var i = 0; i < 3; i = i + 1) {
          print i;
        }
        "#,
    )
    .unwrap();
    assert_eq!(out, "0\n1\n2\n");
}

#[test]
fn runtime_error_operands_must_be_numbers() {
    let err = run_capture(r#"print "a" - 1;"#).unwrap_err();
    assert!(err.starts_with("RUNTIME: "), "got: {err}");
    assert!(err.contains("Operands must be numbers."));
}

#[test]
fn runtime_error_plus_mismatched_types() {
    let err = run_capture(r#"print 1 + "a";"#).unwrap_err();
    assert!(err.starts_with("RUNTIME: "), "got: {err}");
    assert!(err.contains("Operands must be two numbers or two strings."));
}

#[test]
fn calling_non_callable_errors() {
    let err = run_capture("var x = 1; x();").unwrap_err();
    assert!(err.starts_with("RUNTIME: "), "got: {err}");
    assert!(err.contains("Can only call functions and classes."));
}
