//! Activity Model
//!
//! The canonical representation of a user's current activity.
//!
//! This module defines the shared data contract between all plugins
//! and outputs. Plugins produce [`Activity`] values, outputs consume
//! them, and the Presence Engine routes them without inspecting
//! their contents.
//!
//! The model is intentionally application-agnostic. It knows nothing
//! about FL Studio, VS Code, Discord, or any other application.

use std::collections::HashMap;

// ---------------------------------------------------------------------------
// Activity Timestamps
// ---------------------------------------------------------------------------

/// Temporal bounds for an [`Activity`].
///
/// Both fields are optional because an activity may have only a start
/// time (e.g. "started editing 5 minutes ago") or no temporal bounds
/// at all (e.g. "idle").
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ActivityTimestamps {
    /// Unix timestamp in seconds when the activity started.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start: Option<i64>,

    /// Unix timestamp in seconds when the activity is expected to end.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end: Option<i64>,
}

// ---------------------------------------------------------------------------
// Activity
// ---------------------------------------------------------------------------

/// The canonical representation of a user's current activity.
///
/// This is the shared data contract between all plugins and outputs.
/// Every plugin produces [`Activity`] values. Every output consumes them.
///
/// # Fields
///
/// * `state` — What the user is currently doing (e.g. "Playing", "Editing", "Idle").
///   This is the core semantic of the activity. Always required.
///
/// * `details` — Optional human-readable description. Provides additional
///   context beyond the state (e.g. "Project: My Song • BPM: 128").
///
/// * `timestamps` — Optional temporal bounds. Useful for presence displays
///   that show elapsed or remaining time.
///
/// * `application` — Optional application identifier. Plugins should set this
///   to their application name (e.g. "League of Legends", "FL Studio") so the
///   generic presence layer can render it without relying on metadata key
///   conventions. Defaults to `None`. When `None` and `metadata` contains an
///   `"application"` key, the metadata value is used as a
///   backward-compatible fallback.
///
/// * `metadata` — Extensible key-value store. Plugins use this to attach
///   application-specific data without modifying the Activity model. Defaults
///   to an empty map.
///
/// # Application Agnosticism
///
/// The Activity model contains no application-specific fields. It does
/// not know about:
///
/// * FL Studio (project names, tempo, playback state)
/// * VS Code (file names, language, workspace)
/// * Discord (rich presence fields, images, buttons)
/// * Any other application
///
/// Plugins express application-specific information through the `state` field
/// and the `metadata` map. This keeps the model stable while allowing
/// arbitrary extension.
///
/// # Example
///
/// ```rust
/// use presencehub_core::activity::{Activity, ActivityTimestamps};
/// use std::collections::HashMap;
///
/// let activity = Activity {
///     state: "Editing".to_string(),
///     details: Some("Working on main.rs".to_string()),
///     timestamps: Some(ActivityTimestamps {
///         start: Some(1234567890),
///         end: None,
///     }),
///     application: None,
///     metadata: HashMap::new(),
/// };
///
/// assert_eq!(activity.state, "Editing");
/// ```
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Activity {
    /// What the user is currently doing.
    ///
    /// Examples: "Playing", "Editing", "Idle", "Presenting", "Listening".
    ///
    /// This is the core semantic of the activity. It is always required
    /// because an activity without a state is meaningless.
    pub state: String,

    /// Optional human-readable description of the activity.
    ///
    /// Provides additional context beyond the state. For example, a
    /// "Playing" state might have details like "Level 5 - Forest Zone".
    ///
    /// This is optional because not all activities need description.
    /// An "Idle" state, for instance, is self-evident.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub details: Option<String>,

    /// Optional temporal bounds for the activity.
    ///
    /// When present, outputs can display elapsed or remaining time.
    /// This is optional because not all activities have a defined
    /// duration (e.g. "Idle" has no meaningful start time).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timestamps: Option<ActivityTimestamps>,

    /// Optional application identifier.
    ///
    /// Plugins should set this to their application name (e.g.
    /// "League of Legends", "FL Studio") so the generic presence layer
    /// can render it without relying on metadata key conventions.
    ///
    /// Defaults to `None`. When `None` and `metadata` contains an
    /// `"application"` key, the metadata value is used as a
    /// backward-compatible fallback.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub application: Option<String>,

    /// Extensible key-value metadata.
    ///
    /// Plugins use this to attach application-specific data without
    /// modifying the Activity model. The keys and values are
    /// application-defined strings.
    ///
    /// Defaults to an empty map. This is not optional because every
    /// activity logically has a (possibly empty) set of metadata.
    #[serde(default)]
    pub metadata: HashMap<String, String>,
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // -- Activity construction -------------------------------------------------

    #[test]
    fn activity_with_required_fields_only() {
        let activity = Activity {
            state: "Idle".to_string(),
            details: None,
            timestamps: None,
            metadata: HashMap::new(),
            application: None,
        };
        assert_eq!(activity.state, "Idle");
        assert!(activity.details.is_none());
        assert!(activity.timestamps.is_none());
        assert!(activity.metadata.is_empty());
    }

    #[test]
    fn activity_with_explicit_application_field() {
        // The application identity is a first-class optional field: it
        // round-trips through serialization and defaults to None when a
        // payload does not carry it.
        let activity = Activity {
            state: "Playing".to_string(),
            details: None,
            timestamps: None,
            application: Some("League of Legends".to_string()),
            metadata: HashMap::new(),
        };
        assert_eq!(activity.application.as_deref(), Some("League of Legends"));

        let json = serde_json::to_string(&activity).unwrap();
        let back: Activity = serde_json::from_str(&json).unwrap();
        assert_eq!(back, activity);

        let missing: Activity = serde_json::from_str(r#"{"state": "Idle"}"#).unwrap();
        assert!(
            missing.application.is_none(),
            "absent application defaults to None"
        );
    }

    #[test]
    fn activity_with_all_fields() {
        let activity = Activity {
            state: "Playing".to_string(),
            details: Some("Level 5".to_string()),
            timestamps: Some(ActivityTimestamps {
                start: Some(1000),
                end: Some(2000),
            }),
            application: Some("League of Legends".to_string()),
            metadata: {
                let mut m = HashMap::new();
                m.insert("score".to_string(), "9999".to_string());
                m
            },
        };
        assert_eq!(activity.state, "Playing");
        assert_eq!(activity.details, Some("Level 5".to_string()));
        assert_eq!(
            activity.timestamps,
            Some(ActivityTimestamps {
                start: Some(1000),
                end: Some(2000),
            })
        );
        assert_eq!(activity.metadata.get("score"), Some(&"9999".to_string()));
    }

    #[test]
    fn activity_with_partial_timestamps() {
        let activity = Activity {
            state: "Editing".to_string(),
            details: None,
            timestamps: Some(ActivityTimestamps {
                start: Some(1000),
                end: None,
            }),
            metadata: HashMap::new(),
            application: None,
        };
        assert_eq!(activity.timestamps.as_ref().unwrap().start, Some(1000));
        assert!(activity.timestamps.as_ref().unwrap().end.is_none());
    }

    // -- Activity comparison ---------------------------------------------------

    #[test]
    fn equal_activities_are_equal() {
        let a = Activity {
            state: "Playing".to_string(),
            details: None,
            timestamps: None,
            metadata: HashMap::new(),
            application: None,
        };
        let b = Activity {
            state: "Playing".to_string(),
            details: None,
            timestamps: None,
            metadata: HashMap::new(),
            application: None,
        };
        assert_eq!(a, b);
    }

    #[test]
    fn different_activities_are_not_equal() {
        let a = Activity {
            state: "Playing".to_string(),
            details: None,
            timestamps: None,
            metadata: HashMap::new(),
            application: None,
        };
        let b = Activity {
            state: "Idle".to_string(),
            details: None,
            timestamps: None,
            metadata: HashMap::new(),
            application: None,
        };
        assert_ne!(a, b);
    }

    // -- Activity serialization ------------------------------------------------

    #[test]
    fn activity_serializes_to_json() {
        let activity = Activity {
            state: "Playing".to_string(),
            details: Some("Level 5".to_string()),
            timestamps: Some(ActivityTimestamps {
                start: Some(1000),
                end: None,
            }),
            metadata: HashMap::new(),
            application: None,
        };
        let json = serde_json::to_string(&activity).unwrap();
        let deserialized: Activity = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized, activity);
    }

    #[test]
    fn activity_serializes_to_toml() {
        let activity = Activity {
            state: "Playing".to_string(),
            details: Some("Level 5".to_string()),
            timestamps: Some(ActivityTimestamps {
                start: Some(1000),
                end: None,
            }),
            metadata: {
                let mut m = HashMap::new();
                m.insert("score".to_string(), "9999".to_string());
                m
            },
            application: None,
        };
        let toml_str = toml::to_string(&activity).unwrap();
        let deserialized: Activity = toml::from_str(&toml_str).unwrap();
        assert_eq!(deserialized, activity);
    }

    #[test]
    fn activity_deserializes_from_json_with_optional_fields_missing() {
        let json = r#"{"state": "Idle"}"#;
        let activity: Activity = serde_json::from_str(json).unwrap();
        assert_eq!(activity.state, "Idle");
        assert!(activity.details.is_none());
        assert!(activity.timestamps.is_none());
        assert!(activity.metadata.is_empty());
    }

    #[test]
    fn activity_deserializes_from_toml_with_optional_fields_missing() {
        let toml_str = r#"state = "Idle""#;
        let activity: Activity = toml::from_str(toml_str).unwrap();
        assert_eq!(activity.state, "Idle");
        assert!(activity.details.is_none());
        assert!(activity.timestamps.is_none());
        assert!(activity.metadata.is_empty());
    }

    #[test]
    fn activity_serializes_and_deserializes_with_metadata() {
        let mut metadata = HashMap::new();
        metadata.insert("bpm".to_string(), "128".to_string());
        metadata.insert("project".to_string(), "my_song.flp".to_string());

        let activity = Activity {
            state: "Producing".to_string(),
            details: None,
            timestamps: None,
            metadata,
            application: None,
        };

        let json = serde_json::to_string(&activity).unwrap();
        let deserialized: Activity = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.metadata.get("bpm"), Some(&"128".to_string()));
        assert_eq!(
            deserialized.metadata.get("project"),
            Some(&"my_song.flp".to_string())
        );
    }

    // -- ActivityTimestamps tests ----------------------------------------------

    #[test]
    fn timestamps_with_only_start() {
        let ts = ActivityTimestamps {
            start: Some(1000),
            end: None,
        };
        assert_eq!(ts.start, Some(1000));
        assert!(ts.end.is_none());
    }

    #[test]
    fn timestamps_with_only_end() {
        let ts = ActivityTimestamps {
            start: None,
            end: Some(2000),
        };
        assert!(ts.start.is_none());
        assert_eq!(ts.end, Some(2000));
    }

    #[test]
    fn timestamps_with_both_fields() {
        let ts = ActivityTimestamps {
            start: Some(1000),
            end: Some(2000),
        };
        assert_eq!(ts.start, Some(1000));
        assert_eq!(ts.end, Some(2000));
    }

    #[test]
    fn timestamps_with_neither_field() {
        let ts = ActivityTimestamps {
            start: None,
            end: None,
        };
        assert!(ts.start.is_none());
        assert!(ts.end.is_none());
    }

    #[test]
    fn timestamps_serialize_and_deserialize() {
        let ts = ActivityTimestamps {
            start: Some(1000),
            end: Some(2000),
        };
        let json = serde_json::to_string(&ts).unwrap();
        let deserialized: ActivityTimestamps = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized, ts);
    }

    // -- Activity type properties ----------------------------------------------

    #[test]
    fn activity_is_send() {
        fn assert_send<T: Send>() {}
        assert_send::<Activity>();
    }

    #[test]
    fn activity_is_sync() {
        fn assert_sync<T: Sync>() {}
        assert_sync::<Activity>();
    }

    #[test]
    fn activity_timestamps_is_send() {
        fn assert_send<T: Send>() {}
        assert_send::<ActivityTimestamps>();
    }

    #[test]
    fn activity_timestamps_is_sync() {
        fn assert_sync<T: Sync>() {}
        assert_sync::<ActivityTimestamps>();
    }
}
