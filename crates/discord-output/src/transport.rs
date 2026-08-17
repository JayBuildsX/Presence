//! Discord IPC transport layer.
//!
//! Handles low-level named pipe (Windows) / Unix domain socket (Unix) communication
//! with the Discord desktop client.

use std::io::{Read, Write};

use tracing::{debug, error, warn};

use crate::protocol::{Frame, Opcode, ProtocolError, MAX_FRAME_PAYLOAD_LEN};

// ---------------------------------------------------------------------------
// Transport Error
// ---------------------------------------------------------------------------

/// Errors that can occur during transport-level operations.
#[derive(Debug, thiserror::Error)]
pub enum TransportError {
    #[error("connection failed: {0}")]
    ConnectionFailed(String),
    #[error("write failed: {0}")]
    WriteFailed(String),
    #[error("read failed: {0}")]
    ReadFailed(String),
    #[error("disconnected")]
    Disconnected,
    #[error("protocol error: {0}")]
    Protocol(#[from] ProtocolError),
}

// ---------------------------------------------------------------------------
// Platform-specific pipe types
// ---------------------------------------------------------------------------

#[cfg(windows)]
mod platform {
    use std::fs::File;
    use std::io;

    /// Opens a Windows named pipe connection.
    pub fn open_pipe(path: &str) -> io::Result<File> {
        use std::fs::OpenOptions;
        use std::os::windows::fs::OpenOptionsExt;

        OpenOptions::new()
            .read(true)
            .write(true)
            .custom_flags(0)
            .open(path)
    }
}

#[cfg(unix)]
mod platform {
    use std::fs::File;
    use std::io;

    /// Opens a Unix domain socket connection.
    pub fn open_pipe(path: &str) -> io::Result<File> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "Unix domain sockets not supported yet",
        ))
    }
}

// ---------------------------------------------------------------------------
// NamedPipeTransport
// ---------------------------------------------------------------------------

/// Best-effort summary of a received frame's payload for diagnostics.
///
/// Discord payloads are JSON objects carrying an optional `cmd` and, for
/// dispatches, an `evt`. This only extracts those two string fields; any
/// decode failure just means the fallback is logged instead.
struct FramePayloadSummary {
    cmd: Option<String>,
    evt: Option<String>,
}

impl FramePayloadSummary {
    fn from(frame: &Frame) -> Self {
        let mut summary = Self {
            cmd: None,
            evt: None,
        };
        if let Ok(json) = serde_json::from_slice::<serde_json::Value>(&frame.payload) {
            summary.cmd = json.get("cmd").and_then(|v| v.as_str()).map(str::to_owned);
            summary.evt = json.get("evt").and_then(|v| v.as_str()).map(str::to_owned);
        }
        summary
    }
}

/// Low-level transport for Discord IPC.
///
/// Owns the pipe connection and handles reading/writing bytes.
/// Knows nothing about Discord protocol semantics.
pub struct NamedPipeTransport {
    pipe: Option<std::fs::File>,
}

impl NamedPipeTransport {
    /// Create a new transport with no connection.
    pub fn new() -> Self {
        Self { pipe: None }
    }

    /// Returns whether the transport is currently connected.
    #[allow(dead_code)]
    pub fn is_connected(&self) -> bool {
        self.pipe.is_some()
    }

    /// Connect to the Discord IPC pipe.
    ///
    /// Tries pipes 0 through 9 and returns the first successful connection.
    pub fn connect(&mut self) -> Result<(), TransportError> {
        // Try pipes 0 through 9
        for i in 0..10 {
            #[cfg(windows)]
            let path = format!("\\\\.\\pipe\\discord-ipc-{}", i);
            #[cfg(not(windows))]
            let path = {
                let runtime =
                    std::env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| "/tmp".to_string());
                format!("{}/discord-ipc-{}", runtime, i)
            };

            match platform::open_pipe(&path) {
                Ok(file) => {
                    self.pipe = Some(file);
                    return Ok(());
                }
                Err(_) => continue,
            }
        }

        Err(TransportError::ConnectionFailed(
            "No Discord IPC pipe found (tried 0-9)".to_string(),
        ))
    }

    /// Disconnect from the Discord IPC pipe.
    pub fn disconnect(&mut self) {
        self.pipe.take();
    }

    /// Write a frame to the transport.
    pub fn write_frame(&mut self, frame: &Frame) -> Result<(), TransportError> {
        let pipe = self.pipe.as_mut().ok_or(TransportError::Disconnected)?;
        let data = frame.encode();
        let result = pipe.write_all(&data).map_err(|e| {
            self.disconnect();
            TransportError::WriteFailed(e.to_string())
        });
        match &result {
            Ok(()) => {
                debug!(opcode = ?frame.opcode, "Discord IPC write success");
            }
            Err(e) => {
                warn!(opcode = ?frame.opcode, error = %e, "Discord IPC write failed");
            }
        }
        result
    }

    /// Read a frame from the transport.
    pub fn read_frame(&mut self) -> Result<Frame, TransportError> {
        let pipe = self.pipe.as_mut().ok_or(TransportError::Disconnected)?;

        // Read the 8-byte header
        let mut header = [0u8; 8];
        match pipe.read_exact(&mut header) {
            Ok(_) => {}
            Err(e) => {
                self.disconnect();
                return Err(TransportError::ReadFailed(e.to_string()));
            }
        }

        // Decode the header to get frame length
        let length = u32::from_le_bytes([header[4], header[5], header[6], header[7]]) as usize;

        // Reject oversized frames before allocating the payload so a corrupt
        // or hostile peer cannot force a huge allocation. The pipe is torn
        // down because the unread frame would otherwise misalign all reads.
        if length > MAX_FRAME_PAYLOAD_LEN {
            self.disconnect();
            return Err(TransportError::Protocol(ProtocolError::FrameTooLarge {
                length,
                max: MAX_FRAME_PAYLOAD_LEN,
            }));
        }

        // Read the payload
        let mut payload = vec![0u8; length];
        match pipe.read_exact(&mut payload) {
            Ok(_) => {}
            Err(e) => {
                self.disconnect();
                return Err(TransportError::ReadFailed(e.to_string()));
            }
        }

        // Reconstruct the full frame
        let mut full_data = header.to_vec();
        full_data.extend_from_slice(&payload);
        let frame = Frame::decode(&full_data).map_err(TransportError::Protocol)?;

        // Diagnostic logging for every received frame. This surfaces READY,
        // ERROR, CLOSE, and any response frames so we can trace the pipeline.
        Self::log_received_frame(&frame);
        Ok(frame)
    }

    /// Read a frame if a complete one is already buffered, without blocking
    /// when the pipe is idle.
    ///
    /// Used to drain the pipe after the handshake: Discord sends periodic
    /// `PING` frames and can send `CLOSE`/`ERROR` frames at any time, and
    /// those must be consumed without ever blocking the publish path.
    pub fn try_read_frame(&mut self) -> Result<Option<Frame>, TransportError> {
        // How many bytes are already buffered? Peeking is non-consuming.
        let available = match Self::bytes_available(&self.pipe) {
            Some(n) => n,
            None => {
                self.disconnect();
                return Err(TransportError::ReadFailed("pipe peek failed".to_string()));
            }
        };

        if available < 8 {
            // Nothing complete (or nothing at all) to read.
            return Ok(None);
        }

        // Peek the 8-byte header (without consuming) to learn the frame length.
        let header = match Self::peek_bytes(&self.pipe, 8) {
            Some(bytes) => {
                let mut header = [0u8; 8];
                header.copy_from_slice(&bytes[..8]);
                header
            }
            None => return Ok(None),
        };
        let length = u32::from_le_bytes([header[4], header[5], header[6], header[7]]) as usize;

        // Reject oversized frames from the non-blocking path too, before any
        // comparison against the buffered byte count. The transport is torn
        // down so the unread frame cannot misalign subsequent reads.
        if length > MAX_FRAME_PAYLOAD_LEN {
            self.disconnect();
            return Err(TransportError::Protocol(ProtocolError::FrameTooLarge {
                length,
                max: MAX_FRAME_PAYLOAD_LEN,
            }));
        }

        let total = 8 + length;

        if (available as usize) < total {
            // The frame is still being written; don't block waiting for it.
            return Ok(None);
        }

        // A complete frame is buffered; consume it with the blocking reader.
        // Both read_exact calls return immediately because the bytes are
        // already available, so this does not block.
        self.read_frame().map(Some)
    }

    /// Number of bytes buffered in the pipe, without consuming them.
    ///
    /// Windows named pipes expose a non-consuming `PeekNamedPipe`; a closed
    /// or errored pipe yields `None`.
    #[cfg(windows)]
    fn bytes_available(pipe: &Option<std::fs::File>) -> Option<u32> {
        use std::os::windows::io::AsRawHandle;
        use winapi::um::namedpipeapi::PeekNamedPipe;

        let pipe = pipe.as_ref()?;
        unsafe {
            let mut total: u32 = 0;
            let mut remaining: u32 = 0;
            let ok = PeekNamedPipe(
                pipe.as_raw_handle() as *mut winapi::ctypes::c_void,
                std::ptr::null_mut(),
                0,
                std::ptr::null_mut(),
                &mut total,
                &mut remaining,
            );
            if ok == 0 {
                None
            } else {
                Some(total)
            }
        }
    }

    /// Non-Windows fallback: Unix sockets are not supported yet, so the drain
    /// is a no-op rather than blocking.
    #[cfg(not(windows))]
    fn bytes_available(_pipe: &Option<std::fs::File>) -> Option<u32> {
        Some(0)
    }

    /// Peeks up to `n` bytes without consuming them.
    #[cfg(windows)]
    fn peek_bytes(pipe: &Option<std::fs::File>, n: usize) -> Option<Vec<u8>> {
        use std::os::windows::io::AsRawHandle;
        use winapi::um::namedpipeapi::PeekNamedPipe;

        let pipe = pipe.as_ref()?;
        let mut buf = vec![0u8; n];
        unsafe {
            let mut bytes_read: u32 = 0;
            let ok = PeekNamedPipe(
                pipe.as_raw_handle() as *mut winapi::ctypes::c_void,
                buf.as_mut_ptr() as *mut _,
                n as u32,
                &mut bytes_read,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            );
            if ok == 0 {
                return None;
            }
            buf.truncate(bytes_read as usize);
        }
        Some(buf)
    }

    /// Non-Windows fallback for [`Self::peek_bytes`].
    #[cfg(not(windows))]
    fn peek_bytes(_pipe: &Option<std::fs::File>, _n: usize) -> Option<Vec<u8>> {
        None
    }

    /// Emit diagnostic logs describing a received IPC frame.
    pub(crate) fn log_received_frame(frame: &Frame) {
        match frame.opcode {
            Opcode::Close => {
                warn!(opcode = ?frame.opcode, "Discord IPC CLOSE frame received (possibly rejecting presence)");
            }
            _ => {
                // Attempt to surface any command/event fields in the payload.
                let summary = FramePayloadSummary::from(frame);
                if let Some(ev) = &summary.evt {
                    match ev.as_str() {
                        "READY" => {
                            debug!(cmd = summary.cmd, evt = %ev, "Discord IPC READY received");
                        }
                        "ERROR" => {
                            error!(cmd = summary.cmd, evt = %ev, "Discord IPC ERROR frame received");
                        }
                        other => {
                            debug!(cmd = summary.cmd, evt = %other, "Discord IPC frame received");
                        }
                    }
                } else if let Some(cmd) = &summary.cmd {
                    if cmd == "ERROR" {
                        error!(cmd = %cmd, "Discord IPC ERROR frame received");
                    } else {
                        debug!(cmd = %cmd, "Discord IPC frame received");
                    }
                } else {
                    debug!(opcode = ?frame.opcode, "Discord IPC frame received");
                }
            }
        }
    }

    /// Reconnect by closing the existing connection and creating a new one.
    #[allow(dead_code)]
    pub fn reconnect(&mut self) -> Result<(), TransportError> {
        self.disconnect();
        self.connect()
    }
}

impl Default for NamedPipeTransport {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Test support: local Discord-protocol pipe pair
// ---------------------------------------------------------------------------

/// Helpers for exercising the transport and client against a real named pipe
/// without a running Discord.
#[cfg(test)]
pub(crate) mod test_support {
    use std::io::{Read, Write};
    use std::os::windows::ffi::OsStrExt;
    use std::os::windows::io::FromRawHandle;

    use super::*;

    /// Server end of a local pipe pair, owned by the test.
    pub(crate) struct TestServerPipe {
        file: std::fs::File,
    }

    impl TestServerPipe {
        pub(crate) fn write_frame(&self, frame: &Frame) {
            self.write_bytes(&frame.encode());
        }

        /// Writes raw bytes to the pipe without any frame validation, so
        /// tests can feed malformed or oversized headers.
        pub(crate) fn write_bytes(&self, bytes: &[u8]) {
            let mut file = &self.file;
            file.write_all(bytes).unwrap();
        }

        pub(crate) fn read_frame(&self) -> Frame {
            let mut file = &self.file;
            let mut header = [0u8; 8];
            file.read_exact(&mut header).unwrap();
            let length = u32::from_le_bytes([header[4], header[5], header[6], header[7]]) as usize;
            let mut payload = vec![0u8; length];
            file.read_exact(&mut payload).unwrap();
            let mut full = header.to_vec();
            full.extend_from_slice(&payload);
            Frame::decode(&full).unwrap()
        }
    }

    /// Creates a local `\\.\pipe\<name>` pair and returns a transport attached
    /// to the client end plus the server end. Deterministic and offline — no
    /// Discord required.
    pub(crate) fn create_test_pipe(name: &str) -> (NamedPipeTransport, TestServerPipe) {
        use winapi::um::namedpipeapi::{ConnectNamedPipe, CreateNamedPipeW};
        use winapi::um::winbase::FILE_FLAG_FIRST_PIPE_INSTANCE;
        use winapi::um::winbase::{
            PIPE_ACCESS_DUPLEX, PIPE_READMODE_BYTE, PIPE_TYPE_BYTE, PIPE_WAIT,
        };

        let path: Vec<u16> = std::ffi::OsString::from(format!("\\\\.\\pipe\\{name}"))
            .encode_wide()
            .chain(Some(0))
            .collect();
        let server = unsafe {
            CreateNamedPipeW(
                path.as_ptr(),
                PIPE_ACCESS_DUPLEX | FILE_FLAG_FIRST_PIPE_INSTANCE,
                PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT,
                1,
                4096,
                4096,
                0,
                std::ptr::null_mut(),
            )
        };
        assert!(
            !server.is_null(),
            "CreateNamedPipeW failed: {:?}",
            std::io::Error::last_os_error()
        );

        // Accept the connection on a helper thread: on a blocking pipe the
        // client's CreateFile waits until the server calls ConnectNamedPipe.
        // (The raw HANDLE is not Send, so it crosses the thread boundary as a
        // usize and is cast back.)
        let server_for_thread = server as usize;
        let connect_server = std::thread::spawn(move || unsafe {
            let ok = ConnectNamedPipe(
                server_for_thread as *mut winapi::ctypes::c_void,
                std::ptr::null_mut(),
            );
            if ok == 0 {
                let err = std::io::Error::last_os_error();
                // ERROR_PIPE_CONNECTED (535): the client beat us to it.
                assert!(
                    err.raw_os_error() == Some(535),
                    "ConnectNamedPipe failed: {err}"
                );
            }
        });

        let mut transport = NamedPipeTransport::new();
        let file = platform::open_pipe(&format!("\\\\.\\pipe\\{name}"))
            .expect("client connects to test pipe");
        transport.pipe = Some(file);

        connect_server.join().unwrap();

        let server_file =
            unsafe { std::fs::File::from_raw_handle(server as std::os::windows::io::RawHandle) };
        (transport, TestServerPipe { file: server_file })
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transport_initial_state() {
        let transport = NamedPipeTransport::new();
        assert!(!transport.is_connected());
    }

    #[test]
    fn transport_disconnect_without_connection() {
        let mut transport = NamedPipeTransport::new();
        transport.disconnect(); // Should not panic
        assert!(!transport.is_connected());
    }

    #[test]
    fn transport_write_without_connection() {
        let mut transport = NamedPipeTransport::new();
        let frame = Frame::new(crate::protocol::Opcode::Frame, vec![]);
        let result = transport.write_frame(&frame);
        assert!(result.is_err());
    }

    #[test]
    fn try_read_frame_without_connection_is_disconnected_error() {
        let mut transport = NamedPipeTransport::new();
        assert!(transport.try_read_frame().is_err());
    }

    #[cfg(windows)]
    #[test]
    fn try_read_frame_returns_none_when_pipe_idle() {
        let name = format!("ph-presencehub-idle-{}", std::process::id());
        let (mut transport, _server) = test_support::create_test_pipe(&name);
        // Nothing has been written: the drain must return Ok(None), not block.
        assert!(transport.try_read_frame().unwrap().is_none());
    }

    #[cfg(windows)]
    #[test]
    fn try_read_frame_reads_complete_buffered_frame() {
        let name = format!("ph-presencehub-frame-{}", std::process::id());
        let (mut transport, server) = test_support::create_test_pipe(&name);
        let sent = Frame::new(
            crate::protocol::Opcode::Frame,
            br#"{"cmd":"DISPATCH","evt":"READY"}"#.to_vec(),
        );
        server.write_frame(&sent);

        let frame = transport
            .try_read_frame()
            .unwrap()
            .expect("complete frame is buffered");
        assert_eq!(frame.opcode, sent.opcode);
        assert_eq!(frame.payload, sent.payload);
    }

    // -- inbound frame size limit ------------------------------------------------

    #[cfg(windows)]
    fn oversized_header() -> [u8; 8] {
        let mut header = [0u8; 8];
        header[0] = 1; // opcode Frame
        let big = (MAX_FRAME_PAYLOAD_LEN + 1) as u32;
        header[4..8].copy_from_slice(&big.to_le_bytes());
        header
    }

    #[cfg(windows)]
    #[test]
    fn try_read_frame_rejects_oversized_frame() {
        let name = format!("ph-presencehub-oversize-{}", std::process::id());
        let (mut transport, server) = test_support::create_test_pipe(&name);

        // Feed a header claiming a payload larger than the cap (no payload).
        server.write_bytes(&oversized_header());

        let err = transport.try_read_frame().unwrap_err();
        assert!(
            matches!(
                err,
                TransportError::Protocol(ProtocolError::FrameTooLarge { .. })
            ),
            "oversized frame should fail with FrameTooLarge: {err}"
        );
        assert!(
            !transport.is_connected(),
            "oversized frame must tear down the transport"
        );
    }

    #[cfg(windows)]
    #[test]
    fn read_frame_rejects_oversized_frame() {
        let name = format!("ph-presencehub-oversize-block-{}", std::process::id());
        let (mut transport, server) = test_support::create_test_pipe(&name);

        server.write_bytes(&oversized_header());

        let err = transport.read_frame().unwrap_err();
        assert!(
            matches!(
                err,
                TransportError::Protocol(ProtocolError::FrameTooLarge { .. })
            ),
            "oversized frame should fail with FrameTooLarge: {err}"
        );
        assert!(
            !transport.is_connected(),
            "oversized frame must tear down the transport"
        );
    }
}
