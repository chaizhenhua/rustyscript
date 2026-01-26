//! Integration tests for WebPermissions system
//!
//! These tests verify that user-supplied WebPermissions are correctly
//! connected to the deno permission system and actually restrict operations.

use rustyscript::{Module, Runtime, RuntimeOptions};
use std::sync::Arc;
use std::time::Duration;

#[cfg(feature = "web")]
use rustyscript::{
    AllowlistWebPermissions, DefaultWebPermissions, ExtensionOptions, WebOptions,
    to_permissions_options,
};

/// Test that AllowlistWebPermissions with no allowed hosts blocks fetch
#[test]
#[cfg(feature = "web")]
fn test_allowlist_blocks_fetch() {
    let permissions = AllowlistWebPermissions::new();
    // Don't allow any hosts - everything should be blocked

    let mut runtime = Runtime::new(RuntimeOptions {
        timeout: Duration::from_secs(10),
        extension_options: ExtensionOptions {
            web: WebOptions {
                permissions: Arc::new(permissions),
                ..Default::default()
            },
            ..Default::default()
        },
        ..Default::default()
    })
    .expect("Failed to create runtime");

    let module = Module::new(
        "test_blocked_fetch.js",
        r#"
        export async function test() {
            try {
                await fetch('https://example.com');
                return { blocked: false, error: null };
            } catch (e) {
                return { blocked: true, error: e.message };
            }
        }
        "#,
    );

    let handle = runtime.load_module(&module).expect("Failed to load module");
    let result: rustyscript::serde_json::Value = runtime
        .call_function(Some(&handle), "test", rustyscript::json_args!())
        .expect("Failed to call function");

    assert!(
        result["blocked"].as_bool().unwrap_or(false),
        "Fetch should have been blocked by permissions. Got: {:?}",
        result
    );

    // Verify the error message mentions permission or network restriction
    let error_msg = result["error"].as_str().unwrap_or("");
    assert!(
        error_msg.to_lowercase().contains("permission")
            || error_msg.to_lowercase().contains("denied")
            || error_msg.to_lowercase().contains("not allowed")
            || error_msg.to_lowercase().contains("network")
            || error_msg.contains("Requires net access")  // deno_permissions message
            || error_msg.contains("--allow-net"),         // deno flag suggestion
        "Error should mention permission denial. Got: {}",
        error_msg
    );
}

/// Test that DefaultWebPermissions (via default RuntimeOptions) allows fetch
#[test]
#[cfg(feature = "web")]
fn test_default_permissions_allow_fetch() {
    // Use default options which should use DefaultWebPermissions (allow all)
    let mut runtime = Runtime::new(RuntimeOptions {
        timeout: Duration::from_secs(30),
        ..Default::default()
    })
    .expect("Failed to create runtime");

    let module = Module::new(
        "test_allowed_fetch.js",
        r#"
        export async function test() {
            try {
                // Use a reliable test endpoint
                const response = await fetch('https://httpbin.org/get');
                return { success: response.ok, status: response.status };
            } catch (e) {
                return { success: false, error: e.message };
            }
        }
        "#,
    );

    let handle = runtime.load_module(&module).expect("Failed to load module");
    let result: rustyscript::serde_json::Value = runtime
        .call_function(Some(&handle), "test", rustyscript::json_args!())
        .expect("Failed to call function");

    assert!(
        result["success"].as_bool().unwrap_or(false),
        "Fetch should succeed with default permissions. Got: {:?}",
        result
    );
}

/// Test that to_permissions_options correctly detects DefaultWebPermissions as allow-all
#[test]
#[cfg(feature = "web")]
fn test_to_permissions_options_default() {
    let default_perms = DefaultWebPermissions;
    let opts = to_permissions_options(&default_perms);

    // DefaultWebPermissions should result in allow-all options
    assert!(
        opts.allow_read.is_some(),
        "allow_read should be Some for DefaultWebPermissions"
    );
    assert!(
        opts.allow_write.is_some(),
        "allow_write should be Some for DefaultWebPermissions"
    );
    assert!(
        opts.allow_net.is_some(),
        "allow_net should be Some for DefaultWebPermissions"
    );
    assert!(
        opts.allow_env.is_some(),
        "allow_env should be Some for DefaultWebPermissions"
    );
}

/// Test that to_permissions_options correctly detects AllowlistWebPermissions as restrictive
#[test]
#[cfg(feature = "web")]
fn test_to_permissions_options_allowlist() {
    let allowlist_perms = AllowlistWebPermissions::new();
    // Don't allow anything
    let opts = to_permissions_options(&allowlist_perms);

    // AllowlistWebPermissions with no allowances should result in deny-all options
    assert!(
        opts.allow_read.is_none(),
        "allow_read should be None for empty AllowlistWebPermissions"
    );
    assert!(
        opts.allow_write.is_none(),
        "allow_write should be None for empty AllowlistWebPermissions"
    );
    assert!(
        opts.allow_net.is_none(),
        "allow_net should be None for empty AllowlistWebPermissions"
    );
    assert!(
        opts.allow_env.is_none(),
        "allow_env should be None for empty AllowlistWebPermissions"
    );

    // Import should still be allowed for module loading
    assert!(
        opts.allow_import.is_some(),
        "allow_import should be Some even for restrictive permissions"
    );
}
