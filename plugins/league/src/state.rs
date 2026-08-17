//! League state model.
//!
//! Maps the raw League Client API observations into a small, stable set of
//! high-level states that the plugin can render. The state machine is pure
//! and offline-testable: it never performs I/O.
//!
//! # Mode / map normalization
//!
//! Raw Riot/LCU identifiers (queue ids, map ids) are translated here into
//! PresenceHub-friendly, user-facing names. The pipeline is:
//!
//! ```text
//! Raw Riot/LCU identifiers
//!     ↓
//! normalized PresenceHub mode / map
//!     ↓
//! Discord presence
//! ```
//!
//! The normalization is explicit and documented: every known id maps to a
//! stable user-facing name, and unknown ids fall back safely (the numeric id
//! is preserved in metadata) rather than crashing or leaking a raw Riot
//! string to the user.

/// High-level states the League plugin understands.
///
/// Derived from the LCU `gameflow-phase` value. Unknown phase strings are
/// preserved as [`Unknown`](LeagueState::Unknown) so the plugin never
/// panics or silently drops data on a new client version.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LeagueState {
    /// Client is open and idle (phase `None`).
    ClientOpen,
    /// Player is in a lobby.
    Lobby,
    /// Client is searching for a match.
    Matchmaking,
    /// A match was found; awaiting accept.
    ReadyCheck,
    /// Player is in champion select.
    ChampionSelect,
    /// A game is running.
    InProgress,
    /// Post-game stats screen (phase `WaitingForStats`).
    WaitingForStats,
    /// Post-game stats screen (phase `EndOfGame`).
    EndOfGame,
    /// Client is attempting to reconnect to a game.
    Reconnect,
    /// The client terminated the game with an error.
    TerminatedInError,
    /// League client is not running / not reachable.
    NotRunning,
    /// An unrecognized phase string was observed.
    Unknown(String),
}

/// Maps a raw gameflow-phase string to a [`LeagueState`].
///
/// The empty string (a disconnected or freshly-cleared client) is treated as
/// [`ClientOpen`](LeagueState::ClientOpen); callers that need to distinguish
/// "no connection" use the snapshot's `connected` flag.
///
/// Unknown phase strings — e.g. a new phase introduced by a future client
/// version — are preserved as [`Unknown`](LeagueState::Unknown) rather than
/// crashing or being silently dropped.
pub fn state_from_phase(phase: &str) -> LeagueState {
    match phase {
        "" | "None" => LeagueState::ClientOpen,
        "Lobby" => LeagueState::Lobby,
        "Matchmaking" => LeagueState::Matchmaking,
        "ReadyCheck" | "CheckedIntoTournament" => LeagueState::ReadyCheck,
        "ChampSelect" => LeagueState::ChampionSelect,
        "GameStart" | "InProgress" | "Started" => LeagueState::InProgress,
        "WaitingForStats" => LeagueState::WaitingForStats,
        "PreEndOfGame" | "EndOfGame" => LeagueState::EndOfGame,
        "Reconnect" => LeagueState::Reconnect,
        "TerminatedInError" => LeagueState::TerminatedInError,
        other => LeagueState::Unknown(other.to_string()),
    }
}

impl LeagueState {
    /// Whether game-scoped context (champion, map, game timer) should be
    /// preserved for this state.
    ///
    /// Reconnect is still the same live game, so the champion and the game
    /// timer must survive a disconnect. `WaitingForStats` is the post-game
    /// stats screen and keeps the just-played champion so the plugin does
    /// not emit a fresh client session while the player reviews the match.
    pub fn preserves_game_context(&self) -> bool {
        matches!(
            self,
            LeagueState::ChampionSelect
                | LeagueState::InProgress
                | LeagueState::Reconnect
                | LeagueState::WaitingForStats
        )
    }
}

/// The observable League state as seen by the plugin.
///
/// Populated by the LCU transport (worker thread) and consumed read-only by
/// the plugin's `poll`. All fields are optional because the LCU endpoints
/// are not all available in every gameflow phase.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct LeagueSnapshot {
    /// Whether a connection to the LCU is currently established.
    pub connected: bool,
    /// The raw gameflow-phase string.
    pub phase: String,
    /// The lobby/champ-select queue id.
    pub queue_id: Option<i32>,
    /// Human-readable queue name.
    pub queue_name: Option<String>,
    /// Map id (e.g. 11 = Summoner's Rift).
    pub map_id: Option<i64>,
    /// Champion id of the local player.
    pub champion_id: Option<i32>,
    /// Champion display name of the local player.
    pub champion_name: Option<String>,
    /// Ranked tier (e.g. "DIAMOND").
    pub tier: Option<String>,
    /// Ranked division (e.g. "II").
    pub rank: Option<String>,
    /// Client version (e.g. "16.15.8024387").
    pub version: Option<String>,
}

impl LeagueSnapshot {
    /// The high-level state for this snapshot.
    pub fn state(&self) -> LeagueState {
        if !self.connected {
            LeagueState::NotRunning
        } else {
            state_from_phase(&self.phase)
        }
    }
}

/// Returns a human-readable name for a known queue id.
///
/// `None` for unknown/custom queue ids — callers fall back to the numeric id.
///
/// This is the raw queue-name mapping used by the LCU transport to resolve a
/// canonical name from a queue id. It is kept for backward compatibility;
/// new code should prefer [`mode_name`] which produces the normalized
/// user-facing mode.
pub fn queue_name(queue_id: i32) -> Option<String> {
    mode_name(queue_id)
}

/// Returns the normalized, user-facing game mode for a queue id.
///
/// This is the PresenceHub-friendly name shown to the user. It deliberately
/// does **not** include the application name (e.g. "PresenceHub") and does
/// **not** expose raw Riot queue ids. Unknown ids return `None` so callers
/// can fall back to the numeric id in metadata without leaking a raw string
/// into the primary presence text.
///
/// # Supported modes
///
/// * Draft Pick
/// * Ranked Solo/Duo
/// * Ranked Flex
/// * ARAM
/// * Teamfight Tactics
/// * Normal Blind Pick
/// * Swiftplay
/// * Quickplay
/// * Co-op vs. AI
/// * ARURF
/// * ARAM Clash
/// * Practice Tool
/// * Custom Game
pub fn mode_name(queue_id: i32) -> Option<String> {
    let name = match queue_id {
        0 => "Custom Game",
        2 | 430 => "Normal Blind Pick",
        4 | 420 => "Ranked Solo/Duo",
        6 | 440 => "Ranked Flex",
        400 => "Draft Pick",
        410 => "Ranked Draft",
        450 | 460 => "ARAM",
        480 => "Swiftplay",
        490 => "Quickplay",
        870 | 880 | 890 => "Co-op vs. AI",
        900 => "ARURF",
        720 => "ARAM Clash",
        1090 | 1100 | 1130 | 1160 => "Teamfight Tactics",
        2000 => "Practice Tool",
        _ => return None,
    };
    Some(name.to_string())
}

/// Returns the normalized, user-facing map name for a map id.
///
/// Only maps confirmed by the existing data are mapped. Unknown ids return
/// `None` so callers can fall back to the numeric id in metadata.
pub fn map_name(map_id: i64) -> Option<String> {
    let name = match map_id {
        1 | 2 | 11 => "Summoner's Rift",
        12 | 21 => "Howling Abyss",
        _ => return None,
    };
    Some(name.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    // -- state_from_phase ------------------------------------------------------

    #[test]
    fn phase_none_is_client_open() {
        assert_eq!(state_from_phase("None"), LeagueState::ClientOpen);
        assert_eq!(state_from_phase(""), LeagueState::ClientOpen);
    }

    #[test]
    fn known_phases_map_correctly() {
        assert_eq!(state_from_phase("Lobby"), LeagueState::Lobby);
        assert_eq!(state_from_phase("Matchmaking"), LeagueState::Matchmaking);
        assert_eq!(state_from_phase("ReadyCheck"), LeagueState::ReadyCheck);
        assert_eq!(state_from_phase("ChampSelect"), LeagueState::ChampionSelect);
        assert_eq!(state_from_phase("GameStart"), LeagueState::InProgress);
        assert_eq!(state_from_phase("InProgress"), LeagueState::InProgress);
        assert_eq!(
            state_from_phase("WaitingForStats"),
            LeagueState::WaitingForStats
        );
        assert_eq!(state_from_phase("EndOfGame"), LeagueState::EndOfGame);
        assert_eq!(state_from_phase("PreEndOfGame"), LeagueState::EndOfGame);
        assert_eq!(state_from_phase("Reconnect"), LeagueState::Reconnect);
        assert_eq!(
            state_from_phase("TerminatedInError"),
            LeagueState::TerminatedInError
        );
    }

    #[test]
    fn unknown_phase_is_preserved() {
        assert_eq!(
            state_from_phase("SomeNewPhase"),
            LeagueState::Unknown("SomeNewPhase".to_string())
        );
    }

    // -- snapshot --------------------------------------------------------------

    #[test]
    fn disconnected_snapshot_is_not_running() {
        let snap = LeagueSnapshot {
            connected: false,
            phase: "InProgress".to_string(),
            ..LeagueSnapshot::default()
        };
        assert_eq!(snap.state(), LeagueState::NotRunning);
    }

    #[test]
    fn connected_snapshot_uses_phase() {
        let snap = LeagueSnapshot {
            connected: true,
            phase: "ChampSelect".to_string(),
            ..LeagueSnapshot::default()
        };
        assert_eq!(snap.state(), LeagueState::ChampionSelect);
    }

    // -- queue_name (backward-compat) ------------------------------------------

    #[test]
    fn known_queue_ids_have_names() {
        assert_eq!(queue_name(420).as_deref(), Some("Ranked Solo/Duo"));
        assert_eq!(queue_name(400).as_deref(), Some("Draft Pick"));
        assert_eq!(queue_name(450).as_deref(), Some("ARAM"));
        assert_eq!(queue_name(440).as_deref(), Some("Ranked Flex"));
    }

    #[test]
    fn unknown_queue_ids_have_no_name() {
        assert_eq!(queue_name(1337), None);
    }

    #[test]
    fn practice_custom_and_tft_queues_have_names() {
        assert_eq!(queue_name(2000).as_deref(), Some("Practice Tool"));
        assert_eq!(queue_name(0).as_deref(), Some("Custom Game"));
        assert_eq!(queue_name(1090).as_deref(), Some("Teamfight Tactics"));
        assert_eq!(queue_name(1100).as_deref(), Some("Teamfight Tactics"));
        assert_eq!(queue_name(1130).as_deref(), Some("Teamfight Tactics"));
        assert_eq!(queue_name(1160).as_deref(), Some("Teamfight Tactics"));
    }

    #[test]
    fn future_unknown_queue_is_safe() {
        // A queue id introduced by a future patch must not crash; callers
        // fall back to the numeric id.
        assert_eq!(queue_name(999_999), None);
    }

    // -- mode_name (normalized user-facing mode) --------------------------------

    #[test]
    fn mode_name_covers_required_modes() {
        for (id, expected) in [
            (400, "Draft Pick"),
            (420, "Ranked Solo/Duo"),
            (440, "Ranked Flex"),
            (450, "ARAM"),
            (460, "ARAM"),
            (1090, "Teamfight Tactics"),
            (1100, "Teamfight Tactics"),
            (1130, "Teamfight Tactics"),
            (1160, "Teamfight Tactics"),
        ] {
            assert_eq!(mode_name(id).as_deref(), Some(expected), "queue {id}");
        }
    }

    #[test]
    fn mode_name_does_not_include_application_name() {
        // Regression: the mode must never contain "PresenceHub" or any
        // application branding.
        for id in [400, 420, 440, 450, 1090, 1100] {
            let name = mode_name(id).unwrap();
            assert!(
                !name.to_lowercase().contains("presencehub"),
                "mode {id} must not contain PresenceHub: {name}"
            );
        }
    }

    #[test]
    fn mode_name_unknown_is_none() {
        assert_eq!(mode_name(999_999), None);
    }

    #[test]
    fn mode_name_other_queues() {
        assert_eq!(mode_name(0).as_deref(), Some("Custom Game"));
        assert_eq!(mode_name(2).as_deref(), Some("Normal Blind Pick"));
        assert_eq!(mode_name(430).as_deref(), Some("Normal Blind Pick"));
        assert_eq!(mode_name(480).as_deref(), Some("Swiftplay"));
        assert_eq!(mode_name(490).as_deref(), Some("Quickplay"));
        assert_eq!(mode_name(870).as_deref(), Some("Co-op vs. AI"));
        assert_eq!(mode_name(880).as_deref(), Some("Co-op vs. AI"));
        assert_eq!(mode_name(890).as_deref(), Some("Co-op vs. AI"));
        assert_eq!(mode_name(900).as_deref(), Some("ARURF"));
        assert_eq!(mode_name(720).as_deref(), Some("ARAM Clash"));
        assert_eq!(mode_name(2000).as_deref(), Some("Practice Tool"));
    }

    // -- map_name --------------------------------------------------------------

    #[test]
    fn map_name_maps_known_maps() {
        assert_eq!(map_name(11).as_deref(), Some("Summoner's Rift"));
        assert_eq!(map_name(1).as_deref(), Some("Summoner's Rift"));
        assert_eq!(map_name(2).as_deref(), Some("Summoner's Rift"));
        assert_eq!(map_name(12).as_deref(), Some("Howling Abyss"));
        assert_eq!(map_name(21).as_deref(), Some("Howling Abyss"));
    }

    #[test]
    fn map_name_unknown_is_none() {
        assert_eq!(map_name(999), None);
    }

    // -- game-context preservation ----------------------------------------------

    #[test]
    fn preserves_game_context_for_live_game_states() {
        for phase in [
            "ChampSelect",
            "InProgress",
            "GameStart",
            "Reconnect",
            "WaitingForStats",
        ] {
            assert!(
                state_from_phase(phase).preserves_game_context(),
                "{phase} should preserve game context"
            );
        }
    }

    #[test]
    fn clears_game_context_outside_live_game() {
        for phase in [
            "",
            "None",
            "Lobby",
            "Matchmaking",
            "ReadyCheck",
            "EndOfGame",
            "PreEndOfGame",
            "TerminatedInError",
        ] {
            assert!(
                !state_from_phase(phase).preserves_game_context(),
                "{phase:?} should not preserve game context"
            );
        }
    }

    #[test]
    fn snapshot_state_maps_waiting_for_stats() {
        let snap = LeagueSnapshot {
            connected: true,
            phase: "WaitingForStats".to_string(),
            ..LeagueSnapshot::default()
        };
        assert_eq!(snap.state(), LeagueState::WaitingForStats);
    }

    #[test]
    fn major_queue_names_covered() {
        // The full required queue/mode coverage set.
        for (id, expected) in [
            (2, "Normal Blind Pick"),
            (4, "Ranked Solo/Duo"),
            (6, "Ranked Flex"),
            (400, "Draft Pick"),
            (420, "Ranked Solo/Duo"),
            (430, "Normal Blind Pick"),
            (440, "Ranked Flex"),
            (450, "ARAM"),
            (460, "ARAM"),
            (0, "Custom Game"),
            (2000, "Practice Tool"),
        ] {
            assert_eq!(queue_name(id).as_deref(), Some(expected), "queue {id}");
        }
    }
}
