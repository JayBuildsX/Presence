//! PresenceHub League of Legends plugin.
//!
//! Observes the local League Client through the League Client Update (LCU)
//! protocol — read-only — and produces canonical PresenceHub [`Activity`]
//! values.
//!
//! # How it works
//!
//! 1. A worker thread discovers the client via its `lockfile`, connects over
//!    loopback HTTPS, and subscribes to the WebSocket event feed.
//! 2. Observed data is folded into a [`LeagueSnapshot`] shared with the
//!    plugin's synchronous `poll`.
//! 3. `poll` maps the snapshot to an [`Activity`] and applies change
//!    detection (duplicate suppression with an application-exit reset, in the
//!    same pattern as the FL Studio plugin).
//!
//! # Security
//!
//! * Read-only: no endpoint is ever modified.
//! * Loopback-only TLS with a verifier that rejects any non-loopback address.
//! * The LCU token is never logged.

use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use presencehub_core::activity::Activity;
use presencehub_plugin_host::{Plugin, PluginError, PluginMetadata, WindowIdentity};

mod detection;
mod lcu;
mod mapping;
mod state;

pub use detection::{parse_lockfile, read_lockfile, LeagueConnectionInfo, LockfileLocator};
pub use mapping::map_activity;
pub use state::{queue_name, state_from_phase, LeagueSnapshot, LeagueState};

/// The League of Legends plugin.
///
/// # Example
///
/// ```rust
/// use presencehub_league::LeaguePlugin;
/// use presencehub_plugin_host::Plugin;
///
/// let mut plugin = LeaguePlugin::new();
/// assert_eq!(plugin.metadata().name, "League of Legends");
/// ```
pub struct LeaguePlugin {
    /// Plugin metadata.
    metadata: PluginMetadata,
    /// Shared snapshot holder updated by the LCU worker thread.
    shared: Arc<lcu::Shared>,
    /// Worker thread handle (spawned on `init`).
    handle: Option<std::thread::JoinHandle<()>>,
    /// Last emitted activity, used for change detection.
    last_activity: Option<Activity>,
    /// Unix timestamp (seconds) when the current game started.
    game_started_at: Option<i64>,
}

impl LeaguePlugin {
    /// Creates a new League plugin.
    ///
    /// The worker thread is not started until [`Plugin::init`] is called, so
    /// construction is cheap and side-effect free.
    pub fn new() -> Self {
        Self {
            metadata: PluginMetadata::new("League of Legends", "0.1.0"),
            shared: Arc::new(lcu::Shared::default()),
            handle: None,
            last_activity: None,
            game_started_at: None,
        }
    }

    /// Creates a plugin without spawning the worker thread.
    ///
    /// Used by tests to drive the deduplication state machine directly.
    #[cfg(test)]
    fn for_tests() -> Self {
        Self {
            handle: None,
            ..Self::new()
        }
    }

    /// Replaces the shared snapshot (test helper).
    #[cfg(test)]
    fn set_snapshot(&self, snapshot: LeagueSnapshot) {
        if let Ok(mut guard) = self.shared.snapshot.lock() {
            *guard = snapshot;
        }
    }

    /// Handle the League client being absent.
    ///
    /// Forgets the previous activity so the next identical activity is
    /// published again when the client reopens, then returns the poll error
    /// the runtime uses to end the session.
    fn client_not_found(&mut self) -> Result<Option<Activity>, PluginError> {
        self.last_activity = None;
        self.game_started_at = None;
        Err(PluginError::PollFailed(
            "League of Legends client not running".to_string(),
        ))
    }

    /// Observe a snapshot and apply change detection.
    ///
    /// Split out of [`Plugin::poll`] so the deduplication state machine can
    /// be tested without a live League client.
    fn observe_snapshot(
        &mut self,
        snapshot: &LeagueSnapshot,
    ) -> Result<Option<Activity>, PluginError> {
        let state = snapshot.state();

        if state == LeagueState::NotRunning {
            return self.client_not_found();
        }

        // Stamp the game start timestamp once when the game begins. It is
        // preserved through Reconnect (still the same live game) and cleared
        // only when the player actually leaves the game.
        if state == LeagueState::InProgress || state == LeagueState::Reconnect {
            if self.game_started_at.is_none() {
                self.game_started_at = Some(now_secs());
            }
        } else {
            self.game_started_at = None;
        }

        let Some(activity) = mapping::map_activity(snapshot, self.game_started_at) else {
            return Ok(None);
        };

        // Only emit if the activity changed.
        if self.last_activity.as_ref() != Some(&activity) {
            self.last_activity = Some(activity.clone());
            return Ok(Some(activity));
        }

        Ok(None)
    }
}

impl Default for LeaguePlugin {
    fn default() -> Self {
        Self::new()
    }
}

impl Plugin for LeaguePlugin {
    fn metadata(&self) -> &PluginMetadata {
        &self.metadata
    }

    fn window_identity(&self) -> Option<WindowIdentity> {
        // The League client (and in-game process) use generic Chromium window
        // classes, so process base names are the reliable identifier. The
        // CEF class is intentionally not matched — it is shared by many apps.
        Some(WindowIdentity::new(
            [
                "LeagueClientUx.exe".to_string(),
                "League of Legends.exe".to_string(),
            ],
            Vec::<String>::new(),
        ))
    }

    fn init(&mut self) -> Result<(), PluginError> {
        if self.handle.is_none() {
            match lcu::spawn(self.shared.clone()) {
                Some(handle) => self.handle = Some(handle),
                None => {
                    return Err(PluginError::InitFailed(
                        "failed to spawn League LCU worker thread".to_string(),
                    ));
                }
            }
        }
        Ok(())
    }

    fn poll(&mut self) -> Result<Option<Activity>, PluginError> {
        // Recover a poisoned snapshot lock: if the worker panicked while
        // holding the guard, the last written snapshot is still used instead
        // of falling back to a blank (NotRunning) snapshot that would end the
        // presence session.
        let snapshot = self
            .shared
            .snapshot
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        self.observe_snapshot(&snapshot)
    }

    fn shutdown(&mut self) -> Result<(), PluginError> {
        if let Some(handle) = self.handle.take() {
            self.shared.shutdown.store(true, Ordering::Relaxed);
            // The worker checks the flag at most every 2s and exits promptly.
            let _ = handle.join();
        }
        Ok(())
    }
}

/// Current Unix time in seconds.
fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn connected(phase: &str) -> LeagueSnapshot {
        LeagueSnapshot {
            connected: true,
            phase: phase.to_string(),
            ..LeagueSnapshot::default()
        }
    }

    #[test]
    fn plugin_metadata() {
        let plugin = LeaguePlugin::new();
        assert_eq!(plugin.metadata().name, "League of Legends");
        assert_eq!(plugin.metadata().version, "0.1.0");
    }

    #[test]
    fn plugin_init_and_shutdown_without_worker() {
        // Without init, shutdown must be a no-op and succeed.
        let mut plugin = LeaguePlugin::for_tests();
        assert!(plugin.shutdown().is_ok());
        assert!(plugin.init().is_ok());
    }

    #[test]
    fn poll_returns_err_when_client_not_running() {
        let mut plugin = LeaguePlugin::for_tests();
        plugin.set_snapshot(LeagueSnapshot::default());
        assert!(Plugin::poll(&mut plugin).is_err());
    }

    #[test]
    fn poll_recovers_from_poisoned_snapshot_lock() {
        let mut plugin = LeaguePlugin::for_tests();
        plugin.set_snapshot(connected("InProgress"));

        // Simulate the worker panicking while holding the snapshot lock.
        let shared = plugin.shared.clone();
        let poisoned = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _guard = shared.snapshot.lock().unwrap();
            panic!("simulated worker panic while holding lock");
        }));
        assert!(
            poisoned.is_err(),
            "test precondition: lock must be poisoned"
        );

        // poll() must recover the last written snapshot (still in-game)
        // instead of a blank NotRunning snapshot that would end the session.
        let activity = plugin
            .poll()
            .unwrap()
            .expect("recovered connected snapshot must produce an activity");
        assert_eq!(activity.state, "In Game");
    }

    #[test]
    fn poll_publishes_lobby_activity() {
        let mut plugin = LeaguePlugin::for_tests();
        let mut snap = connected("Lobby");
        snap.queue_name = Some("ARAM".to_string());
        plugin.set_snapshot(snap);
        let activity = plugin.poll().unwrap().expect("should publish");
        assert_eq!(activity.state, "In Lobby");
    }

    #[test]
    fn plugin_suppresses_identical_activity_while_running() {
        let mut plugin = LeaguePlugin::for_tests();
        let mut snap = connected("Lobby");
        snap.queue_name = Some("ARAM".to_string());

        plugin.set_snapshot(snap.clone());
        assert!(plugin.poll().unwrap().is_some());

        plugin.set_snapshot(snap.clone());
        assert!(plugin.poll().unwrap().is_none());
    }

    #[test]
    fn plugin_republishes_activity_after_client_restart() {
        let mut plugin = LeaguePlugin::for_tests();
        let mut snap = connected("Lobby");
        snap.queue_name = Some("ARAM".to_string());

        // Open lobby -> published.
        plugin.set_snapshot(snap.clone());
        assert!(plugin.poll().unwrap().is_some());

        // Poll again -> suppressed.
        plugin.set_snapshot(snap.clone());
        assert!(plugin.poll().unwrap().is_none());

        // Client exits -> poll() forgets the previous activity and errors.
        plugin.set_snapshot(LeagueSnapshot::default());
        assert!(plugin.poll().is_err());

        // Reopen the same lobby -> must publish again, not be suppressed.
        plugin.set_snapshot(snap.clone());
        assert!(plugin.poll().unwrap().is_some());
    }

    #[test]
    fn game_start_timestamp_stamped_once() {
        let mut plugin = LeaguePlugin::for_tests();
        let mut snap = connected("InProgress");
        snap.champion_name = Some("Yasuo".to_string());

        plugin.set_snapshot(snap.clone());
        let first = plugin.poll().unwrap().expect("should publish");
        let start = first.timestamps.and_then(|t| t.start);
        assert!(
            start.is_some(),
            "in-progress activity must carry a start timestamp"
        );

        // Same game, next poll: identical activity is suppressed.
        plugin.set_snapshot(snap.clone());
        assert!(plugin.poll().unwrap().is_none());

        // Timestamp is not re-stamped on later polls.
        let stamped = plugin.game_started_at;
        assert_eq!(stamped, start);
    }

    #[test]
    fn game_start_timestamp_cleared_when_leaving_game() {
        let mut plugin = LeaguePlugin::for_tests();

        plugin.set_snapshot(connected("InProgress"));
        let _ = plugin.poll().unwrap();
        assert!(plugin.game_started_at.is_some());

        plugin.set_snapshot(connected("Lobby"));
        let _ = plugin.poll().unwrap();
        assert!(plugin.game_started_at.is_none());
    }

    #[test]
    fn unknown_phase_does_not_crash_poll() {
        let mut plugin = LeaguePlugin::for_tests();
        plugin.set_snapshot(connected("MysteryPhase"));
        let activity = plugin.poll().unwrap().expect("should publish");
        assert_eq!(activity.state, "In League of Legends");
    }

    // -- Phase-transition and reconnect edge cases -------------------------------

    #[test]
    fn game_start_timestamp_survives_reconnect() {
        // Reconnect is the same live game: the game timer must not reset.
        let mut plugin = LeaguePlugin::for_tests();

        plugin.set_snapshot(connected("InProgress"));
        let first = plugin.poll().unwrap().expect("should publish");
        let start = first.timestamps.and_then(|t| t.start);
        assert!(start.is_some());

        // Player's client drops but the game is still live; the client
        // reports the Reconnect phase.
        plugin.set_snapshot(connected("Reconnect"));
        let reconnecting = plugin.poll().unwrap().expect("should publish");
        assert_eq!(reconnecting.state, "Reconnecting");
        let ts = reconnecting.timestamps.unwrap();
        assert_eq!(ts.start, start, "game timer preserved across reconnect");
        assert!(ts.end.is_none());

        // Back in the game: same start, no duplicate session.
        plugin.set_snapshot(connected("InProgress"));
        let back = plugin.poll().unwrap().expect("should publish");
        assert_eq!(back.timestamps.and_then(|t| t.start), start);
    }

    #[test]
    fn reconnect_does_not_reset_session_or_champion() {
        // Champion must survive the InProgress -> Reconnect -> InProgress
        // cycle, and the plugin must not treat the reconnect as a fresh game.
        let mut plugin = LeaguePlugin::for_tests();

        let mut in_game = connected("InProgress");
        in_game.champion_name = Some("Ahri".to_string());
        plugin.set_snapshot(in_game);
        let first = plugin.poll().unwrap().expect("should publish");
        let start = first.timestamps.and_then(|t| t.start);

        let mut reconnect = connected("Reconnect");
        reconnect.champion_name = Some("Ahri".to_string());
        plugin.set_snapshot(reconnect);
        let reconnecting = plugin.poll().unwrap().expect("should publish");
        assert_eq!(reconnecting.details.as_deref(), Some("Ahri"));

        // The session timer must be unchanged after the cycle.
        plugin.set_snapshot(connected("InProgress"));
        let back = plugin.poll().unwrap().expect("should publish");
        assert_eq!(back.timestamps.and_then(|t| t.start), start);
    }

    #[test]
    fn leaving_game_ends_timing_and_clears_context() {
        // WaitingForStats -> EndOfGame -> Lobby: the game timer ends and the
        // champion context is dropped once the player leaves the game.
        let mut plugin = LeaguePlugin::for_tests();

        plugin.set_snapshot(connected("InProgress"));
        let _ = plugin.poll().unwrap();
        assert!(plugin.game_started_at.is_some());

        plugin.set_snapshot(connected("WaitingForStats"));
        let stats = plugin.poll().unwrap().expect("should publish");
        assert_eq!(stats.state, "Viewing Stats");
        assert!(stats.timestamps.is_none(), "timing ends once the game ends");
        assert!(plugin.game_started_at.is_none());

        plugin.set_snapshot(connected("Lobby"));
        let lobby = plugin.poll().unwrap().expect("should publish");
        assert_eq!(lobby.state, "In Lobby");
    }

    #[test]
    fn champion_carries_from_champ_select_into_game() {
        // The champion chosen in champ select must appear in the in-game
        // activity. The snapshot is what lcu.rs produces: the champion is
        // preserved across the ChampSelect -> InProgress transition (see
        // `LeagueState::preserves_game_context`) even though the champ-select
        // session disappears in-game.
        let mut plugin = LeaguePlugin::for_tests();

        let mut champ_select = connected("ChampSelect");
        champ_select.champion_name = Some("Lux".to_string());
        champ_select.champion_id = Some(99);
        champ_select.queue_name = Some("ARAM".to_string());
        plugin.set_snapshot(champ_select);
        let cs = plugin.poll().unwrap().expect("should publish");
        assert_eq!(cs.state, "Champion Select");
        assert_eq!(cs.details.as_deref(), Some("Lux"));

        let mut in_game = connected("InProgress");
        in_game.champion_name = Some("Lux".to_string());
        in_game.champion_id = Some(99);
        in_game.queue_name = Some("ARAM".to_string());
        plugin.set_snapshot(in_game);
        let ig = plugin.poll().unwrap().expect("should publish");
        assert_eq!(
            ig.state, "Lux • ARAM",
            "champion and mode are the primary in-game line"
        );
        assert_eq!(ig.metadata.get("champion"), Some(&"Lux".to_string()));
    }

    #[test]
    fn unknown_queue_does_not_crash_and_shows_fallback() {
        // A brand-new queue id must not crash any transition and must keep
        // the numeric id as the fallback name.
        let mut plugin = LeaguePlugin::for_tests();
        for phase in [
            "Lobby",
            "Matchmaking",
            "ReadyCheck",
            "ChampSelect",
            "InProgress",
        ] {
            let mut snap = connected(phase);
            snap.queue_id = Some(123456);
            snap.queue_name = None;
            plugin.set_snapshot(snap);
            let activity = plugin.poll().unwrap();
            if let Some(activity) = activity {
                assert_eq!(
                    activity.metadata.get("queue"),
                    Some(&"123456".to_string()),
                    "numeric queue fallback for {phase}"
                );
            }
        }
    }

    #[test]
    fn practice_tool_queue_detected() {
        // Practice Tool (queue 2000) is a supported mode name.
        let mut plugin = LeaguePlugin::for_tests();
        let mut snap = connected("InProgress");
        snap.queue_id = Some(2000);
        snap.queue_name = Some("Practice Tool".to_string());
        plugin.set_snapshot(snap);
        let activity = plugin.poll().unwrap().expect("should publish");
        assert_eq!(
            activity.metadata.get("queue"),
            Some(&"Practice Tool".to_string())
        );
        assert_eq!(activity.state, "Practice Tool");
    }

    #[test]
    fn client_restart_into_reconnect_still_publishes() {
        // A full client restart that lands back in a live game (Reconnect)
        // must publish fresh (the previous activity was forgotten) and stamp
        // a fresh game timer.
        let mut plugin = LeaguePlugin::for_tests();

        let mut snap = connected("InProgress");
        snap.champion_name = Some("Yasuo".to_string());
        plugin.set_snapshot(snap.clone());
        assert!(plugin.poll().unwrap().is_some());
        assert!(plugin.poll().unwrap().is_none(), "unchanged suppressed");

        // Client exits (disconnected) -> session error.
        plugin.set_snapshot(LeagueSnapshot::default());
        assert!(plugin.poll().is_err());

        // Client restarts directly into Reconnect.
        plugin.set_snapshot(connected("Reconnect"));
        let activity = plugin.poll().unwrap().expect("must publish after restart");
        assert_eq!(activity.state, "Reconnecting");
        assert!(
            activity.timestamps.and_then(|t| t.start).is_some(),
            "a fresh timer is stamped for the recovered game"
        );
    }

    #[test]
    fn matchmaking_transitions_through_ready_check_and_champ_select() {
        // The standard pre-game transition chain publishes each state.
        let mut plugin = LeaguePlugin::for_tests();
        let mut snap = connected("Matchmaking");
        snap.queue_name = Some("Ranked Solo/Duo".to_string());

        plugin.set_snapshot(snap.clone());
        let a = plugin.poll().unwrap().expect("publish");
        assert_eq!(a.state, "Matchmaking");
        assert_eq!(a.details.as_deref(), Some("Ranked Solo/Duo"));

        plugin.set_snapshot(snap.clone());
        assert!(
            plugin.poll().unwrap().is_none(),
            "unchanged matchmaking suppressed"
        );

        snap.phase = "ReadyCheck".to_string();
        plugin.set_snapshot(snap.clone());
        let a = plugin.poll().unwrap().expect("publish");
        assert_eq!(a.state, "Match Found");

        snap.phase = "ChampSelect".to_string();
        plugin.set_snapshot(snap);
        let a = plugin.poll().unwrap().expect("publish");
        assert_eq!(a.state, "Champion Select");
    }

    #[test]
    fn end_of_game_then_client_open_resets_state() {
        // After the stats screens, returning to the client home emits Idle.
        let mut plugin = LeaguePlugin::for_tests();

        plugin.set_snapshot(connected("EndOfGame"));
        let a = plugin.poll().unwrap().expect("publish");
        assert_eq!(a.state, "Game Over");

        plugin.set_snapshot(connected("None"));
        let a = plugin.poll().unwrap().expect("publish");
        assert_eq!(a.state, "Idle");
    }

    #[test]
    fn terminated_in_error_maps_safely() {
        let mut plugin = LeaguePlugin::for_tests();
        plugin.set_snapshot(connected("TerminatedInError"));
        let a = plugin.poll().unwrap().expect("publish");
        assert_eq!(a.state, "Client Error");
    }

    #[test]
    fn disconnect_clears_game_timer() {
        // Losing the LCU connection (client closed) clears the in-game timer
        // so a later fresh game starts a new timer.
        let mut plugin = LeaguePlugin::for_tests();

        plugin.set_snapshot(connected("InProgress"));
        let _ = plugin.poll().unwrap();
        assert!(plugin.game_started_at.is_some());

        plugin.set_snapshot(LeagueSnapshot::default());
        assert!(plugin.poll().is_err());
        assert!(plugin.game_started_at.is_none());
    }

    // -- Phase 3.3 edge cases ------------------------------------------------------

    #[test]
    fn dodge_returns_to_lobby_cleanly() {
        // A champ-select dodge returns the player to the lobby. The champion
        // context is dropped, the queue/mode is kept, and no game timer was
        // ever started.
        let mut plugin = LeaguePlugin::for_tests();

        let mut champ_select = connected("ChampSelect");
        champ_select.champion_name = Some("Darius".to_string());
        champ_select.queue_name = Some("Ranked Solo/Duo".to_string());
        champ_select.map_id = Some(11);
        plugin.set_snapshot(champ_select);
        let cs = plugin.poll().unwrap().expect("should publish");
        assert_eq!(cs.state, "Champion Select");
        assert!(
            plugin.game_started_at.is_none(),
            "champ select is not a live game"
        );

        let mut lobby = connected("Lobby");
        lobby.queue_name = Some("Ranked Solo/Duo".to_string());
        plugin.set_snapshot(lobby);
        let back = plugin.poll().unwrap().expect("should publish");
        assert_eq!(back.state, "In Lobby", "dodge returns to the lobby cleanly");
        assert_eq!(back.details.as_deref(), Some("Ranked Solo/Duo"));
        assert!(
            !back.metadata.contains_key("champion"),
            "champion context dropped"
        );
        assert_eq!(
            back.metadata.get("queue"),
            Some(&"Ranked Solo/Duo".to_string())
        );
        assert!(plugin.game_started_at.is_none());
    }

    #[test]
    fn failed_game_launch_clears_timer_and_returns_to_lobby() {
        // A game that fails to launch (GameStart -> back to lobby) must end
        // the in-game timer so the next successful game gets a fresh one.
        let mut plugin = LeaguePlugin::for_tests();

        let mut starting = connected("GameStart");
        starting.champion_name = Some("Garen".to_string());
        plugin.set_snapshot(starting);
        let _ = plugin.poll().unwrap();
        assert!(
            plugin.game_started_at.is_some(),
            "GameStart is treated as live"
        );

        // The launch fails; the player returns to the client home.
        plugin.set_snapshot(connected("None"));
        let home = plugin.poll().unwrap().expect("should publish");
        assert_eq!(home.state, "Idle");
        assert!(
            plugin.game_started_at.is_none(),
            "failed launch must clear the timer"
        );
    }

    #[test]
    fn reconnect_preserves_queue_and_champion() {
        // Reconnect is the same live game: champion and queue/mode survive
        // the InProgress -> Reconnect -> InProgress cycle.
        let mut plugin = LeaguePlugin::for_tests();

        let mut in_game = connected("InProgress");
        in_game.champion_name = Some("Ahri".to_string());
        in_game.queue_name = Some("Ranked Solo/Duo".to_string());
        plugin.set_snapshot(in_game);
        let first = plugin.poll().unwrap().expect("should publish");
        let start = first.timestamps.and_then(|t| t.start);

        let mut reconnect = connected("Reconnect");
        reconnect.champion_name = Some("Ahri".to_string());
        reconnect.queue_name = Some("Ranked Solo/Duo".to_string());
        plugin.set_snapshot(reconnect);
        let reconnecting = plugin.poll().unwrap().expect("should publish");
        assert_eq!(reconnecting.state, "Reconnecting");
        assert_eq!(
            reconnecting.details.as_deref(),
            Some("Ahri"),
            "champion survives reconnect"
        );
        assert_eq!(
            reconnecting.metadata.get("champion"),
            Some(&"Ahri".to_string())
        );
        assert_eq!(
            reconnecting.metadata.get("queue"),
            Some(&"Ranked Solo/Duo".to_string())
        );

        let mut back_in = connected("InProgress");
        back_in.champion_name = Some("Ahri".to_string());
        back_in.queue_name = Some("Ranked Solo/Duo".to_string());
        plugin.set_snapshot(back_in);
        let back = plugin.poll().unwrap().expect("should publish");
        assert_eq!(
            back.timestamps.and_then(|t| t.start),
            start,
            "no duplicate session"
        );
        assert_eq!(back.state, "Ahri • Ranked Solo/Duo");
        assert_eq!(
            back.details.as_deref(),
            None,
            "no map known -> no detail line"
        );
    }

    #[test]
    fn waiting_for_stats_is_not_mistaken_for_active_game() {
        // WaitingForStats keeps the post-game context but must never carry a
        // live game timer.
        let mut plugin = LeaguePlugin::for_tests();

        let mut in_game = connected("InProgress");
        in_game.champion_name = Some("Yasuo".to_string());
        plugin.set_snapshot(in_game);
        let _ = plugin.poll().unwrap();
        assert!(plugin.game_started_at.is_some());

        let mut stats = connected("WaitingForStats");
        stats.champion_name = Some("Yasuo".to_string());
        stats.queue_name = Some("ARAM".to_string());
        plugin.set_snapshot(stats);
        let activity = plugin.poll().unwrap().expect("should publish");
        assert_eq!(activity.state, "Viewing Stats");
        assert!(
            activity.timestamps.is_none(),
            "post-game state is not a live game"
        );
        assert!(plugin.game_started_at.is_none());
    }

    #[test]
    fn unknown_phase_preserves_queue_metadata() {
        // A future phase string must not crash and should still carry any
        // queue context already observed.
        let mut plugin = LeaguePlugin::for_tests();
        let mut snap = connected("SomeFuturePhase");
        snap.queue_name = Some("ARAM".to_string());
        plugin.set_snapshot(snap);
        let activity = plugin.poll().unwrap().expect("should publish");
        assert_eq!(activity.state, "In League of Legends");
        assert_eq!(activity.metadata.get("queue"), Some(&"ARAM".to_string()));
    }

    #[test]
    fn tft_queue_is_data_driven() {
        // TFT queues resolve through the data-driven queue map without any
        // special-casing in the state logic, and the presence shows only the
        // mode: no champion, no forced map.
        let mut plugin = LeaguePlugin::for_tests();
        let mut snap = connected("InProgress");
        snap.queue_id = Some(1100);
        snap.queue_name = Some("Teamfight Tactics".to_string());
        snap.champion_name = Some("Ahri".to_string());
        snap.map_id = Some(11);
        plugin.set_snapshot(snap);
        let activity = plugin.poll().unwrap().expect("should publish");
        assert_eq!(
            activity.metadata.get("queue"),
            Some(&"Teamfight Tactics".to_string())
        );
        assert_eq!(activity.state, "Teamfight Tactics");
        assert!(activity.details.is_none(), "TFT must not force a map line");
        assert!(
            !activity.state.contains("Ahri"),
            "TFT must not show a champion"
        );
    }

    #[test]
    fn lobby_then_matchmaking_are_distinct_states() {
        // Lobby and Matchmaking must be distinct canonical states, both
        // preserving the queue/mode while the player searches.
        let mut plugin = LeaguePlugin::for_tests();

        let mut lobby = connected("Lobby");
        lobby.queue_name = Some("Ranked Solo/Duo".to_string());
        plugin.set_snapshot(lobby);
        let a = plugin.poll().unwrap().expect("publish");
        assert_eq!(a.state, "In Lobby");
        assert_eq!(a.details.as_deref(), Some("Ranked Solo/Duo"));

        let mut searching = connected("Matchmaking");
        searching.queue_name = Some("Ranked Solo/Duo".to_string());
        plugin.set_snapshot(searching);
        let b = plugin.poll().unwrap().expect("publish");
        assert_eq!(b.state, "Matchmaking", "matchmaking is a distinct state");
        assert_ne!(
            b.state, a.state,
            "lobby and matchmaking must not be conflated"
        );
        assert_eq!(b.details.as_deref(), Some("Ranked Solo/Duo"));
        assert_eq!(
            b.metadata.get("queue"),
            Some(&"Ranked Solo/Duo".to_string())
        );
    }

    #[test]
    fn matchmaking_cancelled_returns_to_lobby_cleanly() {
        // Cancelling the search returns the player to the lobby: the queue is
        // kept, no champion context leaks, and no game timer was started.
        let mut plugin = LeaguePlugin::for_tests();

        let mut searching = connected("Matchmaking");
        searching.queue_name = Some("ARAM".to_string());
        plugin.set_snapshot(searching);
        let mm = plugin.poll().unwrap().expect("publish");
        assert_eq!(mm.state, "Matchmaking");

        let mut lobby = connected("Lobby");
        lobby.queue_name = Some("ARAM".to_string());
        plugin.set_snapshot(lobby);
        let back = plugin.poll().unwrap().expect("publish");
        assert_eq!(back.state, "In Lobby");
        assert_eq!(back.details.as_deref(), Some("ARAM"));
        assert_eq!(back.metadata.get("queue"), Some(&"ARAM".to_string()));
        assert!(!back.metadata.contains_key("champion"));
        assert!(
            plugin.game_started_at.is_none(),
            "searching never starts a game timer"
        );
    }

    #[test]
    fn leaving_lobby_returns_to_idle_cleanly() {
        // Leaving the lobby returns to the client home: Idle with no stale
        // queue context.
        let mut plugin = LeaguePlugin::for_tests();

        let mut lobby = connected("Lobby");
        lobby.queue_name = Some("Normal Draft".to_string());
        lobby.champion_name = Some("Darius".to_string());
        plugin.set_snapshot(lobby);
        let a = plugin.poll().unwrap().expect("publish");
        assert_eq!(a.state, "In Lobby");
        assert_eq!(a.metadata.get("queue"), Some(&"Normal Draft".to_string()));

        plugin.set_snapshot(connected("None"));
        let idle = plugin.poll().unwrap().expect("publish");
        assert_eq!(idle.state, "Idle");
        assert!(idle.details.is_none());
        assert!(
            !idle.metadata.contains_key("queue"),
            "no stale queue at Idle"
        );
        assert!(
            !idle.metadata.contains_key("champion"),
            "no stale champion at Idle"
        );
    }

    #[test]
    fn champ_select_cancelled_returns_to_lobby_without_timer() {
        // A cancelled champion select (no game started) returns to the lobby
        // cleanly: champion dropped, queue kept, timer never started.
        let mut plugin = LeaguePlugin::for_tests();

        let mut champ_select = connected("ChampSelect");
        champ_select.champion_name = Some("Katarina".to_string());
        champ_select.queue_name = Some("Ranked Flex".to_string());
        plugin.set_snapshot(champ_select);
        let cs = plugin.poll().unwrap().expect("publish");
        assert_eq!(cs.state, "Champion Select");
        assert!(
            plugin.game_started_at.is_none(),
            "champ select is not a live game"
        );

        let mut lobby = connected("Lobby");
        lobby.queue_name = Some("Ranked Flex".to_string());
        plugin.set_snapshot(lobby);
        let back = plugin.poll().unwrap().expect("publish");
        assert_eq!(back.state, "In Lobby");
        assert_eq!(back.details.as_deref(), Some("Ranked Flex"));
        assert!(
            !back.metadata.contains_key("champion"),
            "cancelled select drops the champion"
        );
        assert!(plugin.game_started_at.is_none());
    }

    #[test]
    fn in_game_to_idle_clears_stale_context() {
        // In Game -> Idle must drop champion/map/queue metadata and the game
        // timer so a later Idle -> Lobby starts clean.
        let mut plugin = LeaguePlugin::for_tests();

        let mut in_game = connected("InProgress");
        in_game.champion_name = Some("Yasuo".to_string());
        in_game.champion_id = Some(777);
        in_game.queue_name = Some("Multiplayer Practice Tool Custom".to_string());
        in_game.queue_id = Some(3140);
        in_game.map_id = Some(11);
        plugin.set_snapshot(in_game);
        let ig = plugin.poll().unwrap().expect("publish");
        assert_eq!(ig.state, "Yasuo • Multiplayer Practice Tool Custom");
        assert!(plugin.game_started_at.is_some());
        assert_eq!(ig.metadata.get("map"), Some(&"Summoner's Rift".to_string()));

        plugin.set_snapshot(connected("None"));
        let idle = plugin.poll().unwrap().expect("publish");
        assert_eq!(idle.state, "Idle");
        assert!(
            plugin.game_started_at.is_none(),
            "timer cleared after the game"
        );
        assert!(
            !idle.metadata.contains_key("champion"),
            "stale champion dropped"
        );
        assert!(
            !idle.metadata.contains_key("champion_id"),
            "stale champion id dropped"
        );
        assert!(!idle.metadata.contains_key("map"), "stale map dropped");
        assert!(!idle.metadata.contains_key("queue"), "stale queue dropped");
    }

    #[test]
    fn custom_game_queue_detected() {
        // Custom games (queue 0) resolve through the canonical queue map.
        let mut plugin = LeaguePlugin::for_tests();
        let mut snap = connected("Lobby");
        snap.queue_id = Some(0);
        snap.queue_name = Some("Custom Game".to_string());
        plugin.set_snapshot(snap);
        let activity = plugin.poll().unwrap().expect("should publish");
        assert_eq!(activity.state, "In Lobby");
        assert_eq!(
            activity.metadata.get("queue"),
            Some(&"Custom Game".to_string())
        );
    }

    #[test]
    fn send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<LeaguePlugin>();
    }
}
