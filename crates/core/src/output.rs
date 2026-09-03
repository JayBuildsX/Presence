//! Output Abstraction
//!
//! Defines the generic [`Output`] trait, [`OutputError`], the [`PresenceEngine`]
//! router, and the [`ConsoleOutput`] implementation.
//!
//! This module intentionally knows nothing about specific applications
//! (FL Studio, VS Code, etc.) or specific outputs (Discord, Slack, etc.).

use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};

use super::activity::{Activity, ActivityTimestamps};
use crate::config::{OwnershipPolicy, UnsupportedForegroundPolicy};
use thiserror::Error;
use tracing::{debug, warn};

// ---------------------------------------------------------------------------
// Output Error
// ---------------------------------------------------------------------------

/// Errors that can occur when publishing to an output.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum OutputError {
    /// The output failed to publish the activity.
    #[error("Output failed to publish activity: {0}")]
    PublishFailed(String),
}

// ---------------------------------------------------------------------------
// Output Trait
// ---------------------------------------------------------------------------

/// A destination for published activities.
///
/// Outputs are the terminal consumers of the PresenceHub pipeline.
/// They receive Activities from the Presence Engine and are responsible
/// for rendering, transporting, or storing them.
///
/// # Object Safety
///
/// This trait is object-safe. It has no generic methods and no `Self: Sized`
/// bounds, allowing `Box<dyn Output>` usage inside the Presence Engine.
pub trait Output: Send {
    /// Publish an activity from the given source to this output.
    ///
    /// `source` is the engine's source identity string (the plugin's
    /// metadata name, e.g. "Antigravity"). Outputs that need to
    /// distinguish plugins (e.g. Discord, to select a per-plugin
    /// application ID) use it; other outputs may ignore it.
    ///
    /// Returns an error if the output cannot accept the activity.
    /// The Presence Engine will continue publishing to other outputs
    /// even if one fails.
    fn publish(&mut self, source: &str, activity: &Activity) -> Result<(), OutputError>;

    /// Clear any presence or state this output is displaying.
    ///
    /// Called by the Presence Engine when a session ends, so outputs
    /// (e.g. Discord Rich Presence) can remove stale presence.
    ///
    /// The default implementation is a no-op. Outputs that hold
    /// externally-visible state should override this.
    fn clear(&mut self) {}

    /// Reports whether this output currently holds a live connection, when
    /// the concept applies.
    ///
    /// Returns `None` for outputs where connection state is meaningless
    /// (e.g. console logging). The desktop GUI uses this to display Discord
    /// connection status without knowing anything about Discord.
    fn connection_state(&self) -> Option<bool> {
        None
    }
}

// ---------------------------------------------------------------------------
// Presence Engine
// ---------------------------------------------------------------------------

/// Routes activities from plugins to registered outputs.
///
/// The Presence Engine:
/// - Stores the latest activity per plugin (source)
/// - Detects changes using Activity's PartialEq implementation
/// - Publishes only when the activity changes
/// - Forwards updates to every registered output
/// - Continues publishing to remaining outputs if one fails
///
/// # Session Timing
///
/// The Presence Engine owns the **session timer**, and it owns **one
/// session per plugin source**. Each plugin has an independent last
/// activity and session start time. Ending one plugin's session never
/// affects another plugin's session.
///
/// The engine records when a plugin's session began and stamps every
/// activity it forwards with that session start time. This lets outputs
/// (e.g. Discord Rich Presence) display a live elapsed timer using the
/// platform's native timestamp support.
///
/// The session lifecycle:
/// - **Begins** when the first activity is received from a plugin.
/// - **Continues** across project changes, saves, and metadata updates —
///   the timer is never reset by activity content changes.
/// - **Ends** when the runtime calls
///   [`end_session`](PresenceEngine::end_session) for that plugin
///   (for example, when a plugin's poll fails, indicating the
///   application closed). The plugin's next activity begins a fresh
///   session with a new timer.
///
/// Plugins never manage timers. They emit plain [`Activity`] values with
/// no timestamps; the engine enriches them at the activity-lifecycle stage.
///
/// # Display Ownership
///
/// When multiple plugins have active sessions simultaneously, the engine
/// must choose which activity to display (e.g. Discord shows one activity
/// at a time). The choice is governed by a configurable, source-agnostic
/// [`OwnershipPolicy`]:
///
/// * [`Foreground`](OwnershipPolicy::Foreground) (default) — the source
///   matching the OS foreground window owns the display. When the foreground
///   window belongs to no supported application, the current owner is kept
///   (see [`UnsupportedForegroundPolicy::KeepLast`]) instead of being
///   revoked; only once that owner stops reporting activity does ownership
///   fall back to deterministic registration order among the remaining
///   active sources.
/// * [`Fixed`](OwnershipPolicy::Fixed) — the first-active source owns the
///   display.
/// * [`Recent`](OwnershipPolicy::Recent) — the most recently active source
///   owns the display.
///
/// Each source keeps its own session/activity state. Ending one source's
/// session never affects another's, and the display owner is recalculated
/// whenever a session ends, a source updates, or the foreground changes.
///
/// # Example
///
/// ```rust
/// use presencehub_core::output::{PresenceEngine, ConsoleOutput};
/// use presencehub_core::activity::Activity;
/// use std::collections::HashMap;
///
/// let mut engine = PresenceEngine::new();
/// engine.register_output(Box::new(ConsoleOutput::new()));
///
/// let activity = Activity {
///     state: "Editing".to_string(),
///     details: Some("song.flp".to_string()),
///     timestamps: None,
///     application: None,
///     metadata: HashMap::new(),
/// };
///
/// engine.update("FL Studio", &activity);
///
/// // The engine stamps the session start time on the activity.
/// let current = engine.current_activity().unwrap();
/// assert_eq!(current.state, "Editing");
/// assert!(current.timestamps.as_ref().unwrap().start.is_some());
/// ```
///
/// # Multi-Plugin Isolation
///
/// ```rust
/// use presencehub_core::output::{PresenceEngine, ConsoleOutput};
/// use presencehub_core::activity::Activity;
/// use std::collections::HashMap;
///
/// let mut engine = PresenceEngine::new();
/// engine.register_output(Box::new(ConsoleOutput::new()));
///
/// let coding = Activity {
///     state: "Coding".to_string(),
///     details: None,
///     timestamps: None,
///     application: None,
///     metadata: HashMap::new(),
/// };
///
/// // An app publishes its presence…
/// engine.update("Antigravity", &coding);
///
/// // …then another plugin's poll fails (its app closed). Only that
/// // plugin's session ends; the active presence stays visible.
/// engine.end_session("FL Studio");
/// assert_eq!(engine.current_activity().unwrap().state, "Coding");
/// ```
pub struct PresenceEngine {
    /// Per-plugin sessions, keyed by the plugin's opaque source name.
    sessions: HashMap<String, Session>,
    /// Display priority order: the order in which plugins first became
    /// active. Used by the fixed-policy (and the foreground fallback).
    order: Vec<String>,
    /// Registered outputs that receive published activities.
    outputs: Vec<Box<dyn Output>>,
    /// The configured ownership policy.
    policy: OwnershipPolicy,
    /// How an unsupported foreground application affects the current owner.
    unsupported_foreground: UnsupportedForegroundPolicy,
    /// The source currently matching the OS foreground window, if any.
    foreground: Option<String>,
    /// The source currently owning the display, if any.
    displayed: Option<String>,
    /// Monotonic activity counter used to order sources for the recent
    /// ownership policy.
    recency: u64,
}

/// A single plugin source's session state within the engine.
///
/// Each registered plugin owns an independent session: its own last
/// activity and its own session timer. Ending one plugin's session
/// never affects another plugin's session.
#[derive(Debug, Default)]
struct Session {
    /// The plugin's most recently published activity (session-stamped).
    current: Option<Activity>,
    /// When this plugin's session started.
    session_started_at: Option<SystemTime>,
    /// Monotonic sequence of the last time this source reported activity.
    /// Used by the recent ownership policy.
    last_update: u64,
}

impl Session {
    /// Begins the session on the first detected activity.
    fn begin_if_needed(&mut self) {
        if self.session_started_at.is_none() {
            self.session_started_at = Some(SystemTime::now());
        }
    }
}

impl PresenceEngine {
    /// Creates a new empty Presence Engine with the default ownership policy.
    pub fn new() -> Self {
        Self::with_policy(OwnershipPolicy::default())
    }

    /// Creates a new empty Presence Engine with the given ownership policy.
    pub fn with_policy(policy: OwnershipPolicy) -> Self {
        Self {
            sessions: HashMap::new(),
            order: Vec::new(),
            outputs: Vec::new(),
            policy,
            unsupported_foreground: UnsupportedForegroundPolicy::default(),
            foreground: None,
            displayed: None,
            recency: 0,
        }
    }

    /// Returns the configured ownership policy.
    pub fn policy(&self) -> OwnershipPolicy {
        self.policy
    }

    /// Returns the configured unsupported-foreground policy.
    pub fn unsupported_foreground(&self) -> UnsupportedForegroundPolicy {
        self.unsupported_foreground
    }

    /// Sets the ownership policy.
    pub fn set_policy(&mut self, policy: OwnershipPolicy) {
        if self.policy == policy {
            return;
        }
        self.policy = policy;
        let _ = self.reconcile();
    }

    /// Sets how an unsupported foreground application affects the current
    /// owner. The display owner is recalculated immediately.
    pub fn set_unsupported_foreground(&mut self, policy: UnsupportedForegroundPolicy) {
        if self.unsupported_foreground == policy {
            return;
        }
        self.unsupported_foreground = policy;
        let _ = self.reconcile();
    }

    /// Sets the source currently matching the OS foreground window.
    ///
    /// The runtime calls this each poll cycle with the source resolved from
    /// the foreground window (or `None` when the foreground window does not
    /// belong to any supported application). Only the
    /// [`Foreground`](OwnershipPolicy::Foreground) policy uses this; other
    /// policies ignore it. The display owner is recalculated immediately.
    pub fn set_foreground_source(&mut self, source: Option<&str>) -> Vec<(usize, OutputError)> {
        let next = source.map(str::to_owned);
        if self.foreground == next {
            return Vec::new();
        }
        self.foreground = next;
        self.reconcile()
    }

    /// Registers an output that will receive published activities.
    ///
    /// Outputs are called in registration order.
    pub fn register_output(&mut self, output: Box<dyn Output>) {
        self.outputs.push(output);
    }

    /// Returns the activity currently displayed to outputs, if any.
    ///
    /// The returned activity has been stamped with its session start time.
    pub fn current_activity(&self) -> Option<&Activity> {
        self.displayed_source()
            .and_then(|source| self.sessions.get(source))
            .and_then(|session| session.current.as_ref())
    }

    /// Returns when the displayed session started, if a session is active.
    pub fn session_started_at(&self) -> Option<SystemTime> {
        self.displayed_source()
            .and_then(|source| self.sessions.get(source))
            .and_then(|session| session.session_started_at)
    }

    /// Ends the session for a single plugin source.
    ///
    /// Called when that plugin's application is no longer detected
    /// (e.g. a plugin's poll fails because its window was closed). The
    /// plugin's next activity will begin a fresh session with a new timer.
    ///
    /// This only clears the given plugin's session. Other plugins' active
    /// sessions are unaffected:
    /// - If the ended plugin owned the display and another plugin is
    ///   still active, that plugin's stored activity is promoted and
    ///   published to the outputs.
    /// - If no plugin remains active, [`Output::clear`] is called on every
    ///   registered output so no stale presence remains.
    ///
    /// This is idempotent — calling it for a source with no active
    /// session does nothing.
    pub fn end_session(&mut self, source: &str) {
        // Only act if this plugin has an active session.
        let was_active = self
            .sessions
            .get(source)
            .map(|session| session.session_started_at.is_some())
            .unwrap_or(false);
        if !was_active {
            return;
        }

        if let Some(session) = self.sessions.get_mut(source) {
            session.current = None;
            session.session_started_at = None;
        }

        self.reconcile();
    }

    /// Clears all plugin sessions and ends every session.
    ///
    /// After calling this, `current_activity()` returns `None` until
    /// a new activity is published. All session timers also reset,
    /// matching the "PresenceHub exits" session-end rule.
    ///
    /// Every registered output is told to clear its presence so no stale
    /// activity remains after shutdown (e.g. Discord retaining the last
    /// published presence). Outputs stay registered, so a later session
    /// can publish to them again.
    pub fn clear(&mut self) {
        self.sessions.clear();
        self.order.clear();
        self.foreground = None;
        self.displayed = None;
        self.recency = 0;
        self.clear_outputs();
    }

    /// Clears every registered output without touching sessions.
    ///
    /// Unlike [`clear`](PresenceEngine::clear), sessions, timers, and the
    /// display owner are preserved. Used by the GUI pause control so
    /// Discord shows nothing while paused yet resume continues exactly
    /// where the pause began.
    pub fn clear_outputs(&mut self) {
        for output in self.outputs.iter_mut() {
            output.clear();
        }
    }

    /// Updates the engine with a new activity from a plugin source.
    ///
    /// If the plugin has no session, one is started now. The activity is
    /// stamped with the plugin's session start time, then compared against
    /// that plugin's current activity. If it differs, it is published to
    /// all registered outputs — unless another plugin with priority is
    /// currently displayed, in which case the activity is stored and
    /// promoted later.
    ///
    /// Returns collected errors from outputs that failed to publish.
    /// A failure in one output does not prevent publishing to others.
    pub fn update(&mut self, source: &str, activity: &Activity) -> Vec<(usize, OutputError)> {
        debug!(
            source = %source,
            state = %activity.state,
            details = ?activity.details,
            metadata = ?activity.metadata,
            outputs = self.outputs.len(),
            "PresenceEngine::update called"
        );

        let recency = self.next_recency();
        let session = self.session_mut(source);
        session.begin_if_needed();
        session.last_update = recency;

        let stamped = Self::apply_session_timestamp(session, activity);

        // Skip duplicate activities. Both sides are stamped so the session
        // start remains stable across unchanged plugin output.
        if session.current.as_ref() == Some(&stamped) {
            debug!(source = %source, state = %stamped.state, "PresenceEngine::update skipped duplicate activity");
            return Vec::new();
        }

        session.current = Some(stamped.clone());

        // Publish when the display owner changes, or when this source is the
        // current owner and its own activity changed. A non-owner updating
        // stores its activity for later promotion without touching outputs.
        let next = self.selected_source();
        if self.displayed != next {
            self.set_displayed(next);
            self.publish_displayed()
        } else if self.displayed.as_deref() == Some(source) {
            self.publish_to_outputs(source, &stamped)
        } else {
            Vec::new()
        }
    }

    /// Publish an activity directly, bypassing change detection.
    ///
    /// This is useful for outputs that need to receive every update
    /// regardless of whether the activity changed. The activity is still
    /// stamped with the plugin's current session start time. Like
    /// [`update`](PresenceEngine::update), publishing only occurs when the
    /// plugin owns the display.
    pub fn publish_now(&mut self, source: &str, activity: &Activity) -> Vec<(usize, OutputError)> {
        let recency = self.next_recency();
        let session = self.session_mut(source);
        session.begin_if_needed();
        session.last_update = recency;

        let stamped = Self::apply_session_timestamp(session, activity);
        session.current = Some(stamped.clone());

        let next = self.selected_source();
        if self.displayed != next {
            self.set_displayed(next);
            self.publish_displayed()
        } else if self.displayed.as_deref() == Some(source) {
            self.publish_to_outputs(source, &stamped)
        } else {
            Vec::new()
        }
    }

    /// Publish to all registered outputs and collect errors.
    ///
    /// Only the displayed owner's activity is ever published, so `source`
    /// is the displayed owner's source key. Non-owner sources are stored
    /// internally and never reach the outputs.
    fn publish_to_outputs(
        &mut self,
        source: &str,
        activity: &Activity,
    ) -> Vec<(usize, OutputError)> {
        let mut errors = Vec::new();
        debug!(outputs = self.outputs.len(), source = %source, "PresenceEngine publishing to outputs");

        for (index, output) in self.outputs.iter_mut().enumerate() {
            debug!(output_index = index, "PresenceEngine invoking output");
            if let Err(error) = output.publish(source, activity) {
                warn!(output_index = index, error = %error, "Output failed to publish");
                errors.push((index, error));
            } else {
                debug!(output_index = index, "Output publish succeeded");
            }
        }

        errors
    }

    /// Returns the session for a source, inserting and registering it in
    /// display priority order on first use.
    fn session_mut(&mut self, source: &str) -> &mut Session {
        self.ensure_order(source);
        self.sessions.entry(source.to_string()).or_default()
    }

    /// Registers a source in display priority order on first use.
    fn ensure_order(&mut self, source: &str) {
        if !self.order.iter().any(|entry| entry == source) {
            self.order.push(source.to_string());
        }
    }

    /// Returns the plugin source currently displayed on outputs.
    ///
    /// The owner is chosen by the configured ownership policy, recomputed
    /// whenever a session ends, a source updates, or the foreground changes.
    /// The desktop GUI uses this to show which plugin owns the presence.
    pub fn displayed_source(&self) -> Option<&str> {
        self.displayed.as_deref()
    }

    /// Whether the given source currently has an active session.
    ///
    /// Used by the desktop GUI to distinguish enabled-but-inactive plugins
    /// from enabled-and-active ones.
    pub fn is_source_active(&self, source: &str) -> bool {
        self.is_active(source)
    }

    /// The stored activity for a source, if it has published one.
    ///
    /// Used by the desktop GUI for per-plugin activity summaries. The
    /// returned activity carries the engine-stamped session start time.
    pub fn source_activity(&self, source: &str) -> Option<&Activity> {
        self.sessions
            .get(source)
            .and_then(|session| session.current.as_ref())
    }

    /// Whether any registered output currently reports a live connection.
    ///
    /// Used by the desktop GUI for connection status. Outputs for which
    /// connection state is meaningless report `None` and are ignored.
    pub fn any_output_connected(&self) -> bool {
        self.outputs
            .iter()
            .any(|output| output.connection_state() == Some(true))
    }

    /// Recomputes the display owner and reconciles the outputs.
    ///
    /// - The selected owner changes: publish its stored activity.
    /// - The selected owner disappears: clear every output so no stale
    ///   presence remains.
    /// - The owner is unchanged: nothing happens — an inactive plugin's
    ///   departure must never clear an active presence.
    fn reconcile(&mut self) -> Vec<(usize, OutputError)> {
        let next = self.selected_source();
        if self.displayed != next {
            self.set_displayed(next);
            self.publish_displayed()
        } else {
            Vec::new()
        }
    }

    /// Publishes the currently displayed activity to the outputs, or clears
    /// the outputs when nothing is displayed.
    ///
    /// The displayed owner's source key accompanies the published activity so
    /// outputs know which plugin the presence belongs to.
    fn publish_displayed(&mut self) -> Vec<(usize, OutputError)> {
        let source = self.displayed_source().map(str::to_owned);
        let activity = source
            .as_deref()
            .and_then(|source| self.sessions.get(source))
            .and_then(|session| session.current.clone());
        match activity {
            Some(activity) => {
                let source = source.expect("displayed source present when activity present");
                self.publish_to_outputs(&source, &activity)
            }
            None => {
                for output in self.outputs.iter_mut() {
                    output.clear();
                }
                Vec::new()
            }
        }
    }

    /// Sets the displayed owner, logging only meaningful transitions.
    fn set_displayed(&mut self, next: Option<String>) {
        if self.displayed != next {
            match &next {
                Some(source) => debug!(source = %source, "PresenceEngine display owner changed"),
                None => debug!("PresenceEngine display owner ended"),
            }
            self.displayed = next;
        }
    }

    /// Selects the source that should own the display under the configured
    /// ownership policy.
    ///
    /// Selection is source-agnostic: it operates purely on the registry
    /// order, per-source recency, and the current foreground source — never
    /// on a plugin's name.
    fn selected_source(&self) -> Option<String> {
        match self.policy {
            OwnershipPolicy::Foreground => {
                // A supported application in the foreground owns the display.
                if let Some(source) = self
                    .foreground
                    .as_deref()
                    .filter(|source| self.is_active(source))
                {
                    return Some(source.to_string());
                }

                // The foreground window belongs to no supported application
                // (a browser, file explorer, desktop, etc.). Such a window
                // must not steal or clear the current presence: keep the
                // displayed owner while it is still active, and only fall
                // back to deterministic order once it disappears.
                if self.unsupported_foreground == UnsupportedForegroundPolicy::KeepLast {
                    if let Some(source) = self
                        .displayed
                        .as_deref()
                        .filter(|source| self.is_active(source))
                    {
                        return Some(source.to_string());
                    }
                }

                // Deterministic fallback among the active sources.
                self.fixed_source().map(str::to_owned)
            }
            OwnershipPolicy::Fixed => self.fixed_source().map(str::to_owned),
            OwnershipPolicy::Recent => self.recent_source().map(str::to_owned),
        }
    }

    /// First active source in registration order (fixed policy, and the
    /// foreground policy's fallback when nothing supported is foreground).
    fn fixed_source(&self) -> Option<&str> {
        self.order
            .iter()
            .find(|source| self.is_active(source))
            .map(String::as_str)
    }

    /// Most recently active source (recent policy).
    fn recent_source(&self) -> Option<&str> {
        self.sessions
            .iter()
            .filter(|(_, session)| {
                session.current.is_some() && session.session_started_at.is_some()
            })
            .max_by_key(|(_, session)| session.last_update)
            .map(|(source, _)| source.as_str())
    }

    /// Whether a source currently has an active session.
    fn is_active(&self, source: &str) -> bool {
        self.sessions
            .get(source)
            .map(|session| session.current.is_some() && session.session_started_at.is_some())
            .unwrap_or(false)
    }

    /// Advances the recency counter, wrapping to avoid overflow.
    fn next_recency(&mut self) -> u64 {
        let next = self.recency.wrapping_add(1);
        self.recency = next;
        next
    }

    /// Stamp the activity with the plugin's session start time.
    ///
    /// The engine owns session timing. It fills in `timestamps.start` only
    /// when the plugin did not provide its own — plugins never manage
    /// timers, but a future plugin-provided timestamp (e.g. per-project
    /// timing) is preserved rather than clobbered.
    fn apply_session_timestamp(session: &Session, activity: &Activity) -> Activity {
        // If the plugin already provides a start, respect it.
        if let Some(ts) = &activity.timestamps {
            if ts.start.is_some() {
                return activity.clone();
            }
        }

        let start = session
            .session_started_at
            .unwrap_or_else(SystemTime::now)
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_secs() as i64)
            .unwrap_or(0);

        Activity {
            timestamps: Some(ActivityTimestamps {
                start: Some(start),
                end: activity.timestamps.as_ref().and_then(|t| t.end),
            }),
            ..activity.clone()
        }
    }
}

impl Default for PresenceEngine {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Console Output
// ---------------------------------------------------------------------------

/// A temporary development output that prints activities to stdout.
///
/// # Example Output
///
/// ```text
/// --------------------------------------------------
/// PresenceHub Activity
///
/// Application: FL Studio
/// State: Editing
/// Details: Project: song.flp
/// Session Duration: 00:27:14
///
/// Metadata
/// application = FL Studio
/// version = 21
/// unsaved = true
/// --------------------------------------------------
/// ```
pub struct ConsoleOutput {
    /// Last published activity, for change detection.
    last: Option<Activity>,
}

impl ConsoleOutput {
    /// Creates a new Console Output.
    pub fn new() -> Self {
        Self { last: None }
    }

    /// Returns the last published activity, if any.
    pub fn last_activity(&self) -> Option<&Activity> {
        self.last.as_ref()
    }
}

impl Default for ConsoleOutput {
    fn default() -> Self {
        Self::new()
    }
}

impl Output for ConsoleOutput {
    fn publish(&mut self, _source: &str, activity: &Activity) -> Result<(), OutputError> {
        // Skip duplicate prints
        if self.last.as_ref() == Some(activity) {
            return Ok(());
        }

        self.last = Some(activity.clone());

        println!("--------------------------------------------------");
        println!("PresenceHub Activity");
        println!();
        if let Some(app) = activity.metadata.get("application") {
            println!("Application: {}", app);
        }
        println!("State: {}", activity.state);
        if let Some(ref details) = activity.details {
            println!("Details: {}", details);
        }
        // Print the session duration when the engine has stamped a start.
        if let Some(start) = activity.timestamps.as_ref().and_then(|t| t.start) {
            let epoch = UNIX_EPOCH
                .checked_add(std::time::Duration::from_secs(start as u64))
                .unwrap_or(UNIX_EPOCH);
            if let Ok(elapsed) = SystemTime::now().duration_since(epoch) {
                let secs = elapsed.as_secs();
                println!(
                    "Session Duration: {:02}:{:02}:{:02}",
                    secs / 3600,
                    (secs % 3600) / 60,
                    secs % 60
                );
            }
        }
        println!();
        println!("Metadata");
        for (key, value) in &activity.metadata {
            println!("{} = {}", key, value);
        }
        println!("--------------------------------------------------");

        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use std::collections::HashMap;

    // -- Output trait object safety ----------------------------------------------

    #[test]
    fn output_is_object_safe() {
        fn takes_output(_output: Box<dyn Output>) {}
        takes_output(Box::new(ConsoleOutput::new()));
    }

    // -- ConsoleOutput -----------------------------------------------------------

    #[test]
    fn console_output_publishes() {
        let mut output = ConsoleOutput::new();
        let activity = Activity {
            state: "Editing".to_string(),
            details: Some("song.flp".to_string()),
            timestamps: None,
            metadata: {
                let mut m = HashMap::new();
                m.insert("app".to_string(), "FL".to_string());
                m
            },
            application: None,
        };

        assert!(output.publish("FL Studio", &activity).is_ok());
        assert_eq!(output.last_activity(), Some(&activity));
    }

    #[test]
    fn console_output_skips_duplicates() {
        let mut output = ConsoleOutput::new();
        let activity = Activity {
            state: "Editing".to_string(),
            details: Some("song.flp".to_string()),
            timestamps: None,
            metadata: HashMap::new(),
            application: None,
        };

        assert!(output.publish("A", &activity).is_ok());
        assert!(output.publish("A", &activity).is_ok()); // duplicate
    }

    // -- PresenceEngine ----------------------------------------------------------

    #[test]
    fn engine_starts_empty() {
        let engine = PresenceEngine::new();
        assert!(engine.current_activity().is_none());
        assert!(engine.session_started_at().is_none());
    }

    #[test]
    fn engine_stores_first_activity() {
        let mut engine = PresenceEngine::new();
        let activity = Activity {
            state: "Editing".to_string(),
            details: None,
            timestamps: None,
            metadata: HashMap::new(),
            application: None,
        };

        let errors = engine.update("A", &activity);
        assert!(errors.is_empty());

        // The engine stamps the session start time.
        let current = engine.current_activity().unwrap();
        assert_eq!(current.state, "Editing");
        assert_eq!(current.metadata, activity.metadata);
        assert!(current.timestamps.as_ref().unwrap().start.is_some());
    }

    #[test]
    fn engine_detects_changes() {
        let mut engine = PresenceEngine::new();
        let a1 = Activity {
            state: "Editing".to_string(),
            details: Some("a.flp".to_string()),
            timestamps: None,
            metadata: HashMap::new(),
            application: None,
        };
        let a2 = Activity {
            state: "Editing".to_string(),
            details: Some("b.flp".to_string()),
            timestamps: None,
            metadata: HashMap::new(),
            application: None,
        };

        engine.update("A", &a1);
        let errors = engine.update("A", &a2);
        assert!(errors.is_empty());

        let current = engine.current_activity().unwrap();
        assert_eq!(current.state, "Editing");
        assert_eq!(current.details, Some("b.flp".to_string()));
    }

    #[test]
    fn engine_skips_duplicate_activity() {
        let mut engine = PresenceEngine::new();
        let activity = Activity {
            state: "Idle".to_string(),
            details: None,
            timestamps: None,
            metadata: HashMap::new(),
            application: None,
        };

        engine.update("A", &activity);
        let errors = engine.update("A", &activity);
        assert!(
            errors.is_empty(),
            "Duplicate activity should not produce errors"
        );
    }

    #[test]
    fn engine_clears_current_activity() {
        let mut engine = PresenceEngine::new();
        let activity = Activity {
            state: "Editing".to_string(),
            details: None,
            timestamps: None,
            metadata: HashMap::new(),
            application: None,
        };

        engine.update("A", &activity);
        engine.clear();
        assert!(engine.current_activity().is_none());

        // Clearing also ends the session.
        assert!(engine.session_started_at().is_none());
    }

    #[test]
    fn engine_registers_outputs() {
        let mut engine = PresenceEngine::new();
        engine.register_output(Box::new(ConsoleOutput::new()));
        assert_eq!(engine.outputs.len(), 1);
    }

    #[test]
    fn engine_publishes_to_all_outputs() {
        let mut engine = PresenceEngine::new();
        engine.register_output(Box::new(ConsoleOutput::new()));
        engine.register_output(Box::new(ConsoleOutput::new()));

        let activity = Activity {
            state: "Editing".to_string(),
            details: None,
            timestamps: None,
            metadata: HashMap::new(),
            application: None,
        };

        let errors = engine.update("A", &activity);
        assert!(errors.is_empty());
    }

    #[test]
    fn engine_continues_on_output_failure() {
        struct FailingOutput;
        impl Output for FailingOutput {
            fn publish(&mut self, _source: &str, _activity: &Activity) -> Result<(), OutputError> {
                Err(OutputError::PublishFailed("fail".to_string()))
            }
        }

        let mut engine = PresenceEngine::new();
        engine.register_output(Box::new(FailingOutput));
        engine.register_output(Box::new(ConsoleOutput::new()));

        let activity = Activity {
            state: "Editing".to_string(),
            details: None,
            timestamps: None,
            metadata: HashMap::new(),
            application: None,
        };

        let errors = engine.update("A", &activity);
        assert_eq!(errors.len(), 1);
        assert!(errors[0].1.to_string().contains("fail"));
    }

    #[test]
    fn engine_update_with_empty_outputs() {
        let mut engine = PresenceEngine::new();
        let activity = Activity {
            state: "Idle".to_string(),
            details: None,
            timestamps: None,
            metadata: HashMap::new(),
            application: None,
        };

        let errors = engine.update("A", &activity);
        assert!(errors.is_empty());
        assert!(engine.current_activity().is_some());
    }

    #[test]
    fn engine_publish_now_bypasses_change_detection() {
        let mut engine = PresenceEngine::new();
        engine.register_output(Box::new(ConsoleOutput::new()));

        let activity = Activity {
            state: "Editing".to_string(),
            details: None,
            timestamps: None,
            metadata: HashMap::new(),
            application: None,
        };

        engine.update("A", &activity);
        let errors = engine.publish_now("A", &activity);
        assert!(errors.is_empty());
    }

    // -- PresenceEngine session timing -------------------------------------------

    #[test]
    fn engine_session_timer_initialized_on_first_activity() {
        let mut engine = PresenceEngine::new();
        assert!(
            engine.session_started_at().is_none(),
            "no session before first activity"
        );

        let activity = Activity {
            state: "Editing".to_string(),
            details: Some("beat.flp".to_string()),
            timestamps: None,
            metadata: HashMap::new(),
            application: None,
        };
        engine.update("A", &activity);

        // Session begins when the first activity is detected.
        assert!(engine.session_started_at().is_some());
        // The stamped activity carries the session start as Unix seconds.
        let start = engine
            .current_activity()
            .and_then(|a| a.timestamps.as_ref())
            .and_then(|t| t.start)
            .expect("activity should carry session start timestamp");
        assert!(start > 0);
    }

    #[test]
    fn engine_session_timer_persists_across_project_changes() {
        let mut engine = PresenceEngine::new();

        let beat = Activity {
            state: "Editing".to_string(),
            details: Some("beat.flp".to_string()),
            timestamps: None,
            metadata: HashMap::new(),
            application: None,
        };
        engine.update("A", &beat);
        let session_time = engine.session_started_at().expect("session should start");
        let first_start = engine
            .current_activity()
            .and_then(|a| a.timestamps.as_ref())
            .and_then(|t| t.start)
            .unwrap();

        // Switching projects must NOT reset the session timer.
        let trap = Activity {
            state: "Editing".to_string(),
            details: Some("trap.flp".to_string()),
            timestamps: None,
            metadata: HashMap::new(),
            application: None,
        };
        engine.update("A", &trap);

        assert_eq!(
            engine.session_started_at(),
            Some(session_time),
            "project switch must not reset the session timer"
        );

        let second_start = engine
            .current_activity()
            .and_then(|a| a.timestamps.as_ref())
            .and_then(|t| t.start)
            .unwrap();
        assert_eq!(
            first_start, second_start,
            "both projects share the session start"
        );
        assert_eq!(
            engine.current_activity().unwrap().details,
            Some("trap.flp".to_string())
        );
    }

    #[test]
    fn engine_session_timer_persists_across_metadata_updates() {
        let mut engine = PresenceEngine::new();

        let unsaved = Activity {
            state: "Editing".to_string(),
            details: Some("song.flp*".to_string()),
            timestamps: None,
            metadata: {
                let mut m = HashMap::new();
                m.insert("unsaved".to_string(), "true".to_string());
                m
            },
            application: None,
        };
        engine.update("A", &unsaved);
        let session_time = engine.session_started_at().expect("session should start");

        // Unsaved → saved transition must NOT reset the timer.
        let saved = Activity {
            state: "Editing".to_string(),
            details: Some("song.flp".to_string()),
            timestamps: None,
            metadata: {
                let mut m = HashMap::new();
                m.insert("unsaved".to_string(), "false".to_string());
                m
            },
            application: None,
        };
        engine.update("A", &saved);

        assert_eq!(
            engine.session_started_at(),
            Some(session_time),
            "metadata-only updates must not reset the session timer"
        );
    }

    #[test]
    fn engine_session_timer_resets_after_end_session() {
        let mut engine = PresenceEngine::new();

        let activity = Activity {
            state: "Idle".to_string(),
            details: None,
            timestamps: None,
            metadata: HashMap::new(),
            application: None,
        };

        // First session.
        engine.update("A", &activity);
        let first_session = engine
            .session_started_at()
            .expect("first session should start");

        // Application closes → runtime signals session end.
        engine.end_session("A");
        assert!(
            engine.session_started_at().is_none(),
            "session should be cleared"
        );

        // Application reopens → new session with a fresh timer.
        engine.update("A", &activity);
        let second_session = engine
            .session_started_at()
            .expect("second session should start");
        assert_ne!(
            first_session, second_session,
            "a new session must have a new start time"
        );

        // The new session's stamped activity uses the new start.
        let second_start = engine
            .current_activity()
            .and_then(|a| a.timestamps.as_ref())
            .and_then(|t| t.start)
            .unwrap();
        let second_epoch = second_session
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap();
        assert_eq!(second_start, second_epoch);
    }

    #[test]
    fn engine_end_session_is_idempotent() {
        let mut engine = PresenceEngine::new();
        // Calling end_session with no active session does nothing.
        engine.end_session("A");
        assert!(engine.session_started_at().is_none());
    }

    #[test]
    fn engine_end_session_clears_outputs() {
        // Verify that end_session calls clear() on every registered output,
        // so no stale presence remains after the application closes.
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::sync::Arc;

        struct TrackingOutput {
            clear_count: Arc<AtomicUsize>,
        }
        impl Output for TrackingOutput {
            fn publish(&mut self, _source: &str, _activity: &Activity) -> Result<(), OutputError> {
                Ok(())
            }
            fn clear(&mut self) {
                self.clear_count.fetch_add(1, Ordering::SeqCst);
            }
        }

        let clear_count = Arc::new(AtomicUsize::new(0));
        let mut engine = PresenceEngine::new();
        engine.register_output(Box::new(TrackingOutput {
            clear_count: clear_count.clone(),
        }));
        engine.register_output(Box::new(TrackingOutput {
            clear_count: clear_count.clone(),
        }));

        let activity = Activity {
            state: "Editing".to_string(),
            details: None,
            timestamps: None,
            metadata: HashMap::new(),
            application: None,
        };
        engine.update("A", &activity);

        engine.end_session("A");
        assert_eq!(
            clear_count.load(Ordering::SeqCst),
            2,
            "both outputs should be cleared"
        );
    }

    #[test]
    fn engine_clear_clears_all_registered_outputs() {
        // Regression: PresenceEngine::clear() must call clear() on every
        // registered output so no stale presence remains when PresenceHub
        // shuts down (e.g. Discord must not retain the last activity).
        use std::sync::atomic::AtomicUsize;
        use std::sync::Arc;

        struct TrackingOutput {
            clear_count: Arc<AtomicUsize>,
        }
        impl Output for TrackingOutput {
            fn publish(&mut self, _source: &str, _activity: &Activity) -> Result<(), OutputError> {
                Ok(())
            }
            fn clear(&mut self) {
                self.clear_count.fetch_add(1, Ordering::SeqCst);
            }
        }

        let clear_count = Arc::new(AtomicUsize::new(0));
        let mut engine = PresenceEngine::new();
        engine.register_output(Box::new(TrackingOutput {
            clear_count: clear_count.clone(),
        }));
        engine.register_output(Box::new(TrackingOutput {
            clear_count: clear_count.clone(),
        }));

        let activity = Activity {
            state: "Editing".to_string(),
            details: None,
            timestamps: None,
            metadata: HashMap::new(),
            application: None,
        };
        engine.update("A", &activity);
        assert_eq!(
            clear_count.load(Ordering::SeqCst),
            0,
            "no clear before shutdown"
        );

        engine.clear();
        assert_eq!(
            clear_count.load(Ordering::SeqCst),
            2,
            "clear() must clear every registered output"
        );
        assert!(engine.current_activity().is_none());
        // Outputs stay registered so a subsequent session can publish again.
        assert_eq!(engine.outputs.len(), 2);
    }

    #[test]
    fn engine_clear_outputs_preserves_sessions() {
        // The GUI pause control clears outputs without ending sessions:
        // timers, the display owner, and stored activities must survive.
        use std::sync::atomic::AtomicUsize;
        use std::sync::Arc;

        struct TrackingOutput {
            clear_count: Arc<AtomicUsize>,
        }
        impl Output for TrackingOutput {
            fn publish(&mut self, _source: &str, _activity: &Activity) -> Result<(), OutputError> {
                Ok(())
            }
            fn clear(&mut self) {
                self.clear_count.fetch_add(1, Ordering::SeqCst);
            }
        }

        let clear_count = Arc::new(AtomicUsize::new(0));
        let mut engine = PresenceEngine::new();
        engine.register_output(Box::new(TrackingOutput {
            clear_count: clear_count.clone(),
        }));

        let activity = Activity {
            state: "Editing".to_string(),
            details: None,
            timestamps: None,
            metadata: HashMap::new(),
            application: None,
        };
        engine.update("A", &activity);
        let started = engine.session_started_at().expect("session should start");

        engine.clear_outputs();
        assert_eq!(
            clear_count.load(Ordering::SeqCst),
            1,
            "clear_outputs() must clear every registered output"
        );
        assert!(
            engine.current_activity().is_some(),
            "sessions must survive clear_outputs()"
        );
        assert_eq!(
            engine.session_started_at(),
            Some(started),
            "session timers must survive clear_outputs()"
        );
        assert_eq!(engine.displayed_source(), Some("A"));
    }

    #[test]
    fn engine_displayed_source_and_activity_getters() {
        let mut engine = PresenceEngine::new();
        assert_eq!(engine.displayed_source(), None);
        assert!(!engine.is_source_active("A"));
        assert!(engine.source_activity("A").is_none());
        assert!(!engine.any_output_connected());

        let activity = Activity {
            state: "Editing".to_string(),
            details: None,
            timestamps: None,
            metadata: HashMap::new(),
            application: None,
        };
        engine.update("A", &activity);

        assert_eq!(engine.displayed_source(), Some("A"));
        assert!(engine.is_source_active("A"));
        assert!(!engine.is_source_active("B"));
        assert_eq!(
            engine.source_activity("A").map(|a| a.state.as_str()),
            Some("Editing")
        );
        assert!(engine.source_activity("B").is_none());
    }

    #[test]
    fn engine_shutdown_clears_presence_for_any_displayed_plugin() {
        // Graceful shutdown must deliver clear() to every registered output
        // regardless of which plugin owns the display. Exercises the Antigravity
        // and FL Studio source identities (the same sources the
        // real runtime publishes) to prove plugin independence.
        use std::sync::atomic::AtomicUsize;
        use std::sync::Arc;

        struct TrackingOutput {
            clear_count: Arc<AtomicUsize>,
        }
        impl Output for TrackingOutput {
            fn publish(&mut self, _source: &str, _activity: &Activity) -> Result<(), OutputError> {
                Ok(())
            }
            fn clear(&mut self) {
                self.clear_count.fetch_add(1, Ordering::SeqCst);
            }
        }

        let clear_count = Arc::new(AtomicUsize::new(0));
        let mut engine = PresenceEngine::new();
        engine.register_output(Box::new(TrackingOutput {
            clear_count: clear_count.clone(),
        }));

        // 1. Antigravity owns the display.
        let antigravity_activity = Activity {
            state: "Coding".to_string(),
            details: Some("Working on task".to_string()),
            timestamps: None,
            metadata: HashMap::new(),
            application: None,
        };
        engine.update("Antigravity", &antigravity_activity);
        assert_eq!(
            clear_count.load(Ordering::SeqCst),
            0,
            "no clear before shutdown"
        );

        // 2. Graceful shutdown: the output must receive clear().
        engine.clear();
        assert_eq!(
            clear_count.load(Ordering::SeqCst),
            1,
            "shutdown must clear outputs after an Antigravity session"
        );
        assert!(engine.current_activity().is_none());

        // 3. A fresh FL Studio session is subject to the same guarantee.
        let fl_activity = Activity {
            state: "Editing".to_string(),
            details: Some("song.flp".to_string()),
            timestamps: None,
            metadata: HashMap::new(),
            application: None,
        };
        engine.update("FL Studio", &fl_activity);
        engine.clear();
        assert_eq!(
            clear_count.load(Ordering::SeqCst),
            2,
            "shutdown must clear outputs after an FL Studio session too"
        );
    }

    #[test]
    fn engine_preserves_plugin_provided_start() {
        let mut engine = PresenceEngine::new();

        // A future plugin-provided per-project start must be preserved,
        // not clobbered by the engine's session timer.
        let activity = Activity {
            state: "Editing".to_string(),
            details: Some("track.flp".to_string()),
            timestamps: Some(ActivityTimestamps {
                start: Some(999_999_999),
                end: None,
            }),
            metadata: HashMap::new(),
            application: None,
        };

        engine.update("A", &activity);
        let stored = engine.current_activity().unwrap();
        assert_eq!(
            stored.timestamps.as_ref().unwrap().start,
            Some(999_999_999),
            "plugin-provided start must be preserved"
        );
    }

    // -- Multi-plugin session isolation -----------------------------------------
    //
    // Regression tests for the per-plugin session ownership model: one
    // plugin's poll failure (end_session) must never clear another
    // plugin's published presence.

    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};

    /// A test output that counts publishes and clears and records the last
    /// published state.
    struct TrackingOutput {
        publish_count: Arc<AtomicUsize>,
        clear_count: Arc<AtomicUsize>,
        last_state: Arc<Mutex<Option<String>>>,
    }

    impl Output for TrackingOutput {
        fn publish(&mut self, _source: &str, activity: &Activity) -> Result<(), OutputError> {
            self.publish_count.fetch_add(1, Ordering::SeqCst);
            *self.last_state.lock().unwrap() = Some(activity.state.clone());
            Ok(())
        }

        fn clear(&mut self) {
            self.clear_count.fetch_add(1, Ordering::SeqCst);
            *self.last_state.lock().unwrap() = None;
        }
    }

    /// Builds an engine with a tracking output and returns the shared
    /// counters for assertions.
    #[allow(clippy::type_complexity)]
    fn tracking_engine() -> (
        PresenceEngine,
        Arc<AtomicUsize>,
        Arc<AtomicUsize>,
        Arc<Mutex<Option<String>>>,
    ) {
        let publish_count = Arc::new(AtomicUsize::new(0));
        let clear_count = Arc::new(AtomicUsize::new(0));
        let last_state = Arc::new(Mutex::new(None));

        let mut engine = PresenceEngine::new();
        engine.register_output(Box::new(TrackingOutput {
            publish_count: publish_count.clone(),
            clear_count: clear_count.clone(),
            last_state: last_state.clone(),
        }));

        (engine, publish_count, clear_count, last_state)
    }

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

    #[test]
    fn multi_plugin_other_plugin_failure_preserves_this_plugin() {
        // Regression A: plugin "A" publishes an activity; plugin "B" then
        // fails its poll. B's failure must NOT clear A's published presence.
        let (mut engine, publishes, clears, _last) = tracking_engine();

        engine.update("A", &test_activity("Editing"));
        assert_eq!(
            publishes.load(Ordering::SeqCst),
            1,
            "A's activity should be published"
        );
        assert_eq!(clears.load(Ordering::SeqCst), 0);

        engine.end_session("B"); // B has no active session (e.g. FL Studio closed)
        assert_eq!(
            clears.load(Ordering::SeqCst),
            0,
            "B's failure must not clear A's presence"
        );
        assert!(
            engine.current_activity().is_some(),
            "A's presence must remain visible"
        );
    }

    #[test]
    fn multi_plugin_failing_plugin_clears_only_its_own_session() {
        // Regression B: the plugin that fails is the one whose session ends;
        // its own presence is cleared.
        let (mut engine, publishes, clears, _last) = tracking_engine();

        engine.update("A", &test_activity("Editing"));
        assert_eq!(publishes.load(Ordering::SeqCst), 1);

        engine.end_session("A");
        assert_eq!(
            clears.load(Ordering::SeqCst),
            1,
            "ending A's session clears outputs"
        );
        assert!(engine.current_activity().is_none(), "A's presence is gone");
    }

    #[test]
    fn multi_plugin_unaffected_plugin_dedup_preserved() {
        // Regression C: after B fails, A's unchanged activity must still be
        // deduplicated — no spurious republish.
        let (mut engine, publishes, clears, _last) = tracking_engine();

        engine.update("A", &test_activity("Idle"));
        assert_eq!(publishes.load(Ordering::SeqCst), 1);

        engine.end_session("B");

        engine.update("A", &test_activity("Idle")); // unchanged → deduplicated
        assert_eq!(
            publishes.load(Ordering::SeqCst),
            1,
            "unchanged activity must be deduplicated"
        );
        assert_eq!(clears.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn multi_plugin_exited_plugin_leaves_other_published() {
        // Regression D: A (first active, display owner) exits while B has an
        // active stored session. B must be promoted and republished — the
        // outputs must NOT be cleared.
        let (mut engine, publishes, clears, last_state) = tracking_engine();

        engine.update("A", &test_activity("Editing"));
        assert_eq!(publishes.load(Ordering::SeqCst), 1);

        engine.update("B", &test_activity("InGame"));
        assert_eq!(
            publishes.load(Ordering::SeqCst),
            1,
            "B is stored, not displayed, while A is active"
        );

        engine.end_session("A"); // A exits → B promoted
        assert_eq!(
            clears.load(Ordering::SeqCst),
            0,
            "outputs must not be cleared while B is active"
        );
        assert_eq!(
            publishes.load(Ordering::SeqCst),
            2,
            "B's stored activity should be republished"
        );
        assert_eq!(&*last_state.lock().unwrap(), &Some("InGame".to_string()));

        let current = engine.current_activity().unwrap();
        assert_eq!(current.state, "InGame", "B remains published after A exits");
    }

    #[test]
    fn engine_threads_displayed_owner_source_to_outputs() {
        // The source identity threaded into Output::publish must be the
        // displayed owner's, not the source that just updated. A non-owner
        // update is stored but never reaches the outputs (so a Discord
        // application switch can never be triggered by a non-displayed
        // plugin).
        use std::sync::{Arc, Mutex};

        struct SourceRecordingOutput {
            published: Arc<Mutex<Vec<String>>>,
        }
        impl Output for SourceRecordingOutput {
            fn publish(&mut self, source: &str, _activity: &Activity) -> Result<(), OutputError> {
                self.published.lock().unwrap().push(source.to_string());
                Ok(())
            }
        }

        let published = Arc::new(Mutex::new(Vec::new()));
        let mut engine = PresenceEngine::new();
        engine.register_output(Box::new(SourceRecordingOutput {
            published: published.clone(),
        }));

        engine.update("Antigravity", &test_activity("Coding"));
        assert_eq!(
            *published.lock().unwrap(),
            vec!["Antigravity".to_string()],
            "first owner publishes its own source"
        );

        // FL Studio updates while Antigravity owns the display: stored, not
        // published, and must not reach the output (no Discord switch).
        engine.update("FL Studio", &test_activity("Editing"));
        assert_eq!(
            *published.lock().unwrap(),
            vec!["Antigravity".to_string()],
            "non-owner updates must not be published"
        );

        // FL Studio becomes displayed (foreground): its activity is published
        // with FL Studio's source identity.
        engine.set_foreground_source(Some("FL Studio"));
        assert_eq!(
            *published.lock().unwrap(),
            vec!["Antigravity".to_string(), "FL Studio".to_string()],
            "displayed owner switch publishes the new source"
        );
    }

    // -- Ownership policy --------------------------------------------------------
    //
    // Tests for the configurable ownership policy (foreground / fixed /
    // recent). Policies are source-agnostic: selection never inspects a
    // plugin's name.

    #[test]
    fn ownership_policy_defaults_to_foreground() {
        // The engine must default to the foreground policy so a config file
        // without an explicit `[presence]` section behaves as documented.
        assert_eq!(PresenceEngine::new().policy(), OwnershipPolicy::Foreground);
        assert_eq!(
            Config::default().presence.ownership,
            OwnershipPolicy::Foreground
        );
    }

    #[test]
    fn ownership_fixed_policy_first_active_wins() {
        // Fixed: registration order decides; the first active source owns
        // the display even after a second source becomes active.
        let (mut engine, publishes, clears, last_state) = tracking_engine();
        engine.set_policy(OwnershipPolicy::Fixed);

        engine.update("A", &test_activity("Editing"));
        engine.update("B", &test_activity("InGame"));
        assert_eq!(publishes.load(Ordering::SeqCst), 1, "A owns the display");
        assert_eq!(clears.load(Ordering::SeqCst), 0);
        assert_eq!(engine.current_activity().unwrap().state, "Editing");
        assert_eq!(&*last_state.lock().unwrap(), &Some("Editing".to_string()));

        // B updates → still stored, not displayed.
        engine.update("B", &test_activity("InGame2"));
        assert_eq!(publishes.load(Ordering::SeqCst), 1);
        assert_eq!(engine.current_activity().unwrap().state, "Editing");
    }

    #[test]
    fn ownership_foreground_source_owns_display() {
        // Foreground: the source matching the foreground window owns the
        // display regardless of registration order.
        let (mut engine, publishes, clears, last_state) = tracking_engine();

        engine.update("A", &test_activity("Editing"));
        assert_eq!(publishes.load(Ordering::SeqCst), 1);

        engine.set_foreground_source(Some("B"));
        // "B" is foreground but has no active session → falls back to A.
        assert_eq!(engine.current_activity().unwrap().state, "Editing");

        engine.update("B", &test_activity("InGame"));
        // B is now active and foreground → B owns the display.
        assert_eq!(engine.current_activity().unwrap().state, "InGame");
        assert_eq!(&*last_state.lock().unwrap(), &Some("InGame".to_string()));
        assert_eq!(clears.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn ownership_foreground_falls_back_to_registration_order() {
        // When the foreground window is not a supported application (None),
        // ownership falls back to deterministic registration order.
        let (mut engine, publishes, clears, last_state) = tracking_engine();

        engine.update("A", &test_activity("Editing"));
        engine.update("B", &test_activity("InGame"));
        engine.set_foreground_source(None); // some other app is foreground
        assert_eq!(
            publishes.load(Ordering::SeqCst),
            1,
            "A (registration first) owns display"
        );
        assert_eq!(
            clears.load(Ordering::SeqCst),
            0,
            "other app foreground must not clear presence"
        );
        assert_eq!(engine.current_activity().unwrap().state, "Editing");
        assert_eq!(&*last_state.lock().unwrap(), &Some("Editing".to_string()));
    }

    #[test]
    fn ownership_recent_policy_most_recently_active_wins() {
        // Recent: the most recently active source owns the display.
        let (mut engine, _publishes, clears, last_state) = tracking_engine();
        engine.set_policy(OwnershipPolicy::Recent);

        engine.update("A", &test_activity("Editing"));
        engine.update("B", &test_activity("InGame"));
        assert_eq!(
            engine.current_activity().unwrap().state,
            "InGame",
            "B is most recent"
        );

        // A becomes active again → A most recent → switch.
        engine.update("A", &test_activity("Editing2"));
        assert_eq!(engine.current_activity().unwrap().state, "Editing2");
        assert_eq!(&*last_state.lock().unwrap(), &Some("Editing2".to_string()));
        assert_eq!(clears.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn ownership_no_active_sources_clears_outputs() {
        // No active sources under any policy → outputs cleared.
        let (mut engine, _publishes, clears, _last) = tracking_engine();
        engine.set_policy(OwnershipPolicy::Recent);

        engine.update("A", &test_activity("Editing"));
        engine.end_session("A");
        assert_eq!(
            clears.load(Ordering::SeqCst),
            1,
            "no active source → clear outputs"
        );
        assert!(engine.current_activity().is_none());
    }

    #[test]
    fn ownership_single_active_source_displayed() {
        // A single active source is displayed under every policy.
        for policy in [
            OwnershipPolicy::Foreground,
            OwnershipPolicy::Fixed,
            OwnershipPolicy::Recent,
        ] {
            let (mut engine, publishes, clears, last_state) = tracking_engine();
            engine.set_policy(policy);
            engine.update("A", &test_activity("Editing"));
            assert_eq!(publishes.load(Ordering::SeqCst), 1);
            assert_eq!(clears.load(Ordering::SeqCst), 0);
            assert_eq!(engine.current_activity().unwrap().state, "Editing");
            assert_eq!(&*last_state.lock().unwrap(), &Some("Editing".to_string()));
        }
    }

    #[test]
    fn ownership_foreground_owner_exits_promotes_fixed_fallback() {
        // Foreground owner exits → recalculate immediately; the next active
        // source (registration order fallback) is promoted, not cleared.
        let (mut engine, _publishes, clears, last_state) = tracking_engine();

        engine.update("A", &test_activity("Editing"));
        engine.set_foreground_source(Some("A"));
        engine.update("B", &test_activity("InGame"));
        assert_eq!(engine.current_activity().unwrap().state, "Editing");

        engine.end_session("A"); // foreground owner exits
        assert_eq!(clears.load(Ordering::SeqCst), 0, "B is active → no clear");
        assert_eq!(
            engine.current_activity().unwrap().state,
            "InGame",
            "B promoted"
        );
        assert_eq!(&*last_state.lock().unwrap(), &Some("InGame".to_string()));
    }

    #[test]
    fn ownership_non_owner_exits_does_not_clear() {
        // A non-owner becoming inactive must never clear the owner's
        // published presence.
        let (mut engine, _publishes, clears, _last) = tracking_engine();
        engine.set_policy(OwnershipPolicy::Fixed);

        engine.update("A", &test_activity("Editing"));
        engine.update("B", &test_activity("InGame"));
        engine.end_session("B"); // B was never the owner
        assert_eq!(
            clears.load(Ordering::SeqCst),
            0,
            "non-owner exit must not clear"
        );
        assert_eq!(engine.current_activity().unwrap().state, "Editing");
    }

    #[test]
    fn ownership_recent_owner_exits_promotes_next_recent() {
        // Recent policy: the owner exits → the next most recent active
        // source is promoted.
        let (mut engine, _publishes, clears, last_state) = tracking_engine();
        engine.set_policy(OwnershipPolicy::Recent);

        engine.update("A", &test_activity("Editing"));
        engine.update("B", &test_activity("InGame"));
        assert_eq!(engine.current_activity().unwrap().state, "InGame");

        engine.end_session("B");
        assert_eq!(
            clears.load(Ordering::SeqCst),
            0,
            "A is still active → no clear"
        );
        assert_eq!(
            engine.current_activity().unwrap().state,
            "Editing",
            "A promoted"
        );
        assert_eq!(&*last_state.lock().unwrap(), &Some("Editing".to_string()));
    }

    #[test]
    fn ownership_per_source_dedup_preserved_under_policy() {
        // Per-source duplicate suppression must still work under every
        // ownership policy: an unchanged non-owner update is not republished.
        for policy in [
            OwnershipPolicy::Foreground,
            OwnershipPolicy::Fixed,
            OwnershipPolicy::Recent,
        ] {
            let (mut engine, publishes, clears, _last) = tracking_engine();
            engine.set_policy(policy);

            engine.update("A", &test_activity("Idle"));
            assert_eq!(publishes.load(Ordering::SeqCst), 1);
            engine.update("B", &test_activity("Other"));
            let after_b = publishes.load(Ordering::SeqCst);
            engine.update("B", &test_activity("Other")); // unchanged → dedup
            assert_eq!(
                publishes.load(Ordering::SeqCst),
                after_b,
                "unchanged activity must be deduplicated"
            );
            assert_eq!(clears.load(Ordering::SeqCst), 0);
        }
    }

    #[test]
    fn ownership_foreground_change_to_unknown_app_keeps_presence() {
        // Switching the foreground window to an unsupported application must
        // not clear an active presence — it only falls back to the fixed
        // selection among active sources.
        let (mut engine, _publishes, clears, _last) = tracking_engine();

        engine.update("A", &test_activity("Editing"));
        engine.set_foreground_source(Some("A"));
        engine.update("B", &test_activity("InGame"));
        assert_eq!(engine.current_activity().unwrap().state, "Editing");

        engine.set_foreground_source(None); // foregrounded an unsupported app
        assert_eq!(
            clears.load(Ordering::SeqCst),
            0,
            "presence stays while A is active"
        );
        assert_eq!(engine.current_activity().unwrap().state, "Editing");
    }

    // -- Unsupported foreground ownership ---------------------------------------
    //
    // The core rule: an unsupported foreground application must never steal
    // or clear the presence of the current owner. Ownership only moves when a
    // supported source actually becomes foreground, or when the current owner
    // disappears.

    #[test]
    fn unsupported_foreground_keeps_current_owner() {
        // B wins via the foreground window; an unsupported window afterwards
        // must not revert ownership back to the registration-first source.
        let (mut engine, publishes, clears, last_state) = tracking_engine();

        engine.update("A", &test_activity("Editing"));
        engine.update("B", &test_activity("InGame"));
        engine.set_foreground_source(Some("B"));
        assert_eq!(
            engine.current_activity().unwrap().state,
            "InGame",
            "B owns via foreground"
        );

        engine.set_foreground_source(None); // Brave / Explorer / desktop
        assert_eq!(
            engine.current_activity().unwrap().state,
            "InGame",
            "unsupported foreground must not steal from the current owner"
        );
        assert_eq!(&*last_state.lock().unwrap(), &Some("InGame".to_string()));
        assert_eq!(clears.load(Ordering::SeqCst), 0);
        assert_eq!(publishes.load(Ordering::SeqCst), 2, "no spurious republish");
    }

    #[test]
    fn unsupported_foreground_does_not_clear_presence() {
        // A single supported source owns the display; an unsupported window
        // must not clear it.
        let (mut engine, _publishes, clears, _last) = tracking_engine();

        engine.update("A", &test_activity("Editing"));
        engine.set_foreground_source(Some("A"));
        engine.set_foreground_source(None);
        assert_eq!(
            clears.load(Ordering::SeqCst),
            0,
            "unsupported window must not clear"
        );
        assert_eq!(engine.current_activity().unwrap().state, "Editing");
    }

    #[test]
    fn unsupported_foreground_owner_still_yields_to_new_supported_foreground() {
        // Even while keeping the last owner under an unsupported window, a
        // newly foregrounded supported source still takes over.
        let (mut engine, _publishes, clears, last_state) = tracking_engine();

        engine.update("A", &test_activity("Editing"));
        engine.set_foreground_source(Some("B"));
        engine.update("B", &test_activity("InGame"));
        engine.set_foreground_source(None); // unsupported window — B kept
        assert_eq!(engine.current_activity().unwrap().state, "InGame");

        engine.set_foreground_source(Some("A")); // back to A
        assert_eq!(engine.current_activity().unwrap().state, "Editing");
        assert_eq!(&*last_state.lock().unwrap(), &Some("Editing".to_string()));
        assert_eq!(clears.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn foreground_ownership_acceptance_sequence_antigravity_flstudio_brave_antigravity() {
        // Acceptance scenario: Antigravity -> FL Studio -> Brave -> Antigravity must
        // yield Antigravity -> FL Studio -> FL Studio -> Antigravity with no clears.
        // Mirrors the real window identities from the Antigravity and FL Studio plugins.
        let (mut engine, _publishes, clears, last_state) = tracking_engine();

        engine.update("Antigravity", &test_activity("Coding"));
        engine.set_foreground_source(Some("Antigravity"));
        assert_eq!(
            engine.current_activity().unwrap().state,
            "Coding",
            "Antigravity owns while foreground"
        );

        engine.update("FL Studio", &test_activity("Editing"));
        engine.set_foreground_source(Some("FL Studio"));
        assert_eq!(
            engine.current_activity().unwrap().state,
            "Editing",
            "switch to FL Studio is immediate"
        );
        assert_eq!(&*last_state.lock().unwrap(), &Some("Editing".to_string()));

        engine.set_foreground_source(None); // Brave / unsupported
        assert_eq!(
            engine.current_activity().unwrap().state,
            "Editing",
            "unsupported foreground keeps the last owned activity"
        );
        assert_eq!(&*last_state.lock().unwrap(), &Some("Editing".to_string()));

        engine.set_foreground_source(Some("Antigravity"));
        assert_eq!(
            engine.current_activity().unwrap().state,
            "Coding",
            "back to a supported app restores its activity"
        );
        assert_eq!(&*last_state.lock().unwrap(), &Some("Coding".to_string()));
        assert_eq!(
            clears.load(Ordering::SeqCst),
            0,
            "ownership transfers must not issue clears"
        );
    }

    #[test]
    fn unsupported_foreground_owner_exits_promotes_remaining_source() {
        // The kept owner closes while an unsupported window is foreground:
        // the next best active source is promoted, never the outputs cleared.
        let (mut engine, _publishes, clears, last_state) = tracking_engine();

        engine.update("A", &test_activity("Editing"));
        engine.update("B", &test_activity("InGame"));
        engine.set_foreground_source(Some("B"));
        engine.set_foreground_source(None); // unsupported window — B kept
        assert_eq!(engine.current_activity().unwrap().state, "InGame");

        engine.end_session("B"); // current owner closes
        assert_eq!(
            clears.load(Ordering::SeqCst),
            0,
            "A is still active → no clear"
        );
        assert_eq!(
            engine.current_activity().unwrap().state,
            "Editing",
            "A promoted"
        );
        assert_eq!(&*last_state.lock().unwrap(), &Some("Editing".to_string()));
    }

    #[test]
    fn unsupported_foreground_no_active_sources_still_clears() {
        // When nothing supported remains active, the outputs are cleared even
        // though the foreground window is unsupported.
        let (mut engine, _publishes, clears, _last) = tracking_engine();

        engine.update("A", &test_activity("Editing"));
        engine.set_foreground_source(Some("A"));
        engine.set_foreground_source(None);
        engine.end_session("A");
        assert_eq!(
            clears.load(Ordering::SeqCst),
            1,
            "no active source → clear outputs"
        );
        assert!(engine.current_activity().is_none());
    }

    #[test]
    fn ownership_default_unsupported_foreground_is_keep_last() {
        assert_eq!(
            PresenceEngine::new().unsupported_foreground(),
            UnsupportedForegroundPolicy::KeepLast
        );
        assert_eq!(
            Config::default().presence.unsupported_foreground,
            UnsupportedForegroundPolicy::KeepLast
        );
    }

    #[test]
    fn ownership_fixed_and_recent_ignore_unsupported_foreground() {
        // Under the fixed and recent policies the foreground window is
        // ignored entirely, so an unsupported window can never revoke the
        // chosen owner.
        for policy in [OwnershipPolicy::Fixed, OwnershipPolicy::Recent] {
            let (mut engine, _publishes, clears, _last) = tracking_engine();
            engine.set_policy(policy);
            engine.update("A", &test_activity("Editing"));
            engine.update("B", &test_activity("InGame"));
            let owner = engine.current_activity().unwrap().state.clone();

            engine.set_foreground_source(None);
            assert_eq!(clears.load(Ordering::SeqCst), 0);
            assert_eq!(engine.current_activity().unwrap().state, owner);
        }
    }

    #[test]
    fn ownership_missing_or_invalid_config_falls_back_to_default() {
        // A config file without a `[presence]` section, or with an unknown
        // ownership value, must resolve to the default foreground policy
        // rather than failing to load.
        let missing: Config = toml::from_str("[outputs]\nconsole = true").unwrap();
        assert_eq!(missing.presence.ownership, OwnershipPolicy::Foreground);

        let invalid: Config = toml::from_str("[presence]\nownership = \"bogus\"").unwrap();
        assert_eq!(invalid.presence.ownership, OwnershipPolicy::Foreground);
    }

    #[test]
    fn ownership_policy_switch_while_active() {
        // Switching the policy live must recalculate ownership immediately.
        let (mut engine, _publishes, clears, last_state) = tracking_engine();

        engine.update("A", &test_activity("Editing"));
        engine.update("B", &test_activity("InGame"));
        assert_eq!(engine.current_activity().unwrap().state, "Editing");

        engine.set_policy(OwnershipPolicy::Recent);
        assert_eq!(
            engine.current_activity().unwrap().state,
            "InGame",
            "B is most recent"
        );
        assert_eq!(&*last_state.lock().unwrap(), &Some("InGame".to_string()));
        assert_eq!(clears.load(Ordering::SeqCst), 0);
    }

    // -- ConsoleOutput behavior --------------------------------------------------

    #[test]
    fn console_output_last_activity_none_when_new() {
        let output = ConsoleOutput::new();
        assert!(output.last_activity().is_none());
    }

    #[test]
    fn console_output_complete_pipeline() {
        // Validate the complete flow:
        // Activity -> ConsoleOutput -> last_activity matches
        let mut output = ConsoleOutput::new();
        let mut metadata = HashMap::new();
        metadata.insert("application".to_string(), "FL Studio".to_string());
        metadata.insert("version".to_string(), "21".to_string());
        metadata.insert("unsaved".to_string(), "true".to_string());

        let activity = Activity {
            state: "Editing".to_string(),
            details: Some("Project: song.flp*".to_string()),
            timestamps: None,
            metadata,
            application: None,
        };

        output.publish("FL Studio", &activity).unwrap();
        let last = output.last_activity();
        assert_eq!(last.unwrap().state, "Editing");
        assert_eq!(
            last.unwrap().details,
            Some("Project: song.flp*".to_string())
        );
    }
}
