//! Discord IPC Protocol
//!
//! Internal protocol definitions for Discord Rich Presence IPC communication.
//! No external dependencies.

// ---------------------------------------------------------------------------
// Opcodes
// ---------------------------------------------------------------------------

/// Discord IPC opcodes.
///
/// The values match the official Discord IPC protocol (see the `discord-rpc`
/// reference implementation, `src/rpc_connection.h`):
///
/// | Value | Name      | Direction            |
/// |-------|-----------|----------------------|
/// | 0     | Handshake | client → server      |
/// | 1     | Frame     | both                 |
/// | 2     | Close     | both                 |
/// | 3     | Ping      | server → client      |
/// | 4     | Pong      | client → server      |
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum Opcode {
    /// Handshake from client to server.
    Handshake = 0,
    /// A protocol message frame (commands, dispatches, responses).
    Frame = 1,
    /// Close the connection.
    Close = 2,
    /// Liveness ping from the server.
    Ping = 3,
    /// Pong reply to a server ping.
    Pong = 4,
}

impl Opcode {
    /// Convert opcode to u32 for transmission.
    pub fn to_u32(self) -> u32 {
        self as u32
    }
}

// ---------------------------------------------------------------------------
// Frame
// ---------------------------------------------------------------------------

/// Maximum accepted payload length of an inbound frame, in bytes.
///
/// The 4-byte length field can claim up to 4 GiB. Without a cap, a corrupt
/// or hostile peer could trigger a multi-gigabyte allocation when the frame
/// is read. Discord IPC payloads are small JSON objects (the reference
/// `discord-rpc` implementation uses a 64 KiB buffer), so a 1 MiB cap is
/// generous while still bounding memory. Oversized frames are rejected with
/// [`ProtocolError::FrameTooLarge`].
pub const MAX_FRAME_PAYLOAD_LEN: usize = 1024 * 1024;

/// A Discord IPC frame.
///
/// Frame format:
/// [opcode: 4 bytes LE][length: 4 bytes LE][payload: length bytes]
#[derive(Debug, Clone)]
pub struct Frame {
    pub opcode: Opcode,
    pub payload: Vec<u8>,
}

impl Frame {
    pub fn new(opcode: Opcode, payload: Vec<u8>) -> Self {
        Self { opcode, payload }
    }

    /// Encode frame to bytes.
    pub fn encode(&self) -> Vec<u8> {
        let mut buf = Vec::with_capacity(8 + self.payload.len());
        buf.extend_from_slice(&self.opcode.to_u32().to_le_bytes());
        buf.extend_from_slice(&(self.payload.len() as u32).to_le_bytes());
        buf.extend_from_slice(&self.payload);
        buf
    }

    /// Decode frame from bytes.
    pub fn decode(data: &[u8]) -> Result<Self, ProtocolError> {
        if data.len() < 8 {
            return Err(ProtocolError::FrameTooShort);
        }

        let opcode = u32::from_le_bytes([data[0], data[1], data[2], data[3]]);
        let length = u32::from_le_bytes([data[4], data[5], data[6], data[7]]) as usize;

        // Reject oversized frames before checking completeness so a header
        // claiming a huge length fails fast without allocating the payload.
        if length > MAX_FRAME_PAYLOAD_LEN {
            return Err(ProtocolError::FrameTooLarge {
                length,
                max: MAX_FRAME_PAYLOAD_LEN,
            });
        }

        if data.len() < 8 + length {
            return Err(ProtocolError::FrameIncomplete);
        }

        let opcode = match opcode {
            0 => Opcode::Handshake,
            1 => Opcode::Frame,
            2 => Opcode::Close,
            3 => Opcode::Ping,
            4 => Opcode::Pong,
            _ => return Err(ProtocolError::UnknownOpcode(opcode)),
        };

        let payload = data[8..8 + length].to_vec();

        Ok(Self { opcode, payload })
    }
}

// ---------------------------------------------------------------------------
// Messages
// ---------------------------------------------------------------------------

/// Handshake message from client to server.
///
/// Serializes to `{"v": 1, "client_id": "<application id>"}`, which is the
/// payload the official protocol expects inside an opcode-0 (`Handshake`)
/// frame.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub struct HandshakeMessage {
    #[serde(rename = "v")]
    pub version: u32,
    #[serde(rename = "client_id")]
    pub client_id: String,
}

/// The server's handshake reply: a `Frame` (opcode 1) carrying a
/// `DISPATCH`/`READY` message.
///
/// Only the fields needed to validate the handshake are decoded; serde
/// ignores the remaining payload (e.g. `data`, `nonce`).
#[derive(Debug, serde::Deserialize)]
pub struct ReadyMessage {
    pub cmd: String,
    pub evt: String,
}

/// The command envelope for a SET_ACTIVITY request.
///
/// Discord's IPC protocol requires every command to be wrapped in a
/// `{ "cmd": "...", "args": { ... }, "nonce": "..." }` envelope.
/// Without this wrapper, Discord ignores the payload entirely.
#[derive(Debug, serde::Serialize)]
pub struct CommandEnvelope {
    pub cmd: String,
    pub args: ActivityUpdateArgs,
    pub nonce: String,
}

/// The `args` field of a SET_ACTIVITY command.
#[derive(Debug, serde::Serialize)]
pub struct ActivityUpdateArgs {
    pub pid: u32,
    pub activity: Option<ActivityData>,
}

/// Activity data for Discord Rich Presence.
///
/// Every field except `state` and `flags` is optional and is omitted from
/// the serialized payload when absent. This keeps the legacy wire output
/// byte-for-byte identical when the advanced fields (activity type,
/// buttons, party, secrets, instance) are unused.
#[derive(Debug, serde::Serialize)]
pub struct ActivityData {
    pub state: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timestamps: Option<Timestamps>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub assets: Option<Assets>,
    /// Discord activity type code. `0` (Playing) is the legacy default and
    /// is omitted so existing payloads are unchanged.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub r#type: Option<u64>,
    /// Interactive button labels (up to two). Discord's IPC protocol
    /// carries only the labels; URLs are configured on the application.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub buttons: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub party: Option<PartyData>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub secrets: Option<SecretsData>,
    /// Whether this is a joinable instance. Omitted when `false` so legacy
    /// payloads are unchanged.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub instance: Option<bool>,
    pub flags: u64,
}

#[derive(Debug, serde::Serialize)]
pub struct Timestamps {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub start: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub end: Option<i64>,
}

#[derive(Debug, serde::Serialize)]
pub struct Assets {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub large_text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub small_text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub large_image: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub small_image: Option<String>,
}

/// Party/lobby information for a [`RichPresence`].
///
/// `size` serializes as the two-element array `[current, max]` that
/// Discord's schema expects.
#[derive(Debug, serde::Serialize)]
pub struct PartyData {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size: Option<(u32, u32)>,
}

/// Matchmaking secrets for a [`RichPresence`].
#[derive(Debug, serde::Serialize)]
pub struct SecretsData {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub join: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub spectate: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub r#match: Option<String>,
}

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

#[derive(Debug, thiserror::Error)]
pub enum ProtocolError {
    #[error("frame too short")]
    FrameTooShort,
    #[error("frame incomplete")]
    FrameIncomplete,
    #[error("frame payload of {length} bytes exceeds the maximum of {max} bytes")]
    FrameTooLarge { length: usize, max: usize },
    #[error("unknown opcode: {0}")]
    UnknownOpcode(u32),
    #[error("JSON serialization error: {0}")]
    JsonError(#[from] serde_json::Error),
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_encode_decode_roundtrip() {
        let payload = b"hello world".to_vec();
        let frame = Frame::new(Opcode::Frame, payload.clone());
        let encoded = frame.encode();
        let decoded = Frame::decode(&encoded).unwrap();
        assert_eq!(decoded.opcode, Opcode::Frame);
        assert_eq!(decoded.payload, payload);
    }

    #[test]
    fn frame_decode_too_short() {
        let result = Frame::decode(&[0, 0, 0, 0]);
        assert!(result.is_err());
    }

    #[test]
    fn frame_decode_rejects_oversized_payload() {
        // A header claiming a payload larger than the cap must be rejected
        // as FrameTooLarge even though no payload bytes were supplied.
        let mut header = [0u8; 8];
        header[0] = 1; // opcode Frame
        let big = (MAX_FRAME_PAYLOAD_LEN + 1) as u32;
        header[4..8].copy_from_slice(&big.to_le_bytes());

        let err = Frame::decode(&header).unwrap_err();
        assert!(
            matches!(err, ProtocolError::FrameTooLarge { .. }),
            "oversized claim should fail with FrameTooLarge: {err}"
        );
    }

    #[test]
    fn frame_decode_accepts_payload_at_limit() {
        // A payload of exactly the maximum length is accepted; only lengths
        // above the cap are rejected.
        let payload = vec![0u8; MAX_FRAME_PAYLOAD_LEN];
        let frame = Frame::new(Opcode::Frame, payload);
        let encoded = frame.encode();
        let decoded = Frame::decode(&encoded).unwrap();
        assert_eq!(decoded.payload.len(), MAX_FRAME_PAYLOAD_LEN);
    }

    #[test]
    fn opcode_to_u32() {
        assert_eq!(Opcode::Handshake.to_u32(), 0);
        assert_eq!(Opcode::Frame.to_u32(), 1);
        assert_eq!(Opcode::Close.to_u32(), 2);
        assert_eq!(Opcode::Ping.to_u32(), 3);
        assert_eq!(Opcode::Pong.to_u32(), 4);
    }

    #[test]
    fn frame_decode_maps_all_protocol_opcodes() {
        for (opcode, value) in [
            (Opcode::Handshake, 0u32),
            (Opcode::Frame, 1),
            (Opcode::Close, 2),
            (Opcode::Ping, 3),
            (Opcode::Pong, 4),
        ] {
            let frame = Frame::new(opcode, vec![]);
            let decoded = Frame::decode(&frame.encode()).unwrap();
            assert_eq!(
                decoded.opcode, opcode,
                "value {} should decode to {:?}",
                value, opcode
            );
        }
    }

    #[test]
    fn frame_encode_writes_opcode_and_length_le() {
        let frame = Frame::new(Opcode::Frame, vec![0x01, 0x02, 0x03]);
        let encoded = frame.encode();
        assert_eq!(encoded.len(), 8 + 3);
        // opcode (u32 LE) == 1
        assert_eq!(
            u32::from_le_bytes([encoded[0], encoded[1], encoded[2], encoded[3]]),
            1
        );
        // length (u32 LE) == 3
        assert_eq!(
            u32::from_le_bytes([encoded[4], encoded[5], encoded[6], encoded[7]]),
            3
        );
    }

    #[test]
    fn handshake_message_serializes_per_protocol() {
        let msg = HandshakeMessage {
            version: 1,
            client_id: "123456789".to_string(),
        };
        let json = serde_json::to_value(&msg).unwrap();
        assert_eq!(json["v"], 1);
        assert_eq!(json["client_id"], "123456789");
    }

    #[test]
    fn ready_message_parses_dispatch_ready() {
        let payload = br#"{"cmd":"DISPATCH","data":{"v":1},"evt":"READY","nonce":null}"#;
        let ready: ReadyMessage = serde_json::from_slice(payload).unwrap();
        assert_eq!(ready.cmd, "DISPATCH");
        assert_eq!(ready.evt, "READY");
    }

    #[test]
    fn command_envelope_serializes_with_cmd_and_args() {
        let envelope = CommandEnvelope {
            cmd: "SET_ACTIVITY".to_string(),
            args: ActivityUpdateArgs {
                pid: 1234,
                activity: Some(ActivityData {
                    state: "Editing".to_string(),
                    details: Some("Project: song.flp".to_string()),
                    timestamps: Some(Timestamps {
                        start: Some(1000),
                        end: None,
                    }),
                    assets: Some(Assets {
                        large_text: Some("FL Studio".to_string()),
                        small_text: None,
                        large_image: Some("flstudio".to_string()),
                        small_image: None,
                    }),
                    r#type: None,
                    buttons: Vec::new(),
                    party: None,
                    secrets: None,
                    instance: None,
                    flags: 0,
                }),
            },
            nonce: "test-nonce".to_string(),
        };

        let json = serde_json::to_string(&envelope).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();

        // The command envelope must contain the cmd field and args wrapper.
        assert_eq!(parsed["cmd"], "SET_ACTIVITY");
        assert_eq!(parsed["nonce"], "test-nonce");
        assert_eq!(parsed["args"]["pid"], 1234);
        assert_eq!(parsed["args"]["activity"]["state"], "Editing");
        assert_eq!(parsed["args"]["activity"]["details"], "Project: song.flp");
        assert_eq!(parsed["args"]["activity"]["timestamps"]["start"], 1000);
        assert_eq!(
            parsed["args"]["activity"]["assets"]["large_text"],
            "FL Studio"
        );
        assert_eq!(
            parsed["args"]["activity"]["assets"]["large_image"],
            "flstudio"
        );
    }

    #[test]
    fn command_envelope_omits_null_optional_fields() {
        // Discord's schema rejects null for optional fields (e.g. timestamps.end),
        // so absent Option values must be omitted from the JSON entirely.
        let envelope = CommandEnvelope {
            cmd: "SET_ACTIVITY".to_string(),
            args: ActivityUpdateArgs {
                pid: 1234,
                activity: Some(ActivityData {
                    state: "Editing".to_string(),
                    details: None,
                    timestamps: Some(Timestamps {
                        start: Some(1000),
                        end: None,
                    }),
                    assets: Some(Assets {
                        large_text: Some("FL Studio".to_string()),
                        small_text: None,
                        large_image: Some("flstudio".to_string()),
                        small_image: None,
                    }),
                    r#type: None,
                    buttons: Vec::new(),
                    party: None,
                    secrets: None,
                    instance: None,
                    flags: 0,
                }),
            },
            nonce: "test-nonce".to_string(),
        };

        let json = serde_json::to_string(&envelope).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();

        // Absent optional fields must not serialize as null.
        assert!(
            parsed["args"]["activity"].get("details").is_none(),
            "details should be omitted when None: {}",
            json
        );
        assert!(
            parsed["args"]["activity"]["timestamps"]
                .get("end")
                .is_none(),
            "timestamps.end should be omitted when None: {}",
            json
        );
        assert!(
            parsed["args"]["activity"]["assets"]
                .get("small_text")
                .is_none(),
            "assets.small_text should be omitted when None: {}",
            json
        );
        assert!(
            parsed["args"]["activity"]["assets"]
                .get("small_image")
                .is_none(),
            "assets.small_image should be omitted when None: {}",
            json
        );

        // Present fields are still serialized.
        assert_eq!(parsed["args"]["activity"]["timestamps"]["start"], 1000);
        assert_eq!(
            parsed["args"]["activity"]["assets"]["large_text"],
            "FL Studio"
        );
    }

    #[test]
    fn command_envelope_serializes_with_null_activity_for_clear() {
        // Clearing presence sends activity: null.
        let envelope = CommandEnvelope {
            cmd: "SET_ACTIVITY".to_string(),
            args: ActivityUpdateArgs {
                pid: 1234,
                activity: None,
            },
            nonce: "clear-nonce".to_string(),
        };

        let json = serde_json::to_string(&envelope).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed["cmd"], "SET_ACTIVITY");
        assert!(parsed["args"]["activity"].is_null());
    }
}
