//! Shared broadcast channel wrapper that can communicate with JavaScript BroadcastChannel
//!
//! This module provides a wrapper that shares the same underlying channel as
//! JavaScript's BroadcastChannel API, enabling Rust ↔ JavaScript communication.

use std::sync::Arc;
use std::time::Duration;

use deno_core::parking_lot::Mutex;
use deno_web::InMemoryBroadcastChannel;
use serde::{de::DeserializeOwned, Serialize};
use tokio::sync::broadcast;
use tokio::sync::mpsc;
use uuid::Uuid;

use crate::{big_json_args, Error, Runtime};

/// Message type matching deno_web's internal InMemoryChannelMessage structure
#[derive(Clone, Debug)]
struct InMemoryChannelMessage {
    name: Arc<String>,
    data: Arc<Vec<u8>>,
    uuid: Uuid,
}

/// A wrapper that shares the underlying channel with JavaScript BroadcastChannel
///
/// This allows Rust ↔ JavaScript bidirectional communication through BroadcastChannel.
///
/// # Example
/// ```rust,ignore
/// use rustyscript::{SharedBroadcastChannelWrapper, Runtime, RuntimeOptions};
///
/// let mut options = RuntimeOptions::default();
/// let channel = options.extension_options.web_options.broadcast_channel.clone();
///
/// let mut runtime = Runtime::new(options)?;
/// let wrapper = SharedBroadcastChannelWrapper::new(&channel, "my_channel")?;
///
/// // Send from Rust to JavaScript
/// wrapper.send_sync(&mut runtime, "hello from rust")?;
///
/// // JavaScript can receive this message:
/// // const channel = new BroadcastChannel('my_channel');
/// // channel.onmessage = (event) => console.log(event.data); // "hello from rust"
/// ```
pub struct SharedBroadcastChannelWrapper {
    sender: Arc<Mutex<broadcast::Sender<InMemoryChannelMessage>>>,
    receiver: tokio::sync::Mutex<(
        broadcast::Receiver<InMemoryChannelMessage>,
        mpsc::UnboundedReceiver<()>,
    )>,
    cancel_tx: mpsc::UnboundedSender<()>,
    name: String,
    uuid: Uuid,
}

impl SharedBroadcastChannelWrapper {
    /// Create a new wrapper that shares the channel with JavaScript BroadcastChannel
    ///
    /// # Safety
    /// This function uses unsafe code to access the private field of `InMemoryBroadcastChannel`.
    /// The memory layout is stable because it's a simple tuple struct wrapping `Arc<Mutex<...>>`.
    ///
    /// # Errors
    /// Will return an error if the wrapper cannot be created
    pub fn new(channel: &InMemoryBroadcastChannel, name: impl ToString) -> Result<Self, Error> {
        // SAFETY: InMemoryBroadcastChannel is repr(Rust) tuple struct with single field:
        // pub struct InMemoryBroadcastChannel(Arc<Mutex<broadcast::Sender<InMemoryChannelMessage>>>);
        //
        // We can access the field by transmuting to a tuple:
        let sender: &Arc<Mutex<broadcast::Sender<InMemoryChannelMessage>>> = unsafe {
            &*(channel as *const InMemoryBroadcastChannel
                as *const Arc<Mutex<broadcast::Sender<InMemoryChannelMessage>>>)
        };

        let sender = sender.clone();
        let (cancel_tx, cancel_rx) = mpsc::unbounded_channel();
        let broadcast_rx = sender.lock().subscribe();
        let receiver = tokio::sync::Mutex::new((broadcast_rx, cancel_rx));
        let uuid = Uuid::new_v4();
        let name = name.to_string();

        Ok(Self {
            sender,
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

    /// Send a message to the channel (including to JavaScript BroadcastChannel listeners)
    ///
    /// # Errors
    /// Will return an error if the message cannot be serialized or sent
    pub async fn send<T: Serialize>(&self, runtime: &mut Runtime, data: T) -> Result<(), Error> {
        // Serialize through JavaScript for compatibility
        let data: Vec<u8> = runtime
            .call_function_async(None, "broadcast_serialize", &data)
            .await?;

        let message = InMemoryChannelMessage {
            name: Arc::new(self.name.clone()),
            data: Arc::new(data),
            uuid: self.uuid,
        };

        self.sender
            .lock()
            .send(message)
            .map_err(|e| Error::Runtime(format!("Failed to send broadcast message: {e}")))?;

        Ok(())
    }

    /// Send a message to the channel, blocking until the message is sent
    ///
    /// # Errors
    /// Will return an error if the message cannot be serialized or sent
    pub fn send_sync<T: Serialize>(&self, runtime: &mut Runtime, data: T) -> Result<(), Error> {
        let tokio_rt = runtime.tokio_runtime();
        tokio_rt.block_on(self.send(runtime, data))
    }

    /// Receive a message from the channel (from Rust or JavaScript senders)
    ///
    /// Returns `None` if the timeout is reached or the channel is closed
    ///
    /// # Errors
    /// Will return an error if the message cannot be deserialized
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
                Ok(message) if message.uuid == self.uuid => continue, // Self-send, skip
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

    /// Receive a message from the channel, blocking until a message arrives
    ///
    /// Returns `None` if the timeout is reached or the channel is closed
    ///
    /// # Errors
    /// Will return an error if the message cannot be deserialized
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

impl Drop for SharedBroadcastChannelWrapper {
    fn drop(&mut self) {
        self.close();
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::{module, Module, Runtime, RuntimeOptions};
    use deno_core::PollEventLoopOptions;

    static TEST_MOD: Module = module!(
        "test.js",
        "
        const channel = new BroadcastChannel('my_channel');
        channel.onmessage = (event) => {
            console.log('JS received:', event.data);
            channel.postMessage('Received: ' + event.data);
        };
    "
    );

    #[test]
    fn test_shared_broadcast_channel_js_rust_communication() {
        // This test verifies that SharedBroadcastChannelWrapper can communicate
        // with JavaScript BroadcastChannel bidirectionally
        let options = RuntimeOptions::default();
        let channel = options.extension_options.web.broadcast_channel.clone();

        let mut runtime = Runtime::new(options).unwrap();
        let tokio_runtime = runtime.tokio_runtime();

        let wrapper = SharedBroadcastChannelWrapper::new(&channel, "my_channel").unwrap();

        // Load JavaScript module that listens to BroadcastChannel
        tokio_runtime
            .block_on(runtime.load_module_async(&TEST_MOD))
            .unwrap();

        // Send from Rust to JavaScript
        wrapper.send_sync(&mut runtime, "foo").unwrap();

        // Run event loop to let JavaScript process the message
        runtime
            .block_on_event_loop(
                PollEventLoopOptions::default(),
                Some(std::time::Duration::from_secs(1)),
            )
            .unwrap();

        // Receive reply from JavaScript
        let value = wrapper
            .recv_sync::<String>(&mut runtime, Some(std::time::Duration::from_secs(1)))
            .unwrap()
            .unwrap();

        assert_eq!(value, "Received: foo");
    }

    #[test]
    fn test_shared_wrapper_name_and_close() {
        let options = RuntimeOptions::default();
        let channel = options.extension_options.web.broadcast_channel.clone();
        let mut runtime = Runtime::new(options).unwrap();

        let wrapper = SharedBroadcastChannelWrapper::new(&channel, "test_channel").unwrap();
        assert_eq!(wrapper.name(), "test_channel");

        wrapper.close();

        // After closing, recv should return None
        let result = wrapper
            .recv_sync::<String>(&mut runtime, Some(std::time::Duration::from_millis(100)))
            .unwrap();
        assert!(result.is_none());
    }
}
