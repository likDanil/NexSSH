//! Delivers session events to the webview through a Tauri [`Channel`].
//!
//! Terminal output is sent as raw bytes (an `ArrayBuffer` in JavaScript, no JSON
//! encoding); status changes and prompts are sent as JSON objects.

use nexssh_core::{EventSink, SessionEvent};
use tauri::ipc::{Channel, InvokeResponseBody};

pub struct ChannelSink {
    channel: Channel<InvokeResponseBody>,
}

impl ChannelSink {
    pub fn new(channel: Channel<InvokeResponseBody>) -> Self {
        ChannelSink { channel }
    }
}

impl EventSink for ChannelSink {
    fn event(&self, event: SessionEvent) {
        match serde_json::to_string(&event) {
            Ok(json) => {
                let _ = self.channel.send(InvokeResponseBody::Json(json));
            }
            Err(e) => log::error!("cannot serialize session event: {e}"),
        }
    }

    fn output(&self, data: Vec<u8>) {
        let _ = self.channel.send(InvokeResponseBody::Raw(data));
    }
}
