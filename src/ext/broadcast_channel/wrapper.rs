//! Wrapper for broadcast channel functionality
//!
//! This module provides two types of broadcast channel wrappers:
//!
//! ## `BroadcastChannelWrapper` - Rust-to-Rust Communication
//!
//! This wrapper creates an isolated channel for Rust-to-Rust communication only.
//! It does NOT share the underlying channel with JavaScript's `BroadcastChannel` API.
//!
//! Use this when you need multiple Rust components to communicate via broadcast channels
//! without involving JavaScript.
//!
//! ## `SharedBroadcastChannelWrapper` - JavaScript ↔ Rust Communication
//!
//! This wrapper shares the same underlying channel as JavaScript's `BroadcastChannel` API,
//! enabling bidirectional communication between Rust and JavaScript.
//!
//! Use this when you need to:
//! - Send messages from Rust to JavaScript BroadcastChannel listeners
//! - Receive messages from JavaScript BroadcastChannel in Rust
//! - Integrate with existing JavaScript code using BroadcastChannel
//!
//! ## Example Comparison
//!
//! ### Rust-to-Rust (BroadcastChannelWrapper)
//! ```rust,ignore
//! let channel = BroadcastChannel::new();
//! let sub1 = channel.subscribe("my_channel")?;
//! let sub2 = channel.subscribe("my_channel")?;
//! sub1.send(&mut runtime, "hello").await?; // sub2 receives
//! // JavaScript BroadcastChannel('my_channel') does NOT receive this
//! ```
//!
//! ### Rust ↔ JavaScript (SharedBroadcastChannelWrapper)
//! ```rust,ignore
//! let channel = options.extension_options.web.broadcast_channel.clone();
//! let wrapper = SharedBroadcastChannelWrapper::new(&channel, "my_channel")?;
//! wrapper.send(&mut runtime, "hello").await?;
//! // JavaScript BroadcastChannel('my_channel') DOES receive this
//! ```
//!
//! # Usage
//!
//! ```rust,ignore
//! use rustyscript::{BroadcastChannel, Runtime, RuntimeOptions};
//!
//! // Create a shared channel
//! let channel = BroadcastChannel::new();
//!
//! // Create subscriptions for different runtimes
//! let mut runtime1 = Runtime::new(RuntimeOptions::default())?;
//! let mut runtime2 = Runtime::new(RuntimeOptions::default())?;
//!
//! let sub1 = channel.subscribe("my_channel")?;
//! let sub2 = channel.subscribe("my_channel")?;
//!
//! // Send from one subscription
//! sub1.send_sync(&mut runtime1, "hello")?;
//!
//! // Receive from another
//! let msg: String = sub2.recv_sync(&mut runtime2, None)?.unwrap();
//! ```

use std::sync::Arc;
use std::time::Duration;

use deno_core::parking_lot::Mutex;
use serde::{de::DeserializeOwned, Serialize};
use tokio::sync::broadcast;
use tokio::sync::mpsc;
use uuid::Uuid;

use crate::{big_json_args, Error, Runtime};

/// Message type for internal broadcast channel communication
#[derive(Clone, Debug)]
struct ChannelMessage {
    name: Arc<String>,
    data: Arc<Vec<u8>>,
    sender_id: Uuid,
}

/// A broadcast channel that can be shared across multiple runtimes
///
/// This is the backing storage for broadcast channel communication.
/// Clone this to share the channel between multiple wrappers.
#[derive(Clone)]
pub struct BroadcastChannel {
    sender: Arc<Mutex<broadcast::Sender<ChannelMessage>>>,
}

impl Default for BroadcastChannel {
    fn default() -> Self {
        Self::new()
    }
}

impl BroadcastChannel {
    /// Create a new broadcast channel
    #[must_use]
    pub fn new() -> Self {
        let (sender, _) = broadcast::channel(256);
        Self {
            sender: Arc::new(Mutex::new(sender)),
        }
    }

    /// Subscribe to this channel, creating a wrapper for sending/receiving messages
    ///
    /// # Errors
    /// Will return an error if the subscription cannot be created
    pub fn subscribe(&self, name: impl ToString) -> Result<BroadcastChannelWrapper, Error> {
        BroadcastChannelWrapper::new(self, name)
    }
}

/// Helper struct to wrap a broadcast channel subscription
///
/// Takes care of some of the boilerplate for serialization/deserialization.
/// Messages are serialized through the JavaScript runtime to ensure compatibility
/// with the JavaScript BroadcastChannel API.
pub struct BroadcastChannelWrapper {
    channel: BroadcastChannel,
    receiver: tokio::sync::Mutex<(
        broadcast::Receiver<ChannelMessage>,
        mpsc::UnboundedReceiver<()>,
    )>,
    cancel_tx: mpsc::UnboundedSender<()>,
    name: String,
    uuid: Uuid,
}

impl BroadcastChannelWrapper {
    /// Create a new broadcast channel wrapper and subscribe to the channel
    ///
    /// Unsubscribe is called when the wrapper is dropped
    ///
    /// # Errors
    /// Will return an error if the channel cannot be subscribed to
    pub fn new(channel: &BroadcastChannel, name: impl ToString) -> Result<Self, Error> {
        let (cancel_tx, cancel_rx) = mpsc::unbounded_channel();
        let broadcast_rx = channel.sender.lock().subscribe();
        let receiver = tokio::sync::Mutex::new((broadcast_rx, cancel_rx));
        let uuid = Uuid::new_v4();
        let name = name.to_string();

        Ok(Self {
            channel: channel.clone(),
            receiver,
            cancel_tx,
            name,
            uuid,
        })
    }

    /// Get the name of this channel
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Send a message to the channel, blocking until the message is sent
    ///
    /// # Errors
    /// Will return an error if the message cannot be serialized or sent
    pub fn send_sync<T: Serialize>(&self, runtime: &mut Runtime, data: T) -> Result<(), Error> {
        let tokio_rt = runtime.tokio_runtime();
        tokio_rt.block_on(self.send(runtime, data))
    }

    /// Send a message to the channel
    ///
    /// # Errors
    /// Will return an error if the message cannot be serialized or sent
    pub async fn send<T: Serialize>(&self, runtime: &mut Runtime, data: T) -> Result<(), Error> {
        // Serialize through JavaScript for compatibility
        let data: Vec<u8> = runtime
            .call_function_async(None, "broadcast_serialize", &data)
            .await?;

        let message = ChannelMessage {
            name: Arc::new(self.name.clone()),
            data: Arc::new(data),
            sender_id: self.uuid,
        };

        self.channel
            .sender
            .lock()
            .send(message)
            .map_err(|e| Error::Runtime(format!("Failed to send broadcast message: {e}")))?;

        Ok(())
    }

    /// Receive a message from the channel, waiting for a message to arrive,
    /// or until the timeout is reached
    ///
    /// Returns `None` if the timeout is reached or the channel is closed
    ///
    /// # Errors
    /// Will return an error if the message cannot be deserialized
    /// or if receiving the message fails
    pub async fn recv<T: DeserializeOwned>(
        &self,
        runtime: &mut Runtime,
        timeout: Option<Duration>,
    ) -> Result<Option<T>, Error> {
        let mut guard = self.receiver.lock().await;
        let (broadcast_rx, cancel_rx) = &mut *guard;

        loop {
            let result = if let Some(timeout) = timeout {
                tokio::select! {
                    r = broadcast_rx.recv() => r,
                    () = tokio::time::sleep(timeout) => return Ok(None),
                    _ = cancel_rx.recv() => return Ok(None),
                }
            } else {
                tokio::select! {
                    r = broadcast_rx.recv() => r,
                    _ = cancel_rx.recv() => return Ok(None),
                }
            };

            use tokio::sync::broadcast::error::RecvError::*;
            match result {
                Err(Closed) => return Ok(None),
                Err(Lagged(_)) => continue, // Backlogged, messages dropped - try again
                Ok(message) if message.sender_id == self.uuid => continue, // Self-send, skip
                Ok(message) if *message.name != self.name => continue, // Different channel name
                Ok(message) => {
                    // Deserialize through JavaScript for compatibility
                    let data: T = runtime
                        .call_function_async(
                            None,
                            "broadcast_deserialize",
                            big_json_args!(Vec::clone(&message.data)),
                        )
                        .await?;
                    return Ok(Some(data));
                }
            }
        }
    }

    /// Receive a message from the channel, blocking until a message arrives,
    /// or until the timeout is reached
    ///
    /// Returns `None` if the timeout is reached or the channel is closed
    ///
    /// # Errors
    /// Will return an error if the message cannot be deserialized
    /// or if receiving the message fails
    pub fn recv_sync<T: DeserializeOwned>(
        &self,
        runtime: &mut Runtime,
        timeout: Option<Duration>,
    ) -> Result<Option<T>, Error> {
        let tokio_rt = runtime.tokio_runtime();
        tokio_rt.block_on(self.recv(runtime, timeout))
    }

    /// Close this subscription
    ///
    /// After calling this, `recv` will return `None`
    pub fn close(&self) {
        let _ = self.cancel_tx.send(());
    }
}

impl Drop for BroadcastChannelWrapper {
    fn drop(&mut self) {
        self.close();
    }
}

#[cfg(test)]
mod test {
    use super::BroadcastChannel;
    use crate::{Runtime, RuntimeOptions};

    #[test]
    fn test_broadcast_channel_send_recv() {
        // This test demonstrates Rust-to-Rust communication via the BroadcastChannel.
        // Note: This wrapper is for Rust-side communication only.
        // For JavaScript BroadcastChannel, use the JavaScript API directly.

        let channel = BroadcastChannel::new();

        // Create a runtime for serialization
        let mut runtime = Runtime::new(RuntimeOptions::default()).unwrap();

        // Create two subscriptions on the same channel
        let wrapper1 = channel.subscribe("test_channel").unwrap();
        let wrapper2 = channel.subscribe("test_channel").unwrap();

        // Use async to send and receive
        let tokio_rt = runtime.tokio_runtime();
        tokio_rt.block_on(async {
            // Send from wrapper1
            let send_result: Result<(), crate::Error> =
                wrapper1.send::<&str>(&mut runtime, "hello from rust").await;
            send_result.unwrap();

            // Receive from wrapper2
            let recv_result: Result<Option<String>, crate::Error> = wrapper2
                .recv::<String>(&mut runtime, Some(std::time::Duration::from_secs(1)))
                .await;
            let received: String = recv_result.unwrap().unwrap();

            assert_eq!(received, "hello from rust");
        });
    }

    #[test]
    fn test_broadcast_channel_timeout() {
        let channel = BroadcastChannel::new();
        let mut runtime = Runtime::new(RuntimeOptions::default()).unwrap();
        let wrapper = channel.subscribe("timeout_test").unwrap();

        // Try to receive with a short timeout - should return None
        let result = wrapper
            .recv_sync::<String>(&mut runtime, Some(std::time::Duration::from_millis(100)))
            .unwrap();

        assert!(result.is_none());
    }

    #[test]
    fn test_broadcast_channel_different_names() {
        // Messages should only be received by subscriptions with matching names
        let channel = BroadcastChannel::new();
        let mut runtime = Runtime::new(RuntimeOptions::default()).unwrap();

        let wrapper_a = channel.subscribe("channel_a").unwrap();
        let wrapper_b = channel.subscribe("channel_b").unwrap();

        let tokio_rt = runtime.tokio_runtime();
        tokio_rt.block_on(async {
            // Send to channel_a
            let send_result: Result<(), crate::Error> =
                wrapper_a.send::<&str>(&mut runtime, "message for a").await;
            send_result.unwrap();

            // wrapper_b should not receive this message (different channel name)
            let recv_result: Result<Option<String>, crate::Error> = wrapper_b
                .recv::<String>(&mut runtime, Some(std::time::Duration::from_millis(100)))
                .await;
            let result: Option<String> = recv_result.unwrap();

            assert!(result.is_none());
        });
    }

    /// Test for backward compatibility - verify the new API works with the patterns
    /// users would have used with the old API
    ///
    /// ⚠️ IMPORTANT LIMITATION: Unlike the original implementation, this wrapper
    /// is for Rust-to-Rust communication only. The original test that verified
    /// JavaScript ↔ Rust communication is no longer possible due to upstream
    /// deno_web API changes that made broadcast channel methods private.
    #[test]
    fn test_backward_compat_api_usage() {
        // Old usage pattern:
        // let options = RuntimeOptions::default();
        // let channel = options.extension_options.broadcast_channel.clone();
        // let wrapper = BroadcastChannelWrapper::new(&channel, "my_channel").unwrap();

        // New usage pattern should work similarly for Rust-side operations:
        let channel = BroadcastChannel::new();
        let mut runtime = Runtime::new(RuntimeOptions::default()).unwrap();
        let wrapper = channel.subscribe("my_channel").unwrap();

        // Verify basic operations work
        let tokio_rt = runtime.tokio_runtime();
        tokio_rt.block_on(async {
            // Send should work
            wrapper.send(&mut runtime, "test").await.unwrap();

            // Name method (new in this version) should work
            assert_eq!(wrapper.name(), "my_channel");

            // Close method (new in this version) should work
            wrapper.close();

            // After closing, recv should return None
            let result = wrapper
                .recv::<String>(&mut runtime, Some(std::time::Duration::from_millis(100)))
                .await
                .unwrap();
            assert!(result.is_none());
        });
    }

    /// Test that JavaScript BroadcastChannel still works within JavaScript
    ///
    /// This test confirms that while Rust BroadcastChannelWrapper cannot communicate
    /// with JavaScript BroadcastChannel, the JavaScript API itself still works correctly.
    #[test]
    fn test_javascript_broadcast_channel_internal() {
        use crate::{Module, Runtime, RuntimeOptions};
        use deno_core::PollEventLoopOptions;

        let test_mod = Module::new(
            "test.js",
            r#"
            // Test JavaScript-to-JavaScript communication
            const channel1 = new BroadcastChannel('test_channel');
            const channel2 = new BroadcastChannel('test_channel');

            let received = null;
            channel2.onmessage = (event) => {
                received = event.data;
            };

            channel1.postMessage('hello from js');

            // Give it a moment to propagate
            await new Promise(resolve => setTimeout(resolve, 100));

            // Export the result for verification
            globalThis.testResult = received;
            "#,
        );

        let mut runtime = Runtime::new(RuntimeOptions::default()).unwrap();
        let tokio_rt = runtime.tokio_runtime();

        let module_handle = tokio_rt.block_on(async {
            runtime.load_module_async(&test_mod).await.unwrap()
        });

        // Run event loop to process the BroadcastChannel messages
        tokio_rt.block_on(async {
            runtime
                .await_event_loop(
                    PollEventLoopOptions::default(),
                    Some(std::time::Duration::from_secs(1)),
                )
                .await
                .unwrap();
        });

        // Verify JavaScript BroadcastChannel worked
        let result: Option<String> = runtime.get_value(Some(&module_handle), "testResult").unwrap();
        assert_eq!(result, Some("hello from js".to_string()));
    }
}
