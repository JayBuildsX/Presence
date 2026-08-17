//! Activity mapping.
//!
//! Pure conversion from a [`LeagueSnapshot`] (plus an optional game start
//! timestamp) to a canonical [`Activity`]. The mapping knows nothing about
//! Discord or any output — it only produces the application-agnostic model
//! that every PresenceHub output consumes.
//!
//! # Presence wording
//!
//! The user-facing text is normalized here: raw Riot/LCU identifiers are
//! translated to PresenceHub-friendly names via [`mode_name`] and
//! [`map_name`]. The application name ("PresenceHub") never appears in the
//! text, and raw queue/map ids are never exposed in the primary presence
//! lines — unknown ids stay in metadata only.
//!
//! The Discord client renders its own first line ("Playing {application}",
//! e.g. "Playing League of Legends"), so the primary `state` line here
//! carries the champion and mode ("Ahri • Draft Pick") and the secondary
//! `details` line carries the map ("Summoner's Rift").

use std::collections::HashMap;

use presencehub_core::activity::{Activity, ActivityTimestamps};

use crate::state::{map_name, mode_name, LeagueSnapshot, LeagueState};

/// Application identifier for League activities.
///
/// Set as the first-class `Activity::application` field and mirrored in the
/// `metadata["application"]` key (backward compatibility). The generic
/// presence layer derives the large image/text from it.
pub const APPLICATION_NAME: &str = "League of Legends";

/// The Discord asset key for the League of Legends icon.
///
/// This is a real, registered asset in the PresenceHub Discord application.
/// Setting it explicitly (via `metadata["large_image"]`) prevents Discord
/// from showing a `?` for the unregistered derived slug (`"leagueoflegends"`).
pub const LARGE_IMAGE_KEY: &str = "league";

/// Build a canonical Activity from a snapshot.
///
/// Returns `None` when the snapshot represents a client that is not running
/// ([`LeagueState::NotRunning`]).
///
/// `game_started_at` is the Unix timestamp (seconds) at which the current
/// game started; it is only attached while the player is in a game.
pub fn map_activity(snapshot: &LeagueSnapshot, game_started_at: Option<i64>) -> Option<Activity> {
    let state = snapshot.state();
    if state == LeagueState::NotRunning {
        return None;
    }

    let (state_line, details) = describe(&state, snapshot);

    let mut metadata = HashMap::new();
    metadata.insert("application".to_string(), APPLICATION_NAME.to_string());
    metadata.insert("large_image".to_string(), LARGE_IMAGE_KEY.to_string());
    metadata.insert("phase".to_string(), snapshot.phase.clone());
    if let Some(ref version) = snapshot.version {
        metadata.insert("version".to_string(), version.clone());
    }
    if let Some(ref name) = snapshot.queue_name {
        metadata.insert("queue".to_string(), name.clone());
    } else if let Some(id) = snapshot.queue_id {
        metadata.insert("queue".to_string(), id.to_string());
    }
    if let Some(ref champ) = snapshot.champion_name {
        metadata.insert("champion".to_string(), champ.clone());
    } else if let Some(id) = snapshot.champion_id {
        metadata.insert("champion_id".to_string(), id.to_string());
    }
    if let Some(map) = snapshot.map_id {
        // Prefer the normalized map name; fall back to the numeric id so an
        // unknown map is still preserved in metadata without leaking a raw
        // Riot string into the primary presence text.
        metadata.insert(
            "map".to_string(),
            map_name(map).unwrap_or_else(|| map.to_string()),
        );
    }
    if let (Some(ref tier), Some(ref rank)) = (&snapshot.tier, &snapshot.rank) {
        metadata.insert("rank".to_string(), format!("{} {}", tier, rank));
    }

    let timestamps = if matches!(state, LeagueState::InProgress | LeagueState::Reconnect) {
        Some(ActivityTimestamps {
            start: game_started_at,
            end: None,
        })
    } else {
        None
    };

    let application_name = APPLICATION_NAME;
    Some(Activity {
        state: state_line,
        details,
        timestamps,
        application: Some(application_name.to_string()),
        metadata,
    })
}

/// The primary status line and secondary details for a state.
///
/// The primary line (`state`) carries the most prominent user-facing text
/// (e.g. "Ahri • Draft Pick" while in game), and the secondary line
/// (`details`) carries the map. This follows the existing PresenceHub
/// architecture where `state` is the primary Discord line and `details` the
/// secondary.
fn describe(state: &LeagueState, snapshot: &LeagueSnapshot) -> (String, Option<String>) {
    match state {
        LeagueState::ClientOpen => ("Idle".to_string(), None),
        LeagueState::Lobby => ("In Lobby".to_string(), snapshot.queue_name.clone()),
        LeagueState::Matchmaking => ("Matchmaking".to_string(), snapshot.queue_name.clone()),
        LeagueState::ReadyCheck => ("Match Found".to_string(), snapshot.queue_name.clone()),
        LeagueState::ChampionSelect => (
            "Champion Select".to_string(),
            snapshot.champion_name.clone(),
        ),
        LeagueState::InProgress => in_game_line(snapshot),
        LeagueState::WaitingForStats => ("Viewing Stats".to_string(), queue_or_champion(snapshot)),
        LeagueState::EndOfGame => ("Game Over".to_string(), queue_or_champion(snapshot)),
        LeagueState::Reconnect => ("Reconnecting".to_string(), queue_or_champion(snapshot)),
        LeagueState::TerminatedInError => ("Client Error".to_string(), None),
        LeagueState::Unknown(_) => ("In League of Legends".to_string(), None),
        LeagueState::NotRunning => ("Not Running".to_string(), None),
    }
}

/// Whether the snapshot is a Teamfight Tactics game.
///
/// TFT has no champion-select champion and its games run on a Summoner's
/// Rift map id, so the presence shows only the mode name and never forces a
/// champion or map line.
fn is_tft(snapshot: &LeagueSnapshot) -> bool {
    snapshot.queue_name.as_deref() == Some("Teamfight Tactics")
        || snapshot.queue_id.and_then(mode_name).as_deref() == Some("Teamfight Tactics")
}

/// The normalized mode name for a snapshot, falling back to the application
/// name so the line is never empty. Raw queue ids are never returned.
fn mode_text(snapshot: &LeagueSnapshot) -> String {
    snapshot
        .queue_name
        .clone()
        .or_else(|| snapshot.queue_id.and_then(mode_name))
        .unwrap_or_else(|| APPLICATION_NAME.to_string())
}

/// The primary and secondary lines for an in-game presence.
///
/// The primary line combines the champion and the normalized mode
/// ("Ahri • Draft Pick"); the secondary line is the map. Teamfight Tactics
/// shows only the mode (no champion, no forced map). When the mode is
/// unknown the numeric id is never leaked into the primary line: the
/// champion stands alone, and the status label "In Game" is the final
/// fallback.
fn in_game_line(snapshot: &LeagueSnapshot) -> (String, Option<String>) {
    if is_tft(snapshot) {
        return (mode_text(snapshot), None);
    }
    let mode = snapshot
        .queue_name
        .clone()
        .or_else(|| snapshot.queue_id.and_then(mode_name));
    let state = match (&snapshot.champion_name, &mode) {
        (Some(champion), Some(mode)) => format!("{champion} • {mode}"),
        (Some(champion), None) => champion.clone(),
        (None, Some(mode)) => mode.clone(),
        (None, None) => "In Game".to_string(),
    };
    (state, snapshot.map_id.and_then(map_name))
}

/// Prefer the champion name, falling back to the queue/mode name.
///
/// The champion is the player-facing context for the post-game and
/// reconnect states (the same live game's champion persists). The
/// queue/mode is shown only when no champion is known.
fn queue_or_champion(snapshot: &LeagueSnapshot) -> Option<String> {
    snapshot
        .champion_name
        .clone()
        .or_else(|| snapshot.queue_name.clone())
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
    fn not_running_maps_to_none() {
        let snap = LeagueSnapshot {
            connected: false,
            phase: "InProgress".to_string(),
            ..LeagueSnapshot::default()
        };
        assert!(map_activity(&snap, None).is_none());
    }

    #[test]
    fn client_open_is_idle() {
        let a = map_activity(&connected("None"), None).unwrap();
        assert_eq!(a.state, "Idle");
        assert!(a.details.is_none());
        assert_eq!(
            a.metadata.get("application"),
            Some(&APPLICATION_NAME.to_string())
        );
    }

    #[test]
    fn lobby_uses_queue_name() {
        let mut snap = connected("Lobby");
        snap.queue_name = Some("Ranked Solo/Duo".to_string());
        let a = map_activity(&snap, None).unwrap();
        assert_eq!(a.state, "In Lobby");
        assert_eq!(a.details.as_deref(), Some("Ranked Solo/Duo"));
        assert_eq!(
            a.metadata.get("queue"),
            Some(&"Ranked Solo/Duo".to_string())
        );
    }

    #[test]
    fn canonical_states_use_stable_external_labels() {
        // The canonical states must surface exactly the stable labels so
        // consumers never see raw LCU phase strings in the status line.
        for (phase, expected) in [
            ("None", "Idle"),
            ("Lobby", "In Lobby"),
            ("Matchmaking", "Matchmaking"),
            ("ChampSelect", "Champion Select"),
        ] {
            let a = map_activity(&connected(phase), Some(123)).unwrap();
            assert_eq!(
                a.state, expected,
                "phase {phase:?} must map to canonical state"
            );
        }
    }

    #[test]
    fn champ_select_prefers_champion() {
        let mut snap = connected("ChampSelect");
        snap.champion_name = Some("Annie".to_string());
        snap.queue_name = Some("ARAM".to_string());
        let a = map_activity(&snap, None).unwrap();
        assert_eq!(a.state, "Champion Select");
        assert_eq!(a.details.as_deref(), Some("Annie"));
        assert_eq!(a.metadata.get("champion"), Some(&"Annie".to_string()));
    }

    #[test]
    fn in_progress_attaches_timestamps() {
        let mut snap = connected("InProgress");
        snap.champion_name = Some("Yasuo".to_string());
        let a = map_activity(&snap, Some(1234567890)).unwrap();
        assert_eq!(a.state, "Yasuo", "no mode known -> champion stands alone");
        assert!(a.details.is_none(), "no map known -> no detail line");
        let ts = a.timestamps.unwrap();
        assert_eq!(ts.start, Some(1234567890));
        assert!(ts.end.is_none());
    }

    #[test]
    fn in_progress_shows_champion_mode_and_map() {
        // The primary line is the champion and mode ("Hecarim • Draft Pick")
        // and the secondary line is the map. Discord renders its own
        // "Playing League of Legends" first line.
        let mut snap = connected("InProgress");
        snap.champion_name = Some("Hecarim".to_string());
        snap.queue_name = Some("Draft Pick".to_string());
        snap.map_id = Some(11);
        let a = map_activity(&snap, Some(1234567890)).unwrap();
        assert_eq!(a.state, "Hecarim • Draft Pick");
        assert_eq!(a.details.as_deref(), Some("Summoner's Rift"));
        assert_eq!(a.metadata.get("champion"), Some(&"Hecarim".to_string()));
        assert_eq!(a.metadata.get("map"), Some(&"Summoner's Rift".to_string()));
        assert_eq!(a.metadata.get("queue"), Some(&"Draft Pick".to_string()));
    }

    #[test]
    fn in_progress_without_map_shows_champion_and_mode() {
        // No map known: the champion and mode remain the primary line and
        // the detail line is omitted.
        let mut snap = connected("InProgress");
        snap.champion_name = Some("Ahri".to_string());
        snap.queue_name = Some("ARAM".to_string());
        let a = map_activity(&snap, Some(1234567890)).unwrap();
        assert_eq!(a.state, "Ahri • ARAM");
        assert!(a.details.is_none());
    }

    #[test]
    fn aram_in_progress_presence_contains_champion_map_and_mode() {
        // ARAM regression: with the champion surviving champ-select teardown,
        // the in-game presence must show the champion and the mode as the
        // primary line and the map as the detail line. The mode name is the
        // client-provided display name ("ARAM: Mayhem") for an unknown queue
        // id — never a raw numeric id.
        let mut snap = connected("InProgress");
        snap.queue_id = Some(458);
        snap.queue_name = Some("ARAM: Mayhem".to_string());
        snap.map_id = Some(12);
        snap.champion_name = Some("Ahri".to_string());
        snap.champion_id = Some(103);
        let a = map_activity(&snap, Some(1234567890)).unwrap();

        assert_eq!(a.state, "Ahri • ARAM: Mayhem");
        assert_eq!(a.details.as_deref(), Some("Howling Abyss"));
        assert_eq!(a.metadata.get("champion"), Some(&"Ahri".to_string()));
        assert_eq!(a.metadata.get("queue"), Some(&"ARAM: Mayhem".to_string()));
        assert_eq!(a.metadata.get("map"), Some(&"Howling Abyss".to_string()));
        assert_eq!(
            a.metadata.get("application"),
            Some(&"League of Legends".to_string())
        );
        assert_eq!(a.metadata.get("large_image"), Some(&"league".to_string()));
    }

    #[test]
    fn in_progress_without_champion_or_map_shows_plain_mode() {
        // A client that connects mid-game has no champion/map; the mode
        // still appears in the primary line.
        let mut snap = connected("InProgress");
        snap.queue_name = Some("ARAM".to_string());
        let a = map_activity(&snap, Some(1234567890)).unwrap();
        assert_eq!(a.state, "ARAM");
        assert!(a.details.is_none());
    }

    #[test]
    fn in_progress_without_mode_shows_in_game_label() {
        // No champion, no mode, no map: the stable "In Game" status label is
        // shown instead of leaking a raw queue id or the app name into the
        // mode slot.
        let mut snap = connected("InProgress");
        snap.queue_id = Some(999_999);
        let a = map_activity(&snap, Some(1234567890)).unwrap();
        assert_eq!(a.state, "In Game");
        assert!(a.details.is_none());
        assert_eq!(a.metadata.get("queue"), Some(&"999999".to_string()));
    }

    #[test]
    fn non_game_states_have_no_timestamps() {
        let a = map_activity(&connected("Lobby"), Some(1234567890)).unwrap();
        assert!(a.timestamps.is_none());
    }

    #[test]
    fn metadata_contains_version_and_phase() {
        let mut snap = connected("Matchmaking");
        snap.version = Some("16.15.8024387".to_string());
        let a = map_activity(&snap, None).unwrap();
        assert_eq!(
            a.metadata.get("version"),
            Some(&"16.15.8024387".to_string())
        );
        assert_eq!(a.metadata.get("phase"), Some(&"Matchmaking".to_string()));
    }

    #[test]
    fn rank_metadata_combined() {
        let mut snap = connected("InProgress");
        snap.tier = Some("DIAMOND".to_string());
        snap.rank = Some("II".to_string());
        let a = map_activity(&snap, None).unwrap();
        assert_eq!(a.metadata.get("rank"), Some(&"DIAMOND II".to_string()));
    }

    #[test]
    fn unknown_phase_is_safe() {
        let snap = connected("BrandNewPhase");
        let a = map_activity(&snap, None).unwrap();
        assert_eq!(a.state, "In League of Legends");
    }

    #[test]
    fn reconnect_and_error_states() {
        assert_eq!(
            map_activity(&connected("Reconnect"), None).unwrap().state,
            "Reconnecting"
        );
        assert_eq!(
            map_activity(&connected("TerminatedInError"), None)
                .unwrap()
                .state,
            "Client Error"
        );
    }

    #[test]
    fn reconnect_preserves_champion_and_game_timer() {
        // A reconnect is the same live game: the champion is shown and the
        // game timer keeps running (start timestamp attached).
        let mut snap = connected("Reconnect");
        snap.champion_name = Some("Ahri".to_string());
        let a = map_activity(&snap, Some(1234567890)).unwrap();
        assert_eq!(a.state, "Reconnecting");
        assert_eq!(a.details.as_deref(), Some("Ahri"));
        let ts = a.timestamps.unwrap();
        assert_eq!(
            ts.start,
            Some(1234567890),
            "game timer continues through reconnect"
        );
        assert!(ts.end.is_none());
    }

    #[test]
    fn waiting_for_stats_is_distinct_post_game_state() {
        // WaitingForStats is its own state with post-game context: the
        // champion is shown as the primary detail, no fresh-session behavior,
        // and active-game timing ended. The queue/mode remains in metadata.
        let mut snap = connected("WaitingForStats");
        snap.champion_name = Some("Lux".to_string());
        snap.queue_name = Some("ARAM".to_string());
        let a = map_activity(&snap, Some(1234567890)).unwrap();
        assert_eq!(a.state, "Viewing Stats");
        assert_eq!(
            a.details.as_deref(),
            Some("Lux"),
            "champion is the post-game detail"
        );
        assert_eq!(a.metadata.get("champion"), Some(&"Lux".to_string()));
        assert_eq!(a.metadata.get("queue"), Some(&"ARAM".to_string()));
        assert!(a.timestamps.is_none(), "game timing ends after the game");
    }

    #[test]
    fn end_of_game_keeps_champion_context() {
        let mut snap = connected("EndOfGame");
        snap.champion_name = Some("Yasuo".to_string());
        let a = map_activity(&snap, None).unwrap();
        assert_eq!(a.state, "Game Over");
        assert_eq!(a.details.as_deref(), Some("Yasuo"));
    }

    #[test]
    fn unknown_queue_falls_back_to_numeric_id() {
        // A queue id with no canonical name must not crash; the numeric id
        // is preserved in metadata and used as the fallback detail.
        let mut snap = connected("Lobby");
        snap.queue_id = Some(999_999);
        snap.queue_name = None;
        let a = map_activity(&snap, None).unwrap();
        assert_eq!(a.metadata.get("queue"), Some(&"999999".to_string()));
    }

    #[test]
    fn client_provided_queue_name_used_when_no_canonical_mapping() {
        let mut snap = connected("InProgress");
        snap.queue_id = Some(999_998);
        snap.queue_name = Some("Brand New Mode".to_string());
        let a = map_activity(&snap, None).unwrap();
        assert_eq!(a.metadata.get("queue"), Some(&"Brand New Mode".to_string()));
        assert_eq!(a.state, "Brand New Mode");
    }

    #[test]
    fn waiting_stats_falls_back_to_queue_when_no_champion() {
        let mut snap = connected("WaitingForStats");
        snap.queue_name = Some("ARAM".to_string());
        let a = map_activity(&snap, None).unwrap();
        assert_eq!(a.state, "Viewing Stats");
        assert_eq!(a.details.as_deref(), Some("ARAM"));
    }

    #[test]
    fn reconnect_falls_back_to_queue_when_no_champion() {
        let mut snap = connected("Reconnect");
        snap.queue_name = Some("Ranked Solo/Duo".to_string());
        let a = map_activity(&snap, Some(42)).unwrap();
        assert_eq!(a.state, "Reconnecting");
        assert_eq!(a.details.as_deref(), Some("Ranked Solo/Duo"));
    }

    #[test]
    fn champion_id_metadata_when_name_unavailable() {
        // Champion resolution failing gracefully: the numeric id is kept in
        // metadata when the display name could not be resolved.
        let mut snap = connected("ChampSelect");
        snap.champion_id = Some(99);
        snap.champion_name = None;
        let a = map_activity(&snap, None).unwrap();
        assert_eq!(a.metadata.get("champion_id"), Some(&"99".to_string()));
        assert!(!a.metadata.contains_key("champion"));
    }

    #[test]
    fn map_metadata_uses_normalized_name() {
        // The map metadata must carry the user-facing name, not the raw id.
        let mut snap = connected("InProgress");
        snap.map_id = Some(11);
        let a = map_activity(&snap, None).unwrap();
        assert_eq!(a.metadata.get("map"), Some(&"Summoner's Rift".to_string()));
    }

    #[test]
    fn map_metadata_falls_back_to_numeric_id_for_unknown() {
        let mut snap = connected("InProgress");
        snap.map_id = Some(999);
        let a = map_activity(&snap, None).unwrap();
        assert_eq!(a.metadata.get("map"), Some(&"999".to_string()));
    }

    #[test]
    fn league_asset_key_is_explicit() {
        // Regression: the League icon must resolve. The plugin sets an
        // explicit `large_image` metadata key pointing at a real Discord
        // asset, so the generic layer does not fall back to the unregistered
        // derived slug ("leagueoflegends") which Discord renders as `?`.
        let a = map_activity(&connected("InProgress"), None).unwrap();
        assert_eq!(
            a.metadata.get("large_image"),
            Some(&LARGE_IMAGE_KEY.to_string())
        );
        assert_eq!(a.metadata.get("large_image"), Some(&"league".to_string()));
    }

    #[test]
    fn league_application_identity() {
        // Regression: the application identity is a first-class field, not
        // just a metadata convention. It must be "League of Legends" so the
        // generic layer renders the correct application, never "PresenceHub".
        let a = map_activity(&connected("InProgress"), None).unwrap();
        assert_eq!(a.application.as_deref(), Some("League of Legends"));
        assert_ne!(a.application.as_deref(), Some("PresenceHub"));
        assert_eq!(
            a.metadata.get("application"),
            Some(&"League of Legends".to_string())
        );
    }

    #[test]
    fn league_in_progress_contains_champion() {
        // The champion must appear in the primary presence line, not only in
        // metadata.
        let mut snap = connected("InProgress");
        snap.champion_name = Some("Yasuo".to_string());
        snap.queue_name = Some("Draft Pick".to_string());
        snap.map_id = Some(11);
        let a = map_activity(&snap, Some(1234567890)).unwrap();
        assert_eq!(a.state, "Yasuo • Draft Pick");
        assert!(
            a.state.contains("Yasuo"),
            "champion must be visible in the presence"
        );
        assert_eq!(a.details.as_deref(), Some("Summoner's Rift"));
    }

    #[test]
    fn league_aram_presence() {
        let mut snap = connected("InProgress");
        snap.champion_name = Some("Kai'Sa".to_string());
        snap.queue_name = Some("ARAM".to_string());
        snap.map_id = Some(12);
        let a = map_activity(&snap, Some(1234567890)).unwrap();
        assert_eq!(a.state, "Kai'Sa • ARAM");
        assert_eq!(a.details.as_deref(), Some("Howling Abyss"));
        assert_eq!(a.metadata.get("map"), Some(&"Howling Abyss".to_string()));
    }

    #[test]
    fn league_tft_presence() {
        // TFT must show only the mode: no champion and no forced map, even
        // though TFT games run on a Summoner's Rift map id.
        let mut snap = connected("InProgress");
        snap.champion_name = Some("Ahri".to_string());
        snap.queue_name = Some("Teamfight Tactics".to_string());
        snap.queue_id = Some(1100);
        snap.map_id = Some(11);
        let a = map_activity(&snap, Some(1234567890)).unwrap();
        assert_eq!(a.state, "Teamfight Tactics");
        assert!(a.details.is_none(), "TFT must not force a map line");
        assert!(!a.state.contains("Ahri"), "TFT must not show a champion");
    }

    #[test]
    fn league_unknown_queue_never_leaks_numeric_id() {
        // An unknown queue id must not appear in the primary presence line;
        // it is preserved in metadata only.
        let mut snap = connected("InProgress");
        snap.queue_id = Some(123_456);
        snap.queue_name = None;
        snap.champion_name = Some("Ahri".to_string());
        let a = map_activity(&snap, Some(1234567890)).unwrap();
        assert_eq!(a.state, "Ahri");
        assert!(!a.state.contains("123456"));
        assert_eq!(a.metadata.get("queue"), Some(&"123456".to_string()));
    }

    #[test]
    fn league_unknown_map_never_leaks_numeric_id() {
        // An unknown map id must not appear in the primary presence line; it
        // is preserved in metadata only.
        let mut snap = connected("InProgress");
        snap.queue_name = Some("Draft Pick".to_string());
        snap.map_id = Some(999);
        let a = map_activity(&snap, Some(1234567890)).unwrap();
        assert_eq!(a.state, "Draft Pick");
        assert!(a.details.is_none(), "unknown map -> no detail line");
        assert_eq!(a.metadata.get("map"), Some(&"999".to_string()));
    }

    #[test]
    fn map_activity_never_crashes_on_any_state() {
        let mut snap = connected("None");
        snap.queue_id = Some(420);
        snap.queue_name = None;
        snap.champion_id = Some(1);
        snap.champion_name = None;
        snap.map_id = Some(11);
        snap.tier = Some("IRON".to_string());
        snap.rank = Some("I".to_string());
        snap.version = Some("16.15.8024387".to_string());

        for phase in [
            "",
            "None",
            "Lobby",
            "Matchmaking",
            "ReadyCheck",
            "ChampSelect",
            "GameStart",
            "InProgress",
            "Started",
            "WaitingForStats",
            "PreEndOfGame",
            "EndOfGame",
            "Reconnect",
            "TerminatedInError",
            "TotallyUnknown",
        ] {
            snap.phase = phase.to_string();
            let _ = map_activity(&snap, Some(1234567890));
        }
    }
}
