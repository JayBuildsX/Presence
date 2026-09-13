//! Discord serialization layer.
//!
//! Maps the generic [`RichPresence`] domain model to the Discord IPC
//! protocol's [`ActivityData`] payload. This is the only place in the crate
//! that knows how a Rich Presence becomes a Discord packet; it contains no
//! plugin-specific logic. Whatever a plugin expresses through the typed
//! model is rendered here, mechanically:
//!
//! ```text
//! RichPresence (core)
//!      │
//!      ▼
//! render::to_activity_data
//!      │
//!      ▼
//! ActivityData (protocol)
//!      │
//!      ▼
//! Frame → NamedPipeTransport → Discord
//! ```

use presencehub_core::presence::{PresenceActivityType, RichPresence};

use crate::client::build_activity_data;
use crate::protocol::{ActivityData, PartyData, SecretsData};

/// Map a [`PresenceActivityType`] onto the Discord activity `type` code.
///
/// `Playing` is the legacy default (`0`) and is rendered as `None` so the
/// serialized payload stays byte-for-byte identical to the pre-refactor
/// output. Discord treats a missing `type` as `Playing`.
fn activity_type_code(activity_type: PresenceActivityType) -> Option<u64> {
    match activity_type {
        PresenceActivityType::Playing => None,
        PresenceActivityType::Listening => Some(2),
        PresenceActivityType::Watching => Some(3),
        PresenceActivityType::Competing => Some(5),
    }
}

/// Render a [`RichPresence`] into the Discord [`ActivityData`] payload.
///
/// This is a pure, mechanical translation: every field of the typed model
/// is mapped onto the corresponding Discord protocol field, and fields the
/// presence does not carry are omitted. When the advanced fields (activity
/// type, buttons, party, secrets, instance) are unused, the output matches
/// the pre-refactor payload exactly.
pub fn to_activity_data(presence: &RichPresence) -> ActivityData {
    let timestamps = presence.timestamps.as_ref();
    let assets = presence.assets.as_ref();

    let large_image = assets.and_then(|a| a.large_image.as_deref());
    let small_image = assets.and_then(|a| a.small_image.as_deref());
    let large_text = if large_image.is_some() {
        assets.and_then(|a| a.large_text.as_deref())
    } else {
        None
    };
    let small_text = assets.and_then(|a| a.small_text.as_deref());

    let mut data = build_activity_data(
        presence.state.as_deref().unwrap_or_default(),
        presence.details.as_deref(),
        timestamps.and_then(|t| t.start),
        timestamps.and_then(|t| t.end),
        large_text,
        small_text,
        large_image,
        small_image,
    );

    data.r#type = activity_type_code(presence.activity_type);

    // Discord's IPC protocol carries only the button labels; the URLs are
    // configured on the Discord application. Up to two buttons are allowed.
    data.buttons = presence
        .buttons
        .iter()
        .map(|button| button.label.clone())
        .take(2)
        .collect();

    if let Some(party) = presence.party.as_ref() {
        data.party = Some(PartyData {
            id: party.id.clone(),
            size: party.size,
        });
    }

    if let Some(secrets) = presence.secrets.as_ref() {
        data.secrets = Some(SecretsData {
            join: secrets.join.clone(),
            spectate: secrets.spectate.clone(),
            r#match: secrets.r#match.clone(),
        });
    }

    if presence.instance {
        data.instance = Some(true);
    }

    data
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::{ActivityUpdateArgs, CommandEnvelope};
    use presencehub_core::activity::{Activity, ActivityTimestamps};
    use presencehub_core::presence::{
        PresenceActivityType, PresenceAssets, PresenceButton, PresenceParty, PresenceSecrets,
        PresenceTimestamps, RichPresence,
    };
    use std::collections::HashMap;

    /// The canonical activity exercised by the pre-refactor integration
    /// tests (application + version metadata driving the asset fields).
    fn canonical_flstudio_activity() -> Activity {
        let mut metadata = HashMap::new();
        metadata.insert("application".to_string(), "FL Studio".to_string());
        metadata.insert("version".to_string(), "21".to_string());

        Activity {
            state: "Editing".to_string(),
            details: Some("Project: song.flp*".to_string()),
            timestamps: Some(ActivityTimestamps {
                start: Some(1000),
                end: Some(2000),
            }),
            application: None,
            metadata,
        }
    }

    #[test]
    fn render_matches_legacy_builder_output() {
        // Regression: the refactored pipeline (Activity → RichPresence →
        // ActivityData) must produce exactly the payload the pre-refactor
        // DiscordOutput::publish produced via build_activity_data.
        let activity = canonical_flstudio_activity();
        let presence = RichPresence::from(&activity);
        let rendered = to_activity_data(&presence);

        let legacy = build_activity_data(
            "Editing",
            Some("Project: song.flp*"),
            Some(1000),
            Some(2000),
            Some("FL Studio"),
            Some("21"),
            Some("flstudio"),
            None,
        );

        let rendered_json = serde_json::to_value(&rendered).unwrap();
        let legacy_json = serde_json::to_value(&legacy).unwrap();
        assert_eq!(
            rendered_json, legacy_json,
            "renderer must preserve the pre-refactor payload"
        );
    }

    #[test]
    fn render_preserves_wire_payload_shape() {
        // Verify the serialized SET_ACTIVITY envelope is unchanged: state
        // always present, optional fields omitted when absent.
        let activity = canonical_flstudio_activity();
        let presence = RichPresence::from(&activity);
        let rendered = to_activity_data(&presence);

        let envelope = CommandEnvelope {
            cmd: "SET_ACTIVITY".to_string(),
            args: ActivityUpdateArgs {
                pid: 0,
                activity: Some(rendered),
            },
            nonce: "test-nonce".to_string(),
        };
        let json = serde_json::to_value(&envelope).unwrap();

        assert_eq!(json["cmd"], "SET_ACTIVITY");
        assert_eq!(json["args"]["activity"]["state"], "Editing");
        assert_eq!(json["args"]["activity"]["details"], "Project: song.flp*");
        assert_eq!(json["args"]["activity"]["timestamps"]["start"], 1000);
        assert_eq!(json["args"]["activity"]["timestamps"]["end"], 2000);
        assert_eq!(
            json["args"]["activity"]["assets"]["large_text"],
            "FL Studio"
        );
        assert_eq!(json["args"]["activity"]["assets"]["small_text"], "21");
        assert_eq!(
            json["args"]["activity"]["assets"]["large_image"],
            "flstudio"
        );
        assert!(
            json["args"]["activity"]["assets"]
                .get("small_image")
                .is_none(),
            "absent asset fields must stay omitted"
        );
    }

    #[test]
    fn render_never_sends_an_application_name_field() {
        // Architectural lock: Discord's "Playing <name>" line is the
        // registered name of the application whose client ID was sent in the
        // IPC handshake, and the Rich Presence protocol has no field to
        // change it at runtime. `Activity.application` maps to
        // `assets.large_text` only. This asserts PresenceHub never fabricates
        // an application "name" in the SET_ACTIVITY payload that could be
        // mistaken for control of that line.
        let mut metadata = HashMap::new();
        metadata.insert("large_image".to_string(), "antigravity_logo".to_string());

        let activity = Activity {
            state: "Dynamic Conversation Tracking".to_string(),
            details: Some("Implement conversation detection".to_string()),
            timestamps: None,
            application: Some("Antigravity".to_string()),
            metadata,
        };

        let presence = RichPresence::from(&activity);
        let json = serde_json::to_value(to_activity_data(&presence)).unwrap();

        assert!(
            json.get("name").is_none(),
            "the activity payload must never carry an application name field"
        );
        // The only application identity available is the asset hover text.
        assert_eq!(json["assets"]["large_text"], "Antigravity");
    }

    #[test]
    fn render_omits_large_text_when_large_image_is_absent() {
        let presence = RichPresence::builder()
            .state("Standalone Mode")
            .assets(PresenceAssets {
                large_image: None,
                large_text: Some("No Image Title".to_string()),
                small_image: None,
                small_text: None,
            })
            .build();

        let data = to_activity_data(&presence);
        assert!(
            data.assets.is_none(),
            "assets must be None when no large_image is present"
        );
    }

    #[test]
    fn render_antigravity_payload_uses_explicit_asset_key() {
        // Regression: the Antigravity plugin sets an explicit `large_image`
        // metadata key so the icon resolves. The generic conversion must
        // forward that key to the Discord payload instead of falling back to
        // the derived slug ("antigravity") which Discord renders as `?`.
        let mut metadata = HashMap::new();
        metadata.insert("large_image".to_string(), "antigravity_logo".to_string());

        let activity = Activity {
            state: "Dynamic Conversation Tracking".to_string(),
            details: Some("Implement conversation detection".to_string()),
            timestamps: None,
            application: Some("Antigravity".to_string()),
            metadata,
        };

        let presence = RichPresence::from(&activity);
        let rendered = to_activity_data(&presence);
        let assets = rendered
            .assets
            .as_ref()
            .expect("antigravity asset should be present");

        assert_eq!(assets.large_text.as_deref(), Some("Antigravity"));
        assert_eq!(assets.large_image.as_deref(), Some("antigravity_logo"));

        // The serialized payload must carry the real asset key, not the
        // unregistered derived slug.
        let json = serde_json::to_value(&rendered).unwrap();
        assert_eq!(json["assets"]["large_image"], "antigravity_logo");
        assert_ne!(json["assets"]["large_image"], "antigravity");
    }

    #[test]
    fn render_antigravity_payload_carries_application_task_and_heading() {
        // End-to-end: the Antigravity plugin's Activity (application identity,
        // section heading state, active task detail) must surface in the final
        // Discord payload in the expected fields.
        let mut metadata = HashMap::new();
        metadata.insert("large_image".to_string(), "antigravity_logo".to_string());

        let activity = Activity {
            state: "Dynamic Conversation Tracking".to_string(),
            details: Some("Implement conversation detection".to_string()),
            timestamps: Some(ActivityTimestamps {
                start: Some(1234567890),
                end: None,
            }),
            application: Some("Antigravity".to_string()),
            metadata,
        };

        let presence = RichPresence::from(&activity);
        let rendered = to_activity_data(&presence);

        let json = serde_json::to_value(&rendered).unwrap();
        // Application identity lands in the asset hover text.
        assert_eq!(json["assets"]["large_text"], "Antigravity");
        assert_eq!(json["assets"]["large_image"], "antigravity_logo");
        // Section name is state; active task is details.
        assert_eq!(json["state"], "Dynamic Conversation Tracking");
        assert_eq!(json["details"], "Implement conversation detection");
        assert_eq!(json["timestamps"]["start"], 1234567890);
    }

    #[test]
    fn render_omits_optional_fields_when_absent() {
        let activity = Activity {
            state: "Idle".to_string(),
            details: None,
            timestamps: None,
            metadata: HashMap::new(),
            application: None,
        };

        let presence = RichPresence::from(&activity);
        let rendered = to_activity_data(&presence);

        assert_eq!(rendered.state, "Idle");
        assert!(rendered.details.is_none());
        assert!(rendered.timestamps.is_none());
        assert!(rendered.assets.is_none());
    }

    #[test]
    fn render_accepts_presence_without_state() {
        // A presence constructed directly (not from an Activity) may omit
        // state; it must still render without panicking.
        let presence = RichPresence::default();
        let rendered = to_activity_data(&presence);
        assert_eq!(rendered.state, "");
        assert!(rendered.details.is_none());
        assert!(rendered.timestamps.is_none());
        assert!(rendered.assets.is_none());
    }

    // -- Advanced field serialization ------------------------------------------

    #[test]
    fn render_serializes_buttons_as_labels() {
        let presence = RichPresence::builder()
            .state("In game")
            .button(PresenceButton {
                label: "Join".to_string(),
                url: "https://example.com/join".to_string(),
            })
            .button(PresenceButton {
                label: "Spectate".to_string(),
                url: "https://example.com/spectate".to_string(),
            })
            .build();

        let rendered = to_activity_data(&presence);
        assert_eq!(
            rendered.buttons,
            vec!["Join".to_string(), "Spectate".to_string()]
        );
    }

    #[test]
    fn render_truncates_buttons_to_two() {
        let presence = RichPresence::builder()
            .state("In game")
            .button(PresenceButton {
                label: "One".to_string(),
                url: "https://a.example".to_string(),
            })
            .button(PresenceButton {
                label: "Two".to_string(),
                url: "https://b.example".to_string(),
            })
            .button(PresenceButton {
                label: "Three".to_string(),
                url: "https://c.example".to_string(),
            })
            .build();

        let rendered = to_activity_data(&presence);
        assert_eq!(rendered.buttons.len(), 2);
        assert_eq!(rendered.buttons, vec!["One".to_string(), "Two".to_string()]);
    }

    #[test]
    fn render_serializes_activity_types() {
        let listening = RichPresence::builder()
            .state("Listening")
            .activity_type(PresenceActivityType::Listening)
            .build();
        assert_eq!(to_activity_data(&listening).r#type, Some(2));

        let watching = RichPresence::builder()
            .state("Watching")
            .activity_type(PresenceActivityType::Watching)
            .build();
        assert_eq!(to_activity_data(&watching).r#type, Some(3));

        let competing = RichPresence::builder()
            .state("Competing")
            .activity_type(PresenceActivityType::Competing)
            .build();
        assert_eq!(to_activity_data(&competing).r#type, Some(5));
    }

    #[test]
    fn render_playing_type_is_omitted_for_legacy() {
        // Playing (the default) must not emit a `type` field so existing
        // payloads stay byte-for-byte identical.
        let presence = RichPresence::builder()
            .state("Editing")
            .activity_type(PresenceActivityType::Playing)
            .build();
        let rendered = to_activity_data(&presence);
        assert_eq!(rendered.r#type, None);
    }

    #[test]
    fn render_serializes_party() {
        let presence = RichPresence::builder()
            .state("In lobby")
            .party(PresenceParty {
                id: Some("lobby-1".to_string()),
                size: Some((2, 8)),
            })
            .build();

        let rendered = to_activity_data(&presence);
        let party = rendered.party.expect("party should be rendered");
        assert_eq!(party.id.as_deref(), Some("lobby-1"));
        assert_eq!(party.size, Some((2, 8)));
    }

    #[test]
    fn render_serializes_secrets() {
        let presence = RichPresence::builder()
            .state("Queued")
            .secrets(PresenceSecrets {
                join: Some("join-secret".to_string()),
                spectate: Some("spectate-secret".to_string()),
                r#match: Some("match-secret".to_string()),
            })
            .build();

        let rendered = to_activity_data(&presence);
        let secrets = rendered.secrets.expect("secrets should be rendered");
        assert_eq!(secrets.join.as_deref(), Some("join-secret"));
        assert_eq!(secrets.spectate.as_deref(), Some("spectate-secret"));
        assert_eq!(secrets.r#match.as_deref(), Some("match-secret"));
    }

    #[test]
    fn render_serializes_instance_when_true() {
        let presence = RichPresence::builder()
            .state("Ready")
            .instance(true)
            .build();
        let rendered = to_activity_data(&presence);
        assert_eq!(rendered.instance, Some(true));
    }

    #[test]
    fn render_omits_advanced_fields_when_unused() {
        // A presence that never touches the advanced fields must produce a
        // payload identical to the legacy output (no type/buttons/party/
        // secrets/instance keys).
        let presence = RichPresence::builder().state("Editing").build();
        let rendered = to_activity_data(&presence);

        assert_eq!(rendered.r#type, None);
        assert!(rendered.buttons.is_empty());
        assert!(rendered.party.is_none());
        assert!(rendered.secrets.is_none());
        assert!(rendered.instance.is_none());

        let json = serde_json::to_string(&rendered).unwrap();
        assert!(!json.contains("\"type\""));
        assert!(!json.contains("\"buttons\""));
        assert!(!json.contains("\"party\""));
        assert!(!json.contains("\"secrets\""));
        assert!(!json.contains("\"instance\""));
    }

    #[test]
    fn render_full_feature_payload_json_is_stable() {
        let presence = RichPresence::builder()
            .state("In game")
            .details("Ranked match")
            .activity_type(PresenceActivityType::Competing)
            .timestamps(PresenceTimestamps {
                start: Some(1000),
                end: Some(2000),
            })
            .assets(PresenceAssets {
                large_image: Some("game".to_string()),
                large_text: Some("My Game".to_string()),
                small_image: Some("badge".to_string()),
                small_text: Some("Rank 5".to_string()),
            })
            .button(PresenceButton {
                label: "Join".to_string(),
                url: "https://example.com/join".to_string(),
            })
            .party(PresenceParty {
                id: Some("lobby-1".to_string()),
                size: Some((2, 8)),
            })
            .secrets(PresenceSecrets {
                join: Some("join-secret".to_string()),
                spectate: None,
                r#match: Some("match-secret".to_string()),
            })
            .instance(true)
            .build();

        let rendered = to_activity_data(&presence);
        let json = serde_json::to_value(&rendered).unwrap();

        assert_eq!(json["type"], 5);
        assert_eq!(json["buttons"], serde_json::json!(["Join"]));
        assert_eq!(json["party"]["id"], "lobby-1");
        assert_eq!(json["party"]["size"], serde_json::json!([2, 8]));
        assert_eq!(json["secrets"]["join"], "join-secret");
        assert_eq!(json["secrets"]["match"], "match-secret");
        assert!(json["secrets"].get("spectate").is_none());
        assert_eq!(json["instance"], true);
        assert_eq!(json["timestamps"]["start"], 1000);
        assert_eq!(json["assets"]["large_image"], "game");
    }

    #[test]
    fn render_builder_to_renderer_integration() {
        // The full pipeline: fluent builder -> RichPresence -> ActivityData.
        let presence = RichPresence::builder()
            .state("Producing")
            .details("Horizon.flp")
            .button(PresenceButton {
                label: "Open Project".to_string(),
                url: "https://example.com/open".to_string(),
            })
            .party(PresenceParty {
                id: None,
                size: Some((1, 1)),
            })
            .build();

        let rendered = to_activity_data(&presence);
        assert_eq!(rendered.state, "Producing");
        assert_eq!(rendered.details.as_deref(), Some("Horizon.flp"));
        assert_eq!(rendered.buttons, vec!["Open Project".to_string()]);
        assert_eq!(rendered.party.unwrap().size, Some((1, 1)));
    }
}
