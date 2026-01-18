// This example tests for the BorrowMutError bug in deno_core 0.376.0
//
// Issue: When executing JavaScript that throws a ReferenceError,
// deno_core panics with "already borrowed: BorrowMutError"
// instead of properly returning the error.
//
// Expected behavior: The error should be returned as Err(...)
// Actual behavior (in buggy versions): Process panics/aborts

use rustyscript::{Module, Runtime, RuntimeOptions};
use std::time::Duration;

fn main() {
    println!("=== Testing for deno_core BorrowMutError bug ===\n");

    // Create a basic Runtime with minimal configuration
    let options = RuntimeOptions {
        timeout: Duration::from_secs(5),
        ..Default::default()
    };

    let mut runtime = Runtime::new(options).expect("Failed to create runtime");

    // Test 1: ReferenceError from accessing undefined variable
    println!("Test 1: ReferenceError from accessing undefined variable");
    test_error_handling(
        &mut runtime,
        r#"
            // This should throw a ReferenceError
            undefinedVariable.toString();
        "#,
        "test_undefined_variable.js",
    );

    // Test 2: TypeError from calling non-function
    println!("\nTest 2: TypeError from calling non-function");
    test_error_handling(
        &mut runtime,
        r#"
            // This should throw a TypeError
            const notAFunction = "I am not a function";
            notAFunction();
        "#,
        "test_type_error.js",
    );

    // Test 3: Syntax error
    println!("\nTest 3: Syntax error");
    test_error_handling(
        &mut runtime,
        r#"
            // This should throw a SyntaxError
            const x = ;
        "#,
        "test_syntax_error.js",
    );

    // Test 4: Range error
    println!("\nTest 4: Range error");
    test_error_handling(
        &mut runtime,
        r#"
            // This should throw a RangeError
            function recursiveFunction() {
                recursiveFunction();
            }
            recursiveFunction();
        "#,
        "test_range_error.js",
    );

    println!("\n=== All tests passed! No BorrowMutError detected ===");
}

fn test_error_handling(runtime: &mut Runtime, code: &str, module_name: &str) {
    let module = Module::new(module_name, code);

    print!("  Loading module '{}' (expected to fail)... ", module_name);
    let result = runtime.load_module(&module);

    match result {
        Ok(_) => {
            println!("❌ UNEXPECTED: Module loaded successfully (should have failed)");
            std::process::exit(1);
        }
        Err(e) => {
            println!(
                "✓ Correctly returned error: {}",
                e.to_string().lines().next().unwrap_or("Unknown error")
            );
        }
    }
}
