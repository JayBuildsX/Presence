//! PresenceHub Discord Rich Presence output.
//!
//! This crate publishes [`Activity`] objects to Discord Rich Presence
//! using a native implementation of the Discord IPC protocol.
//!
//! # Architecture
//!
//! ```text
//! Activity
//!     │  (generic conversion, presencehub-core)
//!     ▼
//! RichPresence
//!     │  (render::to_activity_data)
//!     ▼
//! DiscordOutput
//!     │
//!     ▼
//! DiscordClient
//!     │
//!     ▼
//! NamedPipeTransport
//!     │
//!     ▼
//! Discord Desktop
//! ```
//!
//! DiscordOutput is responsible only for:
//! - Rendering a [`RichPresence`] into a Discord packet
//! - Delegating to DiscordClient
//!
//! No protocol code and no plugin-specific logic exists here.
//!
//! # Application identity limitation
//!
//! Discord renders the first presence line as **"Playing \<name\>"** where
//! `<name>` is the *registered name* of the Discord Developer Application
//! whose Application ID was sent in the IPC handshake
//! ([`DiscordClient::new`](client::DiscordClient) / the `client_id` in the
//! handshake frame). The Rich Presence protocol has **no command or field to
//! change that application name at runtime** — the `Activity.application`
//! value this crate receives maps only to `assets.large_text` (the hover
//! text over the large image), never to the "Playing ..." line.
//!
//! Consequence: one Discord connection can only ever display one application
//! name. A plugin cannot make PresenceHub's single connected application
//! appear as "Antigravity" one moment and "FL Studio" the next.
//!
//! ## Production solution
//!
//! Per-plugin application identity requires a separate Discord application
//! per game (each registered with that game's name and its own asset keys)
//! and selecting the matching client ID *before* connecting — reconnecting
//! when the active plugin changes. That means:
//!
//! - one Discord application / client ID per plugin (e.g. "Antigravity",
//!   "FL Studio"), each with its own application ID;
//! - the configuration carrying a plugin-specific Discord application ID;
//! - the output selecting the current client ID and reconnecting when the
//!   active plugin changes.
//!
//! The current architecture uses a single global `discord_app_id`, so this
//! is **not** implemented here. Do not fake it by moving the application
//! name into `state`/`details`: Discord would show the app name twice
//! ("Playing PresenceHub" / "Antigravity • Dynamic Conversation Tracking").

mod client;
mod protocol;
mod render;
mod transport;

/// Serializes environment-dependent tests that open the single real Discord
/// IPC pipe. Running several of them concurrently races for pipe 0, which
/// produces spurious "No Discord IPC pipe found" failures.
#[cfg(test)]
pub(crate) static DISCORD_PIPE_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

use std::collections::HashMap;

use presencehub_core::{
    activity::Activity, output::Output, output::OutputError, presence::RichPresence,
};
use tracing::{debug, info, warn};

use client::DiscordClient;
use render::to_activity_data;

// ---------------------------------------------------------------------------
// Discord Output
// ---------------------------------------------------------------------------

/// Publishes activities to Discord Rich Presence.
///
/// Uses a native Discord IPC implementation.
/// Connection is established lazily on first publish.
/// Reconnects automatically if Discord restarts.
///
/// DiscordOutput is a **renderer** of the generic [`RichPresence`] domain
/// model: it maps a presence onto the Discord protocol payload and sends it.
/// It never decides what the presence contains — plugins express that
/// through the typed model, and the generic `Activity → RichPresence`
/// conversion in `presencehub-core` bridges the canonical [`Activity`] model
/// to it. Adding a new application therefore requires no changes here.
///
/// ## Activity Mapping
///
/// | Activity Field | Rich Presence Field | Discord Field |
/// |---|---|---|
/// | `state` | `state` | `state` |
/// | `details` | `details` | `details` |
/// | `timestamps.start` | `timestamps.start` | `timestamps.start` |
/// | `timestamps.end` | `timestamps.end` | `timestamps.end` |
/// | `metadata["application"]` | `assets.large_text` | `assets.large_text` |
/// | `metadata["version"]` | `assets.small_text` | `assets.small_text` |
///
/// The large image is set to a lowercase slug of the application name
/// (e.g. "FL Studio" → "flstudio"). This is a placeholder until real
/// asset keys are uploaded to the Discord application.
pub struct DiscordOutput {
    client: Option<DiscordClient>,
    /// Discord application ID used when the published source has no entry in
    /// [`Self::app_ids`].
    default_app_id: u64,
    /// Per-source Discord application IDs keyed by the engine source string.
    app_ids: HashMap<String, u64>,
    /// The application ID of the connection currently (or last) established.
    /// `None` means no application has been selected yet.
    current_app_id: Option<u64>,
}

impl DiscordOutput {
    /// Creates a new Discord Output with a default application ID and an
    /// optional per-source application ID map.
    ///
    /// A resolved application ID of `0` disables the output for that source.
    pub fn new(default_app_id: u64, app_ids: HashMap<String, u64>) -> Self {
        Self {
            client: None,
            default_app_id,
            app_ids,
            current_app_id: None,
        }
    }

    /// Returns whether this output is currently connected to Discord.
    pub fn is_connected(&self) -> bool {
        self.client.as_ref().is_some_and(|c| c.is_connected())
    }

    /// Resolve the Discord application ID for a source.
    ///
    /// Prefers the per-source entry, falling back to the default application
    /// ID. The engine source identity is authoritative — never derived from
    /// the activity's display metadata.
    fn resolve_app_id(&self, source: &str) -> u64 {
        self.app_ids
            .get(source)
            .copied()
            .unwrap_or(self.default_app_id)
    }

    /// Ensure the client is connected.
    ///
    /// If the client is `None` (never connected, or dropped after a
    /// failure), a fresh client is created for the current application ID
    /// and connected. This enables automatic reconnection when Discord
    /// restarts.
    fn ensure_client(&mut self) -> Result<&mut DiscordClient, OutputError> {
        if self.client.is_none() {
            let application_id = self.current_app_id.unwrap_or(self.default_app_id);
            let mut client = DiscordClient::new(application_id);
            client.connect().map_err(|e| {
                warn!(error = %e, "Discord connection failed");
                OutputError::PublishFailed(format!("Discord connection failed: {}", e))
            })?;
            info!(app_id = application_id, "Discord connected");
            self.client = Some(client);
        }
        Ok(self.client.as_mut().unwrap())
    }

    /// Drop the client, forcing a fresh connection on the next publish.
    ///
    /// Called when a publish fails due to a lost connection, and when
    /// [`Self::clear`] finishes sending the activity clear. The next
    /// `publish` creates a new client and reconnects with the current
    /// application ID.
    fn drop_client(&mut self) {
        if let Some(client) = self.client.take() {
            drop(client);
            warn!("Discord disconnected");
        }
    }

    /// Render a [`RichPresence`] to Discord and send it.
    ///
    /// This is the renderer entry point. It maps the generic presence onto
    /// the Discord protocol payload and delegates to the client. It knows
    /// nothing about plugins or application-specific data.
    pub fn set_presence(&mut self, presence: &RichPresence) -> Result<(), OutputError> {
        let client = self.ensure_client()?;

        let activity_data = to_activity_data(presence);

        match client.set_activity(activity_data) {
            Ok(()) => {
                debug!(state = %presence.state.as_deref().unwrap_or_default(), "Published Rich Presence");
                Ok(())
            }
            Err(e) => {
                // Connection lost — drop the client so the next publish reconnects.
                warn!(error = %e, "Discord publish failed, will reconnect");
                self.drop_client();
                Err(OutputError::PublishFailed(format!(
                    "Discord publish failed: {}",
                    e
                )))
            }
        }
    }
}

impl Output for DiscordOutput {
    fn publish(&mut self, source: &str, activity: &Activity) -> Result<(), OutputError> {
        let resolved_app_id = self.resolve_app_id(source);

        info!(
            source = %source,
            app_id = resolved_app_id,
            state = %activity.state,
            details = ?activity.details,
            metadata = ?activity.metadata,
            "DiscordOutput::publish executed"
        );

        // A resolved application ID of 0 means this source is not configured
        // for Discord: drop any connection and remain idle.
        if resolved_app_id == 0 {
            self.drop_client();
            self.current_app_id = None;
            return Ok(());
        }

        // Switch Discord applications only when the resolved application ID
        // changed. Repeated publishes for the same source reuse the existing
        // connection and never reconnect.
        if self.current_app_id != Some(resolved_app_id) {
            self.drop_client();
            self.current_app_id = Some(resolved_app_id);
        }

        self.ensure_client()?;

        // Bridge the canonical Activity model to the Rich Presence domain
        // model, then render it. DiscordOutput never inspects activity
        // metadata itself.
        let presence = RichPresence::from(activity);
        self.set_presence(&presence)
    }

    /// Clear the Rich Presence activity.
    ///
    /// Tells Discord to remove the current presence. This overrides the
    /// `Output::clear` default no-op so it is invoked through dynamic
    /// dispatch when [`PresenceEngine::end_session`] or
    /// [`PresenceEngine::clear`] runs. Called when the session ends
    /// (e.g. FL Studio closes) or the runtime shuts down, so no stale
    /// presence remains.
    ///
    /// After the clear is sent, the connection is dropped. This is a no-op
    /// if Discord is not connected, and it is idempotent — a second call
    /// after the client was dropped has nothing to clear.
    fn clear(&mut self) {
        // No client (or one already dropped by a previous clear) means there
        // is nothing to clear — safe no-op, and idempotent across repeated
        // shutdown calls.
        let result = match self.client.as_mut() {
            Some(client) => client.clear_activity(),
            None => return,
        };
        match result {
            Ok(()) => {
                info!("Cleared Rich Presence");
                self.drop_client();
            }
            Err(e) => {
                warn!(error = %e, "Failed to clear Rich Presence");
                self.drop_client();
            }
        }
    }

    fn connection_state(&self) -> Option<bool> {
        Some(self.is_connected())
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
    fn discord_output_implements_output_trait() {
        fn takes_output(_output: Box<dyn Output>) {}
        takes_output(Box::new(DiscordOutput::new(123456789, HashMap::new())));
    }

    #[test]
    fn discord_output_creation() {
        let output = DiscordOutput::new(123456789, HashMap::new());
        assert!(!output.is_connected());
    }

    #[test]
    fn discord_output_publish_succeeds_when_discord_running() {
        // Environment-dependent: only meaningful when Discord is running locally.
        let _guard = crate::DISCORD_PIPE_TEST_LOCK.lock().unwrap();
        if !discord_pipe_available() {
            return;
        }
        let mut output = DiscordOutput::new(1533559059125637311, HashMap::new());
        let activity = Activity {
            state: "Editing".to_string(),
            details: Some("Project: song.flp".to_string()),
            timestamps: None,
            metadata: {
                let mut m = std::collections::HashMap::new();
                m.insert("application".to_string(), "FL Studio".to_string());
                m
            },
            application: None,
        };
        let result = output.publish("FL Studio", &activity);
        assert!(
            result.is_ok(),
            "publish should succeed against a live Discord: {:?}",
            result
        );
        assert!(output.is_connected());
    }

    #[test]
    fn discord_output_publish_fails_when_discord_not_running() {
        // Environment-dependent: skips when Discord is running locally.
        let _guard = crate::DISCORD_PIPE_TEST_LOCK.lock().unwrap();
        if discord_pipe_available() {
            return;
        }
        let mut output = DiscordOutput::new(123456789, HashMap::new());
        let activity = Activity {
            state: "Editing".to_string(),
            details: Some("song.flp".to_string()),
            timestamps: None,
            metadata: {
                let mut m = std::collections::HashMap::new();
                m.insert("application".to_string(), "FL Studio".to_string());
                m
            },
            application: None,
        };

        let result = output.publish("FL Studio", &activity);
        // Discord is not running in CI, so this should fail gracefully.
        assert!(result.is_err());
        // The client should be dropped so the next publish reconnects.
        assert!(output.client.is_none());
    }

    #[test]
    fn discord_output_clear_when_not_connected_is_noop() {
        let mut output = DiscordOutput::new(123456789, HashMap::new());
        // Not connected — clearing should not panic or error.
        output.clear();
        assert!(output.client.is_none());
    }

    #[test]
    fn clear_drops_connected_client_after_clearing() {
        // Graceful shutdown: clear() sends the activity clear, then drops
        // the connection so no IPC state survives the process exit.
        let mut output = DiscordOutput {
            client: Some(DiscordClient::new(123456789)),
            default_app_id: 123456789,
            app_ids: HashMap::new(),
            current_app_id: Some(123456789),
        };
        assert!(output.client.is_some());
        output.clear();
        assert!(
            output.client.is_none(),
            "clear() must drop the client after sending the activity clear"
        );
        // The per-source application ID selection is preserved for a later
        // session: the next publish reconnects with the same App ID.
        assert_eq!(output.current_app_id, Some(123456789));
    }

    #[test]
    fn clear_is_idempotent_when_called_twice() {
        // Calling clear() repeatedly (e.g. shutdown() invoked twice) must
        // not panic and must not perform a second IPC operation once the
        // client was dropped.
        let mut output = DiscordOutput {
            client: Some(DiscordClient::new(123456789)),
            default_app_id: 123456789,
            app_ids: HashMap::new(),
            current_app_id: Some(123456789),
        };
        output.clear();
        output.clear();
        assert!(output.client.is_none());
        assert_eq!(output.current_app_id, Some(123456789));
    }

    #[test]
    fn end_session_dispatches_discord_clear_override() {
        // Regression: PresenceEngine::end_session() must invoke
        // DiscordOutput::clear() through dynamic dispatch (the Output::clear
        // override), not the default no-op.
        //
        // This fails on the previous implementation, where clear() existed only
        // as an inherent method and the Output trait used its default no-op.
        //
        // The output is constructed with a (disconnected but present) client so
        // the override's clear path is exercised: client.clear_activity() on a
        // non-connected client returns Ok and logs "Cleared Rich Presence".
        use std::sync::Arc;
        use std::sync::Mutex;

        struct CaptureWriter(Arc<Mutex<String>>);
        impl std::io::Write for CaptureWriter {
            fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
                self.0
                    .lock()
                    .unwrap()
                    .push_str(&String::from_utf8_lossy(buf));
                Ok(buf.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for CaptureWriter {
            type Writer = CaptureWriter;
            fn make_writer(&'a self) -> Self::Writer {
                CaptureWriter(self.0.clone())
            }
        }

        let buf = Arc::new(Mutex::new(String::new()));
        let writer = CaptureWriter(buf.clone());
        let subscriber = tracing_subscriber::fmt()
            .with_writer(writer)
            .with_ansi(false)
            .with_max_level(tracing::Level::INFO)
            .finish();

        tracing::subscriber::with_default(subscriber, || {
            let mut engine = presencehub_core::output::PresenceEngine::new();

            // Start a session without any outputs registered (no publish, so
            // the fake client below is never touched or dropped).
            let activity = Activity {
                state: "Idle".to_string(),
                details: None,
                timestamps: None,
                metadata: std::collections::HashMap::new(),
                application: None,
            };
            let errors = engine.update("test", &activity);
            assert!(errors.is_empty());

            // Register a DiscordOutput with a present (but not connected)
            // client, so the override's clear path runs: client.clear_activity()
            // on a non-connected client returns Ok and logs "Cleared Rich
            // Presence". The default no-op logs nothing.
            let output = DiscordOutput {
                client: Some(DiscordClient::new(123456789)),
                default_app_id: 123456789,
                app_ids: HashMap::new(),
                current_app_id: None,
            };
            engine.register_output(Box::new(output));

            engine.end_session("test");
        });

        let logs = buf.lock().unwrap().clone();
        assert!(
            logs.contains("Cleared Rich Presence"),
            "end_session() should dispatch the DiscordOutput::clear() override; logs were: {}",
            logs
        );
    }

    #[test]
    fn discord_output_activity_mapping() {
        let mut metadata = std::collections::HashMap::new();
        metadata.insert("application".to_string(), "FL Studio".to_string());
        metadata.insert("version".to_string(), "21".to_string());

        let activity = Activity {
            state: "Editing".to_string(),
            details: Some("Project: song.flp*".to_string()),
            timestamps: Some(presencehub_core::activity::ActivityTimestamps {
                start: Some(1000),
                end: Some(2000),
            }),
            metadata,
            application: None,
        };

        // The publish path: Activity → RichPresence → Discord ActivityData.
        let presence = RichPresence::from(&activity);
        let data = to_activity_data(&presence);
        assert_eq!(data.state, "Editing");
        assert_eq!(data.details, Some("Project: song.flp*".to_string()));
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
    fn discord_output_receives_session_start_timestamp() {
        // Verify that a session start timestamp (as stamped by the PresenceEngine)
        // is correctly forwarded to Discord's native timestamp field.
        let session_start_epoch: i64 = 1_700_000_000; // example session start

        let activity = Activity {
            state: "Editing".to_string(),
            details: Some("Project: Horizon.flp".to_string()),
            timestamps: Some(presencehub_core::activity::ActivityTimestamps {
                start: Some(session_start_epoch),
                end: None,
            }),
            metadata: {
                let mut m = std::collections::HashMap::new();
                m.insert("application".to_string(), "FL Studio".to_string());
                m
            },
            application: None,
        };

        // Map the activity exactly as DiscordOutput::publish does.
        let presence = RichPresence::from(&activity);
        let data = to_activity_data(&presence);

        // Discord's native timestamp support uses the start field to render
        // "Started X minutes ago" / "Elapsed: X hours".
        assert_eq!(
            data.timestamps.as_ref().unwrap().start,
            Some(session_start_epoch),
            "Discord must receive the session start timestamp"
        );
        assert!(data.timestamps.as_ref().unwrap().end.is_none());
    }

    #[test]
    fn discord_output_large_image_slug() {
        // Verify the large image slug generation for the application name.
        let app = "FL Studio";
        let slug = app.to_lowercase().replace(' ', "");
        assert_eq!(slug, "flstudio");
    }

    // -- Per-source application ID resolution ------------------------------------

    /// A minimal activity for a given state.
    fn test_activity(state: &str) -> Activity {
        Activity {
            state: state.to_string(),
            details: None,
            timestamps: None,
            application: None,
            metadata: HashMap::new(),
        }
    }

    /// The per-source map used by the switch tests: Antigravity → 200,
    /// FL Studio → 300, default 100.
    fn per_source_app_ids() -> HashMap<String, u64> {
        HashMap::from([
            ("Antigravity".to_string(), 200),
            ("FL Studio".to_string(), 300),
        ])
    }

    #[test]
    fn resolve_app_id_uses_source_map_then_default() {
        let output = DiscordOutput::new(100, per_source_app_ids());
        assert_eq!(output.resolve_app_id("Antigravity"), 200);
        assert_eq!(output.resolve_app_id("FL Studio"), 300);
        assert_eq!(output.resolve_app_id("Unknown Source"), 100);
    }

    #[test]
    fn resolve_app_id_falls_back_to_default_without_map() {
        // Backward compatibility at the output level: an empty per-source map
        // resolves every source to the default application ID.
        let output = DiscordOutput::new(1533559059125637311, HashMap::new());
        assert_eq!(output.resolve_app_id("Antigravity"), 1533559059125637311);
        assert_eq!(output.resolve_app_id("FL Studio"), 1533559059125637311);
    }

    #[test]
    fn publish_resolves_zero_app_id_to_noop() {
        // A resolved application ID of 0 must be a no-op: no connection is
        // attempted and no application is selected.
        let mut output = DiscordOutput::new(0, HashMap::new());
        let result = output.publish("Unknown Source", &test_activity("Idle"));
        assert!(result.is_ok(), "zero app id must no-op, not fail");
        assert!(
            output.client.is_none(),
            "no connection attempt for zero app id"
        );
        assert_eq!(output.current_app_id, None);
    }

    #[test]
    fn publish_resolves_per_source_app_id_while_connected() {
        // Publishing a mapped source selects that source's application ID.
        let mut output = DiscordOutput::new(100, per_source_app_ids());
        // Discord not running locally → publish fails, but the resolved app id
        // is selected before the connection attempt.
        let _ = output.publish("Antigravity", &test_activity("Coding"));
        assert_eq!(output.current_app_id, Some(200));
    }

    // -- Application switching ---------------------------------------------------

    #[test]
    fn publish_switches_app_id_antigravity_to_flstudio() {
        // Antigravity (200) is the current connection. Publishing FL Studio (300)
        // must drop the old client and select the new application ID.
        let mut output = DiscordOutput {
            client: Some(DiscordClient::new(200)),
            default_app_id: 100,
            app_ids: per_source_app_ids(),
            current_app_id: Some(200),
        };

        let _ = output.publish("FL Studio", &test_activity("Editing"));

        assert!(
            output.client.is_none(),
            "old client must be dropped when the application switches"
        );
        assert_eq!(
            output.current_app_id,
            Some(300),
            "new application ID selected"
        );
    }

    #[test]
    fn publish_switches_app_id_flstudio_to_antigravity() {
        // The reverse transition.
        let mut output = DiscordOutput {
            client: Some(DiscordClient::new(300)),
            default_app_id: 100,
            app_ids: per_source_app_ids(),
            current_app_id: Some(300),
        };

        let _ = output.publish("Antigravity", &test_activity("Coding"));

        assert!(
            output.client.is_none(),
            "old client must be dropped when the application switches"
        );
        assert_eq!(
            output.current_app_id,
            Some(200),
            "new application ID selected"
        );
    }

    #[test]
    fn publish_repeated_same_source_keeps_app_id() {
        // Repeated publishes for the same source must never change the
        // selected application ID (the reconnect gate must not fire).
        let mut output = DiscordOutput::new(100, per_source_app_ids());

        let _ = output.publish("Antigravity", &test_activity("Coding"));
        assert_eq!(output.current_app_id, Some(200));

        let _ = output.publish("Antigravity", &test_activity("Coding 2"));
        assert_eq!(
            output.current_app_id,
            Some(200),
            "same source must not re-select a different application ID"
        );

        let _ = output.publish("Antigravity", &test_activity("Coding 3"));
        assert_eq!(output.current_app_id, Some(200));
    }

    #[test]
    fn publish_repeated_same_source_keeps_single_connection_when_discord_running() {
        // Environment-dependent: only meaningful when Discord is running
        // locally. Confirms the client is not recreated across repeated
        // same-source publishes.
        let _guard = crate::DISCORD_PIPE_TEST_LOCK.lock().unwrap();
        if !discord_pipe_available() {
            return;
        }
        let mut output = DiscordOutput::new(1533559059125637311, HashMap::new());

        assert!(output
            .publish("FL Studio", &test_activity("Editing"))
            .is_ok());
        assert!(output.is_connected());
        let original_client = output.client.as_ref().map(|c| c as *const DiscordClient);

        assert!(output
            .publish("FL Studio", &test_activity("Editing 2"))
            .is_ok());
        assert!(output.is_connected());
        assert_eq!(
            output.client.as_ref().map(|c| c as *const DiscordClient),
            original_client,
            "the same client object must be reused, never recreated"
        );
        assert_eq!(output.current_app_id, Some(1533559059125637311));
    }

    #[test]
    fn publish_switch_while_disconnected_uses_new_sources_app_id() {
        // No client exists (disconnected). Publishing Antigravity selects 200;
        // switching source still disconnected selects 300 without any prior
        // connection to tear down.
        let mut output = DiscordOutput::new(100, per_source_app_ids());

        let _ = output.publish("Antigravity", &test_activity("Coding"));
        assert_eq!(output.current_app_id, Some(200));
        assert!(output.client.is_none());

        let _ = output.publish("FL Studio", &test_activity("Editing"));
        assert_eq!(output.current_app_id, Some(300));
        assert!(output.client.is_none());
    }

    #[test]
    fn publish_failure_drops_client_and_keeps_app_id() {
        // A publish that fails (no live Discord) drops the client but keeps
        // the resolved application ID so the next publish reconnects with the
        // correct application.
        let mut output = DiscordOutput {
            client: Some(DiscordClient::new(200)),
            default_app_id: 100,
            app_ids: per_source_app_ids(),
            current_app_id: Some(200),
        };

        let result = output.publish("Antigravity", &test_activity("Coding"));
        assert!(result.is_err(), "publish without a live Discord must fail");
        assert!(output.client.is_none(), "failed publish drops the client");
        assert_eq!(
            output.current_app_id,
            Some(200),
            "resolved app id retained so the next publish reconnects with it"
        );

        // The next publish reconnects using the retained application ID.
        let _ = output.publish("Antigravity", &test_activity("Coding 2"));
        assert_eq!(output.current_app_id, Some(200));
    }
}
