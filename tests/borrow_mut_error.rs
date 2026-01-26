//! Regression tests for the BorrowMutError bug in deno_core 0.376.0
//!
//! Issue: When executing JavaScript that throws errors (ReferenceError, TypeError, etc.),
//! deno_core should return the error properly instead of panicking with
//! "already borrowed: BorrowMutError".
//!
//! These tests verify that various JavaScript errors are correctly returned as Err(...)
//! rather than causing a panic.

use rustyscript::{Module, Runtime, RuntimeOptions};
use std::time::Duration;

fn create_runtime() -> Runtime {
    let options = RuntimeOptions {
        timeout: Duration::from_secs(5),
        ..Default::default()
    };
    Runtime::new(options).expect("Failed to create runtime")
}

/// Test that ReferenceError from accessing undefined variable is properly returned
#[test]
fn test_reference_error_handling() {
    let mut runtime = create_runtime();

    let module = Module::new(
        "test_undefined_variable.js",
        r#"
            // This should throw a ReferenceError
            undefinedVariable.toString();
        "#,
    );

    let result = runtime.load_module(&module);
    assert!(
        result.is_err(),
        "Expected ReferenceError but module loaded successfully"
    );

    let error_msg = result.unwrap_err().to_string();
    assert!(
        error_msg.contains("ReferenceError") || error_msg.contains("undefinedVariable"),
        "Expected ReferenceError message, got: {}",
        error_msg
    );
}

/// Test that TypeError from calling non-function is properly returned
#[test]
fn test_type_error_handling() {
    let mut runtime = create_runtime();

    let module = Module::new(
        "test_type_error.js",
        r#"
            // This should throw a TypeError
            const notAFunction = "I am not a function";
            notAFunction();
        "#,
    );

    let result = runtime.load_module(&module);
    assert!(
        result.is_err(),
        "Expected TypeError but module loaded successfully"
    );

    let error_msg = result.unwrap_err().to_string();
    assert!(
        error_msg.contains("TypeError") || error_msg.contains("not a function"),
        "Expected TypeError message, got: {}",
        error_msg
    );
}

/// Test that SyntaxError is properly returned
#[test]
fn test_syntax_error_handling() {
    let mut runtime = create_runtime();

    let module = Module::new(
        "test_syntax_error.js",
        r#"
            // This should throw a SyntaxError
            const x = ;
        "#,
    );

    let result = runtime.load_module(&module);
    assert!(
        result.is_err(),
        "Expected SyntaxError but module loaded successfully"
    );

    let error_msg = result.unwrap_err().to_string();
    assert!(
        error_msg.contains("SyntaxError") || error_msg.contains("Unexpected token"),
        "Expected SyntaxError message, got: {}",
        error_msg
    );
}

/// Test that RangeError (stack overflow) is properly returned
#[test]
fn test_range_error_handling() {
    let mut runtime = create_runtime();

    let module = Module::new(
        "test_range_error.js",
        r#"
            // This should throw a RangeError (stack overflow)
            function recursiveFunction() {
                recursiveFunction();
            }
            recursiveFunction();
        "#,
    );

    let result = runtime.load_module(&module);
    assert!(
        result.is_err(),
        "Expected RangeError but module loaded successfully"
    );

    let error_msg = result.unwrap_err().to_string();
    assert!(
        error_msg.contains("RangeError")
            || error_msg.contains("stack")
            || error_msg.contains("recursion"),
        "Expected RangeError/stack overflow message, got: {}",
        error_msg
    );
}

/// Test that multiple errors in sequence don't cause BorrowMutError
#[test]
fn test_multiple_errors_in_sequence() {
    let mut runtime = create_runtime();

    // First error
    let module1 = Module::new("error1.js", "undefinedVar1.foo();");
    assert!(runtime.load_module(&module1).is_err());

    // Second error - should not panic with BorrowMutError
    let module2 = Module::new("error2.js", "undefinedVar2.bar();");
    assert!(runtime.load_module(&module2).is_err());

    // Third error
    let module3 = Module::new("error3.js", "const x = ;");
    assert!(runtime.load_module(&module3).is_err());

    // Runtime should still be usable after errors
    let valid_module = Module::new("valid.js", "export const value = 42;");
    assert!(
        runtime.load_module(&valid_module).is_ok(),
        "Runtime should still work after handling errors"
    );
}
