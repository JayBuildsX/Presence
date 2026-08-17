//! Discord IPC client.
//!
//! Owns the handshake, packet serialization, and connection state.
//! Knows nothing about the Activity model — only Discord packets.

use crate::protocol::{
    ActivityData, ActivityUpdateArgs, Assets, CommandEnvelope, Frame, HandshakeMessage, Opcode,
    ProtocolError, ReadyMessage, Timestamps,
};
use crate::transport::{NamedPipeTransport, TransportError};
use tracing::{debug, warn};

// ---------------------------------------------------------------------------
// Client Error
// ---------------------------------------------------------------------------

/// Errors that can occur during client operations.
#[derive(Debug, thiserror::Error)]
pub enum ClientError {
    #[error("transport error: {0}")]
    Transport(#[from] TransportError),
    #[error("protocol error: {0}")]
    Protocol(#[from] ProtocolError),
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("handshake failed: {0}")]
    HandshakeFailed(String),
    #[allow(dead_code)]
    #[error("not connected")]
    NotConnected,
}

// ---------------------------------------------------------------------------
// DiscordClient
// ---------------------------------------------------------------------------

/// Discord IPC client.
///
/// Owns:
/// - Handshake with Discord
/// - Packet serialization/deserialization
/// - Activity update packets
/// - Connection state
///
/// No Activity knowledge.
pub struct DiscordClient {
    transport: NamedPipeTransport,
    application_id: u64,
    connected: bool,
    /// Monotonic nonce counter for command envelopes.
    nonce_counter: u64,
}

impl DiscordClient {
    /// Create a new Discord client.
    pub fn new(application_id: u64) -> Self {
        Self {
            transport: NamedPipeTransport::new(),
            application_id,
            connected: false,
            nonce_counter: 0,
        }
    }

    /// Returns whether the client is currently connected.
    pub fn is_connected(&self) -> bool {
        self.connected
    }

    /// Connect to Discord and perform the handshake.
    pub fn connect(&mut self) -> Result<(), ClientError> {
        // Connect the transport, then send the handshake frame (opcode 0).
        self.transport.connect()?;
        let frame = Self::build_handshake_frame(self.application_id)?;
        self.transport.write_frame(&frame)?;

        // Read the server's reply and validate it.
        let response = self.transport.read_frame()?;
        let result = self.handle_handshake_response(response);
        match &result {
            Ok(()) => debug!("Discord handshake success"),
            Err(e) => warn!(error = %e, "Discord handshake failed"),
        }
        if result.is_ok() {
            // Consume any frames Discord already queued behind the READY
            // reply so stale PINGs/CLOSE frames cannot pile up.
            self.drain_incoming();
        }
        result
    }

    /// Handle a single incoming frame. Returns `false` when the connection
    /// should be torn down (a `CLOSE` frame, or a `PONG` that could not be
    /// written).
    fn handle_incoming_frame(&mut self, frame: Frame) -> bool {
        match frame.opcode {
            Opcode::Ping => {
                // Discord keeps the connection alive with periodic PINGs; an
                // unanswered ping eventually makes Discord drop us.
                let pong = Frame::new(Opcode::Pong, vec![]);
                match self.transport.write_frame(&pong) {
                    Ok(()) => true,
                    Err(e) => {
                        warn!(error = %e, "Failed to send PONG");
                        self.connected = false;
                        self.transport.disconnect();
                        false
                    }
                }
            }
            Opcode::Close => {
                warn!("Discord IPC CLOSE frame received; disconnecting");
                self.connected = false;
                self.transport.disconnect();
                false
            }
            _ => {
                // DISPATCH events and command responses: log them (the
                // transport's diagnostic path) and keep draining.
                NamedPipeTransport::log_received_frame(&frame);
                true
            }
        }
    }

    /// Consume every frame Discord has already sent, without blocking.
    ///
    /// After the handshake the connection is used mostly for writes, but
    /// Discord still sends frames at any time: periodic `PING`s, `CLOSE`, and
    /// `DISPATCH` events. Not consuming them leaves pings unanswered and
    /// hides close/error frames. Called after connecting and before each
    /// command so the pipe never silently accumulates server traffic.
    pub fn drain_incoming(&mut self) {
        loop {
            let frame = match self.transport.try_read_frame() {
                Ok(Some(frame)) => frame,
                Ok(None) => break,
                Err(_) => {
                    self.connected = false;
                    self.transport.disconnect();
                    break;
                }
            };
            if !self.handle_incoming_frame(frame) {
                break;
            }
        }
    }

    /// Build the handshake frame for the given application id.
    ///
    /// Per the official protocol this is an opcode-0 (`Handshake`) frame whose
    /// payload is `{"v": 1, "client_id": "<id>"}`.
    fn build_handshake_frame(application_id: u64) -> Result<Frame, ClientError> {
        let handshake = HandshakeMessage {
            version: 1,
            client_id: application_id.to_string(),
        };
        let payload = serde_json::to_vec(&handshake)?;
        Ok(Frame::new(Opcode::Handshake, payload))
    }

    /// Validate the server's handshake response.
    ///
    /// The official protocol answers a handshake with an opcode-1 (`Frame`)
    /// carrying a `DISPATCH`/`READY` message. An opcode-2 (`Close`) frame means
    /// the handshake was rejected. Any other frame is invalid mid-handshake.
    fn handle_handshake_response(&mut self, response: Frame) -> Result<(), ClientError> {
        match response.opcode {
            Opcode::Frame => {
                let ready: ReadyMessage = match serde_json::from_slice(&response.payload) {
                    Ok(ready) => ready,
                    Err(e) => {
                        self.connected = false;
                        self.transport.disconnect();
                        return Err(ClientError::HandshakeFailed(format!(
                            "Invalid READY payload: {}",
                            e
                        )));
                    }
                };
                if ready.cmd == "DISPATCH" && ready.evt == "READY" {
                    self.connected = true;
                    Ok(())
                } else {
                    self.connected = false;
                    self.transport.disconnect();
                    Err(ClientError::HandshakeFailed(format!(
                        "Unexpected handshake response: cmd={}, evt={}",
                        ready.cmd, ready.evt
                    )))
                }
            }
            Opcode::Close => {
                self.connected = false;
                self.transport.disconnect();
                Err(ClientError::HandshakeFailed(
                    "Discord rejected handshake".to_string(),
                ))
            }
            other => {
                self.connected = false;
                self.transport.disconnect();
                Err(ClientError::HandshakeFailed(format!(
                    "Unexpected opcode during handshake: {:?}",
                    other
                )))
            }
        }
    }

    /// Disconnect from Discord.
    pub fn disconnect(&mut self) {
        // Send close frame
        if self.connected {
            let close_frame = Frame::new(Opcode::Close, vec![]);
            let _ = self.transport.write_frame(&close_frame);
        }
        self.transport.disconnect();
        self.connected = false;
    }

    /// Send an activity update to Discord.
    ///
    /// The activity is wrapped in the required `SET_ACTIVITY` command
    /// envelope. If the connection is lost, the client reconnects and
    /// retries once.
    pub fn set_activity(&mut self, activity: ActivityData) -> Result<(), ClientError> {
        debug!(
            state = activity.state,
            details = activity.details,
            "DiscordClient::set_activity executed"
        );
        if !self.connected {
            debug!("DiscordClient not connected, connecting before set_activity");
            self.connect()?;
        }

        // Consume any pending PING/CLOSE frames before writing so a close
        // that arrived since the last publish is not left unread.
        self.drain_incoming();
        if !self.connected {
            // The drain surfaced a CLOSE/read failure; reconnect once.
            self.connect()?;
        }

        let envelope = CommandEnvelope {
            cmd: "SET_ACTIVITY".to_string(),
            args: ActivityUpdateArgs {
                pid: std::process::id(),
                activity: Some(activity),
            },
            nonce: self.next_nonce(),
        };

        self.write_envelope(&envelope)
    }

    /// Clear the current Rich Presence activity.
    ///
    /// Sends a `SET_ACTIVITY` command with `activity: null`, which tells
    /// Discord to remove the presence. If the connection is lost, the
    /// client reconnects and retries once.
    pub fn clear_activity(&mut self) -> Result<(), ClientError> {
        if !self.connected {
            // If not connected, there is nothing to clear.
            return Ok(());
        }

        // Consume any pending PING/CLOSE frames before writing.
        self.drain_incoming();
        if !self.connected {
            // The drain surfaced a CLOSE/read failure; the connection is
            // gone, so there is nothing left to clear.
            return Ok(());
        }

        let envelope = CommandEnvelope {
            cmd: "SET_ACTIVITY".to_string(),
            args: ActivityUpdateArgs {
                pid: std::process::id(),
                activity: None,
            },
            nonce: self.next_nonce(),
        };

        self.write_envelope(&envelope)
    }

    /// Reconnect to Discord.
    #[allow(dead_code)]
    pub fn reconnect(&mut self) -> Result<(), ClientError> {
        self.disconnect();
        self.connect()
    }

    /// Generate the next nonce for a command envelope.
    fn next_nonce(&mut self) -> String {
        self.nonce_counter += 1;
        format!("presencehub-{}", self.nonce_counter)
    }

    /// Build a command frame (opcode 1, `Frame`) wrapping the given envelope.
    ///
    /// All commands such as `SET_ACTIVITY` must be sent as opcode-1 frames per
    /// the official protocol.
    fn build_command_frame(envelope: &CommandEnvelope) -> Result<Frame, ClientError> {
        let data = serde_json::to_vec(envelope)?;
        debug!(
            cmd = %envelope.cmd,
            json = %String::from_utf8_lossy(&data),
            "Discord SET_ACTIVITY JSON payload"
        );
        Ok(Frame::new(Opcode::Frame, data))
    }

    /// Write a command envelope, reconnecting once if the write fails.
    fn write_envelope(&mut self, envelope: &CommandEnvelope) -> Result<(), ClientError> {
        let frame = Self::build_command_frame(envelope)?;

        match self.transport.write_frame(&frame) {
            Ok(_) => Ok(()),
            Err(_) => {
                // Connection lost — mark disconnected, reconnect, and retry once.
                self.connected = false;
                self.transport.disconnect();
                self.connect()?;
                let frame = Self::build_command_frame(envelope)?;
                self.transport
                    .write_frame(&frame)
                    .map_err(ClientError::Transport)
            }
        }
    }
}

impl Drop for DiscordClient {
    fn drop(&mut self) {
        self.disconnect();
    }
}

// ---------------------------------------------------------------------------
// ActivityData builder
// ---------------------------------------------------------------------------

/// Build an ActivityData from canonical Activity fields.
pub fn build_activity_data(
    state: &str,
    details: Option<&str>,
    start: Option<i64>,
    end: Option<i64>,
    large_text: Option<&str>,
    small_text: Option<&str>,
    large_image: Option<&str>,
    small_image: Option<&str>,
) -> ActivityData {
    let timestamps = if start.is_some() || end.is_some() {
        Some(Timestamps { start, end })
    } else {
        None
    };

    let assets = if large_text.is_some()
        || small_text.is_some()
        || large_image.is_some()
        || small_image.is_some()
    {
        Some(Assets {
            large_text: large_text.map(|s| s.to_string()),
            small_text: small_text.map(|s| s.to_string()),
            large_image: large_image.map(|s| s.to_string()),
            small_image: small_image.map(|s| s.to_string()),
        })
    } else {
        None
    };

    ActivityData {
        state: state.to_string(),
        details: details.map(|s| s.to_string()),
        timestamps,
        assets,
        r#type: None,
        buttons: Vec::new(),
        party: None,
        secrets: None,
        instance: None,
        flags: 0,
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transport::NamedPipeTransport;

    /// Whether a Discord IPC pipe is currently reachable (i.e. Discord is
    /// running locally). Used by environment-dependent connectivity tests.
    fn discord_pipe_available() -> bool {
        let mut transport = NamedPipeTransport::new();
        transport.connect().is_ok()
    }

    #[test]
    fn client_initial_state() {
        let client = DiscordClient::new(123456789);
        assert!(!client.is_connected());
    }

    #[test]
    fn client_connect_fails_when_discord_not_running() {
        // Environment-dependent: skips when Discord is running locally.
        let _guard = crate::DISCORD_PIPE_TEST_LOCK.lock().unwrap();
        if discord_pipe_available() {
            return;
        }
        let mut client = DiscordClient::new(123456789);
        let result = client.connect();
        assert!(result.is_err());
    }

    #[test]
    fn client_connect_succeeds_when_discord_running() {
        // Environment-dependent: only meaningful when Discord is running locally.
        let _guard = crate::DISCORD_PIPE_TEST_LOCK.lock().unwrap();
        if !discord_pipe_available() {
            return;
        }
        let mut client = DiscordClient::new(1533559059125637311);
        let result = client.connect();
        assert!(
            result.is_ok(),
            "handshake should complete against a live Discord: {:?}",
            result
        );
        client.disconnect();
    }

    #[test]
    fn handshake_frame_uses_handshake_opcode_and_payload() {
        let frame = DiscordClient::build_handshake_frame(123456789).unwrap();
        assert_eq!(frame.opcode, Opcode::Handshake);
        assert_eq!(frame.opcode.to_u32(), 0);
        let encoded = frame.encode();
        assert_eq!(
            u32::from_le_bytes([encoded[0], encoded[1], encoded[2], encoded[3]]),
            0
        );
        let parsed: serde_json::Value = serde_json::from_slice(&frame.payload).unwrap();
        assert_eq!(parsed["v"], 1);
        assert_eq!(parsed["client_id"], "123456789");
    }

    #[test]
    fn command_frame_uses_frame_opcode() {
        let envelope = CommandEnvelope {
            cmd: "SET_ACTIVITY".to_string(),
            args: ActivityUpdateArgs {
                pid: 1234,
                activity: None,
            },
            nonce: "presencehub-1".to_string(),
        };
        let frame = DiscordClient::build_command_frame(&envelope).unwrap();
        assert_eq!(frame.opcode, Opcode::Frame);
        assert_eq!(frame.opcode.to_u32(), 1);
        let encoded = frame.encode();
        assert_eq!(
            u32::from_le_bytes([encoded[0], encoded[1], encoded[2], encoded[3]]),
            1
        );
        let parsed: serde_json::Value = serde_json::from_slice(&frame.payload).unwrap();
        assert_eq!(parsed["cmd"], "SET_ACTIVITY");
        assert_eq!(parsed["args"]["pid"], 1234);
        assert_eq!(parsed["nonce"], "presencehub-1");
    }

    #[test]
    fn handshake_response_accepts_ready_dispatch() {
        let mut client = DiscordClient::new(123456789);
        let payload = br#"{"cmd":"DISPATCH","data":{"v":1},"evt":"READY","nonce":null}"#.to_vec();
        let frame = Frame::new(Opcode::Frame, payload);
        let result = client.handle_handshake_response(frame);
        assert!(result.is_ok());
        assert!(client.is_connected());
    }

    #[test]
    fn handshake_response_rejects_close() {
        let mut client = DiscordClient::new(123456789);
        let frame = Frame::new(
            Opcode::Close,
            br#"{"code":4000,"message":"invalid client id"}"#.to_vec(),
        );
        let result = client.handle_handshake_response(frame);
        assert!(result.is_err());
        assert!(!client.is_connected());
    }

    #[test]
    fn handshake_response_rejects_non_ready_dispatch() {
        let mut client = DiscordClient::new(123456789);
        let payload = br#"{"cmd":"DISPATCH","evt":"ERROR","data":{"code":4000}}"#.to_vec();
        let frame = Frame::new(Opcode::Frame, payload);
        let result = client.handle_handshake_response(frame);
        assert!(result.is_err());
        assert!(!client.is_connected());
    }

    #[test]
    fn handshake_response_rejects_unexpected_opcode() {
        let mut client = DiscordClient::new(123456789);
        let frame = Frame::new(Opcode::Ping, vec![]);
        let result = client.handle_handshake_response(frame);
        assert!(result.is_err());
        assert!(!client.is_connected());
    }

    #[test]
    fn handshake_response_rejects_unparseable_payload() {
        let mut client = DiscordClient::new(123456789);
        let frame = Frame::new(Opcode::Frame, b"not json".to_vec());
        let result = client.handle_handshake_response(frame);
        assert!(result.is_err());
        assert!(!client.is_connected());
    }

    #[test]
    fn build_activity_data_with_all_fields() {
        let data = build_activity_data(
            "Editing",
            Some("song.flp"),
            Some(1000),
            Some(2000),
            Some("FL Studio"),
            Some("21"),
            Some("flstudio"),
            None,
        );

        assert_eq!(data.state, "Editing");
        assert_eq!(data.details, Some("song.flp".to_string()));
        assert_eq!(data.timestamps.as_ref().unwrap().start, Some(1000));
        assert_eq!(data.timestamps.as_ref().unwrap().end, Some(2000));
        assert_eq!(
            data.assets.as_ref().unwrap().large_text,
            Some("FL Studio".to_string())
        );
        assert_eq!(
            data.assets.as_ref().unwrap().small_text,
            Some("21".to_string())
        );
        assert_eq!(
            data.assets.as_ref().unwrap().large_image,
            Some("flstudio".to_string())
        );
    }

    #[test]
    fn build_activity_data_without_optional_fields() {
        let data = build_activity_data("Idle", None, None, None, None, None, None, None);

        assert_eq!(data.state, "Idle");
        assert!(data.details.is_none());
        assert!(data.timestamps.is_none());
        assert!(data.assets.is_none());
    }

    #[test]
    fn build_activity_data_with_only_timestamps() {
        let data = build_activity_data("Playing", None, Some(1000), None, None, None, None, None);

        assert_eq!(data.state, "Playing");
        assert!(data.details.is_none());
        assert!(data.timestamps.is_some());
        assert!(data.assets.is_none());
    }

    #[test]
    fn build_activity_data_with_only_assets() {
        let data = build_activity_data("Editing", None, None, None, Some("App"), None, None, None);

        assert_eq!(data.state, "Editing");
        assert!(data.details.is_none());
        assert!(data.timestamps.is_none());
        assert!(data.assets.is_some());
        assert_eq!(
            data.assets.as_ref().unwrap().large_text,
            Some("App".to_string())
        );
    }

    #[test]
    fn build_activity_data_with_large_image() {
        let data = build_activity_data(
            "Editing",
            None,
            None,
            None,
            Some("FL Studio"),
            None,
            Some("flstudio"),
            None,
        );

        assert_eq!(
            data.assets.as_ref().unwrap().large_image,
            Some("flstudio".to_string())
        );
    }

    #[test]
    fn clear_activity_when_not_connected_is_noop() {
        let mut client = DiscordClient::new(123456789);
        // Not connected — clearing should succeed without error.
        assert!(client.clear_activity().is_ok());
    }

    #[cfg(windows)]
    #[test]
    fn clear_activity_sends_set_activity_with_null_activity() {
        // Graceful shutdown must send an explicit SET_ACTIVITY with
        // activity: null so Discord removes the presence. This verifies the
        // actual IPC payload written to the pipe, not just internal state.
        use crate::transport::test_support;

        let name = format!("ph-presencehub-clear-payload-{}", std::process::id());
        let (transport, server) = test_support::create_test_pipe(&name);
        let mut client = DiscordClient::new(1533559059125637311);
        client.transport = transport;
        client.connected = true;

        client.clear_activity().expect("clear should succeed");

        let frame = server.read_frame();
        assert_eq!(
            frame.opcode,
            Opcode::Frame,
            "SET_ACTIVITY must be sent as an opcode-1 Frame"
        );
        let payload: serde_json::Value =
            serde_json::from_slice(&frame.payload).expect("envelope must be JSON");
        assert_eq!(
            payload["cmd"], "SET_ACTIVITY",
            "clear must use the SET_ACTIVITY command; got {:?}",
            payload
        );
        assert!(
            payload["args"]["activity"].is_null(),
            "clear must send activity: null; got {:?}",
            payload
        );
    }

    // -- incoming frame handling (the post-handshake read loop) ----------------

    #[test]
    fn handle_incoming_close_disconnects() {
        let mut client = DiscordClient::new(123456789);
        let frame = Frame::new(Opcode::Close, br#"{"code":4000}"#.to_vec());
        assert!(
            !client.handle_incoming_frame(frame),
            "CLOSE tears the connection down"
        );
        assert!(!client.is_connected());
    }

    #[test]
    fn handle_incoming_dispatch_keeps_connection() {
        let mut client = DiscordClient::new(123456789);
        // A DISPATCH frame (e.g. a command response) is consumed and logged;
        // the drain must continue (returns true) without tearing down.
        let payload = br#"{"cmd":"DISPATCH","evt":"ACTIVITY_JOIN","data":{}}"#.to_vec();
        let frame = Frame::new(Opcode::Frame, payload);
        assert!(
            client.handle_incoming_frame(frame),
            "DISPATCH does not tear down"
        );
    }

    #[test]
    fn handle_incoming_ping_when_disconnected_degrades_safely() {
        let mut client = DiscordClient::new(123456789);
        // No pipe is attached, so the PONG write fails; the client must
        // degrade to disconnected rather than panic or hang.
        let frame = Frame::new(Opcode::Ping, vec![]);
        assert!(!client.handle_incoming_frame(frame));
        assert!(!client.is_connected());
    }

    #[test]
    fn drain_incoming_without_connection_is_noop() {
        let mut client = DiscordClient::new(123456789);
        // A transport with no pipe must not panic or block.
        client.drain_incoming();
        assert!(!client.is_connected());
    }

    #[cfg(windows)]
    #[test]
    fn drain_answers_ping_with_pong() {
        use crate::transport::test_support;

        let name = format!("ph-presencehub-ping-{}", std::process::id());
        let (transport, server) = test_support::create_test_pipe(&name);
        let mut client = DiscordClient::new(123456789);
        client.transport = transport;
        client.connected = true;

        // Discord sends a PING; drain must answer with a PONG frame.
        server.write_frame(&Frame::new(Opcode::Ping, vec![]));
        client.drain_incoming();

        let pong = server.read_frame();
        assert_eq!(pong.opcode, Opcode::Pong, "PING must be answered with PONG");
        assert!(client.is_connected(), "connection stays alive after PONG");
    }

    #[cfg(windows)]
    #[test]
    fn drain_disconnects_on_close() {
        use crate::transport::test_support;

        let name = format!("ph-presencehub-close-{}", std::process::id());
        let (transport, server) = test_support::create_test_pipe(&name);
        let mut client = DiscordClient::new(123456789);
        client.transport = transport;
        client.connected = true;

        server.write_frame(&Frame::new(Opcode::Close, br#"{"code":4000}"#.to_vec()));
        client.drain_incoming();

        assert!(
            !client.is_connected(),
            "CLOSE must mark the client disconnected"
        );
    }

    #[cfg(windows)]
    #[test]
    fn drain_consumes_stacked_ping_then_close() {
        use crate::transport::test_support;

        let name = format!("ph-presencehub-stacked-{}", std::process::id());
        let (transport, server) = test_support::create_test_pipe(&name);
        let mut client = DiscordClient::new(123456789);
        client.transport = transport;
        client.connected = true;

        // A PING followed by a CLOSE, both buffered before the drain runs:
        // the drain answers the ping, then observes the close and disconnects.
        server.write_frame(&Frame::new(Opcode::Ping, vec![]));
        server.write_frame(&Frame::new(Opcode::Close, vec![]));
        client.drain_incoming();

        let pong = server.read_frame();
        assert_eq!(
            pong.opcode,
            Opcode::Pong,
            "PING answered before CLOSE processed"
        );
        assert!(!client.is_connected(), "CLOSE observed after the PONG");
    }

    #[cfg(windows)]
    #[test]
    fn drain_rejects_oversized_frame_and_disconnects() {
        use crate::transport::test_support;

        let name = format!("ph-presencehub-oversize-drain-{}", std::process::id());
        let (transport, server) = test_support::create_test_pipe(&name);
        let mut client = DiscordClient::new(123456789);
        client.transport = transport;
        client.connected = true;

        // A header claiming a payload larger than the cap, with no payload
        // behind it: the drain must tear the connection down rather than
        // hang or silently consume the malformed frame.
        let mut header = [0u8; 8];
        header[0] = 1; // opcode Frame
        let big = (crate::protocol::MAX_FRAME_PAYLOAD_LEN + 1) as u32;
        header[4..8].copy_from_slice(&big.to_le_bytes());
        server.write_bytes(&header);

        client.drain_incoming();

        assert!(
            !client.is_connected(),
            "an oversized inbound frame must disconnect the client"
        );
    }
}
