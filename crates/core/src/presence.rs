//! Rich Presence Model
//!
//! The generic, application-agnostic description of a user's current
//! presence. It sits between the canonical [`Activity`] produced by
//! plugins and the protocol-specific serialization performed by outputs:
//!
//! ```text
//! Plugin Activity
//!      │
//!      ▼
//! Rich Presence   (this module — domain model, no protocol details)
//!      │
//!      ▼
//! Discord serialization  (inside the discord-output crate)
//!      │
//!      ▼
//! IPC Frame
//! ```
//!
//! The model knows nothing about Discord (opcodes, frames, JSON schemas,
//! asset-key conventions). It describes *what* is happening; an output
//! decides *how* to render it. This keeps the plugin interface free of
//! Discord-specific types while letting any number of future applications
//! (FL Studio, VS Code, Blender, Spotify, …) express a rich presence
//! without requiring output changes.
//!
//! # From Activity
//!
//! [`RichPresence::from`] provides a generic conversion from the canonical
//! [`Activity`] that plugins emit today. The direct fields (`state`,
//! `details`, `timestamps`) map one-to-one. The application identity and
//! two conventional metadata keys are also recognised so existing plugins
//! keep working unchanged:
//!
//! * `activity.application` → [`PresenceAssets::large_text`] and a derived
//!   `large_image` slug (lowercase, spaces removed). This is the preferred
//!   field and takes precedence over the metadata key.
//! * `metadata["application"]` → the same, used only as a backward-compatible
//!   fallback when `activity.application` is `None`.
//! * `metadata["large_text"]` → [`PresenceAssets::large_text`], used only when
//!   neither `activity.application` nor `metadata["application"]` is present.
//!   Lets a plugin set a hover tooltip (e.g. "Python") without making it the
//!   application identity.
//! * `metadata["version"]` → [`PresenceAssets::small_text`].
//! * `metadata["small_text"]` → [`PresenceAssets::small_text`], taking
//!   precedence over the version fallback.
//! * `metadata["large_image"]` → [`PresenceAssets::large_image`], overriding
//!   the derived slug so a plugin can point at a real Discord asset key.
//! * `metadata["small_image"]` → [`PresenceAssets::small_image`].
//!
//! These are the only metadata keys the generic layer interprets. All
//! other metadata is ignored; richer plugins can populate the typed
//! [`RichPresence`] fields directly.

use crate::activity::Activity;

// ---------------------------------------------------------------------------
// Activity Type
// ---------------------------------------------------------------------------

/// The semantic category of an activity.
///
/// These are the standard presence categories shared by modern presence
/// systems (games, listening, watching, competing). The default is
/// [`Playing`](PresenceActivityType::Playing), which is what PresenceHub
/// has always displayed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PresenceActivityType {
    /// The user is playing something (games, DAWs, editors).
    #[default]
    Playing,
    /// The user is listening to audio.
    Listening,
    /// The user is watching video or a broadcast.
    Watching,
    /// The user is in a competitive/ranked session.
    Competing,
}

// ---------------------------------------------------------------------------
// Rich Presence
// ---------------------------------------------------------------------------

/// A generic rich presence description.
///
/// Every field is optional except nothing — a presence may show just a
/// state, just details, or a full rich presence with images and buttons.
/// Unpopulated fields are simply omitted when an output renders it.
///
/// `party`, `secrets`, and `buttons` are defined as part of the model so
/// the shape is stable, but they are reserved for future use and are not
/// yet rendered by the current Discord serialization.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct RichPresence {
    /// The semantic category of the activity.
    pub activity_type: PresenceActivityType,
    /// The primary status line (e.g. "Editing", "Idle").
    pub state: Option<String>,
    /// Secondary description providing more context.
    pub details: Option<String>,
    /// Optional temporal bounds for elapsed/remaining time.
    pub timestamps: Option<PresenceTimestamps>,
    /// Optional images and hover text shown alongside the presence.
    pub assets: Option<PresenceAssets>,
    /// Interactive buttons attached to the presence.
    pub buttons: Vec<PresenceButton>,
    /// Optional party/lobby information.
    pub party: Option<PresenceParty>,
    /// Optional matchmaking secrets.
    pub secrets: Option<PresenceSecrets>,
    /// Whether this presence represents a joinable instance.
    pub instance: bool,
}

/// Temporal bounds for a [`RichPresence`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PresenceTimestamps {
    /// Unix timestamp in seconds when the activity started.
    pub start: Option<i64>,
    /// Unix timestamp in seconds when the activity is expected to end.
    pub end: Option<i64>,
}

/// Images and hover text for a [`RichPresence`].
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PresenceAssets {
    /// Key or URL of the large image.
    pub large_image: Option<String>,
    /// Hover text for the large image.
    pub large_text: Option<String>,
    /// Key or URL of the small image.
    pub small_image: Option<String>,
    /// Hover text for the small image.
    pub small_text: Option<String>,
}

/// An interactive button on a [`RichPresence`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PresenceButton {
    /// The button label shown to the user.
    pub label: String,
    /// The URL opened when the button is clicked.
    pub url: String,
}

/// Party/lobby information for a [`RichPresence`].
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PresenceParty {
    /// Optional stable identifier for the party.
    pub id: Option<String>,
    /// Current size and maximum size of the party.
    pub size: Option<(u32, u32)>,
}

/// Matchmaking secrets for a [`RichPresence`].
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PresenceSecrets {
    /// Secret for joining the activity.
    pub join: Option<String>,
    /// Secret for spectating the activity.
    pub spectate: Option<String>,
    /// Secret for matching into the activity.
    pub r#match: Option<String>,
}

// ---------------------------------------------------------------------------
// Rich Presence Builder
// ---------------------------------------------------------------------------

impl RichPresence {
    /// Begin building a [`RichPresence`].
    ///
    /// This is the preferred construction mechanism going forward. It
    /// provides a fluent, strongly-typed API where plugins only set the
    /// fields they care about; everything else defaults sensibly.
    pub fn builder() -> RichPresenceBuilder {
        RichPresenceBuilder::default()
    }
}

/// A fluent, strongly-typed constructor for [`RichPresence`].
///
/// The builder owns all intermediate state and produces an immutable
/// [`RichPresence`] via [`build`](RichPresenceBuilder::build), so a
/// partially-initialized value can never escape as a live presence.
///
/// Setters consume their argument by value where possible (no needless
/// cloning); string-typed fields accept anything [`Into<String>`].
///
/// # Example
///
/// ```rust
/// use presencehub_core::presence::{PresenceActivityType, RichPresence};
///
/// let presence = RichPresence::builder()
///     .state("Editing")
///     .details("Project: song.flp")
///     .activity_type(PresenceActivityType::Playing)
///     .build();
///
/// assert_eq!(presence.state.as_deref(), Some("Editing"));
/// ```
#[derive(Debug, Clone, Default)]
pub struct RichPresenceBuilder {
    activity_type: PresenceActivityType,
    state: Option<String>,
    details: Option<String>,
    timestamps: Option<PresenceTimestamps>,
    assets: Option<PresenceAssets>,
    buttons: Vec<PresenceButton>,
    party: Option<PresenceParty>,
    secrets: Option<PresenceSecrets>,
    instance: bool,
}

impl RichPresenceBuilder {
    /// Create a builder with every field at its default.
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the semantic activity category. Defaults to [`Playing`].
    pub fn activity_type(mut self, activity_type: PresenceActivityType) -> Self {
        self.activity_type = activity_type;
        self
    }

    /// Set the primary status line.
    pub fn state(mut self, state: impl Into<String>) -> Self {
        self.state = Some(state.into());
        self
    }

    /// Set the secondary description.
    pub fn details(mut self, details: impl Into<String>) -> Self {
        self.details = Some(details.into());
        self
    }

    /// Set the temporal bounds.
    pub fn timestamps(mut self, timestamps: PresenceTimestamps) -> Self {
        self.timestamps = Some(timestamps);
        self
    }

    /// Set the images and hover text.
    pub fn assets(mut self, assets: PresenceAssets) -> Self {
        self.assets = Some(assets);
        self
    }

    /// Set the interactive buttons attached to the presence.
    pub fn buttons(mut self, buttons: Vec<PresenceButton>) -> Self {
        self.buttons = buttons;
        self
    }

    /// Append a single interactive button to the presence.
    ///
    /// Discord renders at most two buttons; any excess is dropped when the
    /// presence is serialized.
    pub fn button(mut self, button: PresenceButton) -> Self {
        self.buttons.push(button);
        self
    }

    /// Mark the presence as a joinable instance.
    pub fn instance(mut self, instance: bool) -> Self {
        self.instance = instance;
        self
    }

    /// Set the party/lobby information.
    pub fn party(mut self, party: PresenceParty) -> Self {
        self.party = Some(party);
        self
    }

    /// Set the matchmaking secrets.
    pub fn secrets(mut self, secrets: PresenceSecrets) -> Self {
        self.secrets = Some(secrets);
        self
    }

    /// Consume the builder and produce the final [`RichPresence`].
    ///
    /// Fields that were never set remain at their defaults.
    pub fn build(self) -> RichPresence {
        RichPresence {
            activity_type: self.activity_type,
            state: self.state,
            details: self.details,
            timestamps: self.timestamps,
            assets: self.assets,
            buttons: self.buttons,
            party: self.party,
            secrets: self.secrets,
            instance: self.instance,
        }
    }
}

// ---------------------------------------------------------------------------
// From Activity
// ---------------------------------------------------------------------------

/// Derive a lowercased, space-stripped slug from an application name.
///
/// Used as a placeholder asset key until real keys are registered, e.g.
/// `"FL Studio"` → `"flstudio"`.
fn application_slug(application: &str) -> String {
    application.to_lowercase().replace(' ', "")
}

impl From<&Activity> for RichPresence {
    fn from(activity: &Activity) -> Self {
        // The explicit application field is the first-class identity; the
        // metadata key remains as a backward-compatible fallback so plugins
        // that predate the field keep rendering identically. The `large_text`
        // metadata key is a final fallback so a plugin can set a hover tooltip
        // (e.g. a file type name) without claiming the application identity.
        let large_text = activity
            .application
            .clone()
            .or_else(|| activity.metadata.get("application").cloned())
            .or_else(|| activity.metadata.get("large_text").cloned());
        let small_text = activity
            .metadata
            .get("small_text")
            .cloned()
            .or_else(|| activity.metadata.get("version").cloned());

        // Explicit asset keys (metadata["large_image"] / ["small_image"])
        // override the derived application slug. Plugins that register real
        // assets in the Discord application set these so the icon resolves
        // instead of Discord showing a placeholder `?` for an unregistered
        // derived key.
        let large_image =
            if activity.metadata.get("no_large_image").map(|s| s.as_str()) == Some("true") {
                None
            } else {
                activity
                    .metadata
                    .get("large_image")
                    .cloned()
                    .or_else(|| large_text.as_deref().map(application_slug))
            };
        let small_image = activity.metadata.get("small_image").cloned();

        let mut builder = RichPresence::builder().state(activity.state.clone());
        if let Some(details) = activity.details.as_deref() {
            builder = builder.details(details);
        }
        if let Some(ts) = activity.timestamps.as_ref() {
            builder = builder.timestamps(PresenceTimestamps {
                start: ts.start,
                end: ts.end,
            });
        }
        if large_text.is_some()
            || small_text.is_some()
            || large_image.is_some()
            || small_image.is_some()
        {
            builder = builder.assets(PresenceAssets {
                large_image,
                large_text,
                small_image,
                small_text,
            });
        }

        builder.build()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::activity::ActivityTimestamps;
    use std::collections::HashMap;

    // -- Construction --------------------------------------------------------

    #[test]
    fn rich_presence_default() {
        let presence = RichPresence::default();
        assert_eq!(presence.activity_type, PresenceActivityType::Playing);
        assert!(presence.state.is_none());
        assert!(presence.details.is_none());
        assert!(presence.timestamps.is_none());
        assert!(presence.assets.is_none());
        assert!(presence.buttons.is_empty());
        assert!(presence.party.is_none());
        assert!(presence.secrets.is_none());
        assert!(!presence.instance);
    }

    #[test]
    fn rich_presence_with_all_fields() {
        let presence = RichPresence {
            activity_type: PresenceActivityType::Competing,
            state: Some("In game".to_string()),
            details: Some("Ranked match".to_string()),
            timestamps: Some(PresenceTimestamps {
                start: Some(1000),
                end: Some(2000),
            }),
            assets: Some(PresenceAssets {
                large_image: Some("game".to_string()),
                large_text: Some("My Game".to_string()),
                small_image: Some("badge".to_string()),
                small_text: Some("Rank 5".to_string()),
            }),
            buttons: vec![PresenceButton {
                label: "Join".to_string(),
                url: "https://example.com".to_string(),
            }],
            party: Some(PresenceParty {
                id: Some("lobby-1".to_string()),
                size: Some((2, 8)),
            }),
            secrets: Some(PresenceSecrets {
                join: Some("j".to_string()),
                spectate: None,
                r#match: None,
            }),
            instance: true,
        };

        assert_eq!(presence.activity_type, PresenceActivityType::Competing);
        assert_eq!(presence.state.as_deref(), Some("In game"));
        assert_eq!(presence.buttons.len(), 1);
        assert_eq!(presence.party.unwrap().size, Some((2, 8)));
        assert_eq!(presence.secrets.unwrap().join.as_deref(), Some("j"));
        assert!(presence.instance);
    }

    #[test]
    fn presence_activity_type_default_is_playing() {
        assert_eq!(
            PresenceActivityType::default(),
            PresenceActivityType::Playing
        );
    }

    // -- Builder ---------------------------------------------------------------

    #[test]
    fn builder_default_equals_default_presence() {
        assert_eq!(RichPresence::builder().build(), RichPresence::default());
    }

    #[test]
    fn builder_new_equals_builder_default() {
        assert_eq!(
            RichPresence::builder().build(),
            RichPresenceBuilder::new().build()
        );
    }

    #[test]
    fn builder_chained_setters_populate_all_fields() {
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
            .buttons(vec![PresenceButton {
                label: "Join".to_string(),
                url: "https://example.com".to_string(),
            }])
            .party(PresenceParty {
                id: Some("lobby-1".to_string()),
                size: Some((2, 8)),
            })
            .secrets(PresenceSecrets {
                join: Some("j".to_string()),
                spectate: None,
                r#match: Some("m".to_string()),
            })
            .instance(true)
            .build();

        assert_eq!(presence.activity_type, PresenceActivityType::Competing);
        assert_eq!(presence.state.as_deref(), Some("In game"));
        assert_eq!(presence.details.as_deref(), Some("Ranked match"));
        assert_eq!(presence.timestamps.unwrap().start, Some(1000));
        let assets = presence.assets.unwrap();
        assert_eq!(assets.large_image.as_deref(), Some("game"));
        assert_eq!(assets.small_text.as_deref(), Some("Rank 5"));
        assert_eq!(presence.buttons.len(), 1);
        assert_eq!(presence.buttons[0].label, "Join");
        assert_eq!(presence.buttons[0].url, "https://example.com");
        assert_eq!(presence.party.unwrap().size, Some((2, 8)));
        assert_eq!(presence.secrets.unwrap().r#match.as_deref(), Some("m"));
        assert!(presence.instance);
    }

    #[test]
    fn builder_fields_omitted_stay_none() {
        let presence = RichPresence::builder().state("Only state").build();

        assert_eq!(presence.state.as_deref(), Some("Only state"));
        assert!(presence.details.is_none());
        assert!(presence.timestamps.is_none());
        assert!(presence.assets.is_none());
        assert!(presence.party.is_none());
        assert!(presence.secrets.is_none());
        assert!(presence.buttons.is_empty());
        assert!(!presence.instance);
    }

    #[test]
    fn builder_accepts_into_string_for_state_and_details() {
        let presence = RichPresence::builder()
            .state(String::from("From String"))
            .details("From &str")
            .build();

        assert_eq!(presence.state.as_deref(), Some("From String"));
        assert_eq!(presence.details.as_deref(), Some("From &str"));
    }

    #[test]
    fn builder_button_appends_singular_buttons() {
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

        assert_eq!(presence.buttons.len(), 2);
        assert_eq!(presence.buttons[0].label, "Join");
        assert_eq!(presence.buttons[1].url, "https://example.com/spectate");
    }

    #[test]
    fn builder_button_and_buttons_are_equivalent() {
        let chained = RichPresence::builder()
            .button(PresenceButton {
                label: "A".to_string(),
                url: "https://a.example".to_string(),
            })
            .button(PresenceButton {
                label: "B".to_string(),
                url: "https://b.example".to_string(),
            })
            .build();

        let whole = RichPresence::builder()
            .buttons(vec![
                PresenceButton {
                    label: "A".to_string(),
                    url: "https://a.example".to_string(),
                },
                PresenceButton {
                    label: "B".to_string(),
                    url: "https://b.example".to_string(),
                },
            ])
            .build();

        assert_eq!(chained, whole);
    }

    #[test]
    fn builder_activity_type_defaults_to_playing() {
        let presence = RichPresence::builder().build();
        assert_eq!(presence.activity_type, PresenceActivityType::Playing);
    }

    #[test]
    fn builder_output_is_immutable_after_build() {
        let presence = RichPresence::builder()
            .state("Initial")
            .details("A")
            .build();
        let original = presence.clone();

        // Eager refs must be local to the builder; the produced value is fixed.
        let _fresh = RichPresence::builder().state("Changed").build();

        assert_eq!(presence, original);
        assert_eq!(presence.state.as_deref(), Some("Initial"));
    }

    #[test]
    fn builder_reusing_a_cloned_builder_produces_independent_values() {
        let builder = RichPresence::builder().state("Shared");
        let first = builder.clone().build();
        let second = builder.build();

        assert_eq!(first, second);
        assert_eq!(first.state.as_deref(), Some("Shared"));
        assert_eq!(second.state.as_deref(), Some("Shared"));
    }

    #[test]
    fn builder_equivalence_with_struct_literal() {
        let from_builder = RichPresence::builder()
            .state("Editing")
            .details("Producing")
            .activity_type(PresenceActivityType::Playing)
            .build();

        let from_literal = RichPresence {
            activity_type: PresenceActivityType::Playing,
            state: Some("Editing".to_string()),
            details: Some("Producing".to_string()),
            timestamps: None,
            assets: None,
            buttons: Vec::new(),
            party: None,
            secrets: None,
            instance: false,
        };

        assert_eq!(from_builder, from_literal);
    }

    #[test]
    fn builder_produces_send_sync_value() {
        fn assert_send<T: Send>() {}
        fn assert_sync<T: Sync>() {}
        assert_send::<RichPresenceBuilder>();
        assert_sync::<RichPresenceBuilder>();
        assert_send::<RichPresence>();
        assert_sync::<RichPresence>();
    }

    // -- Conversion from Activity ---------------------------------------------

    #[test]
    fn conversion_maps_state_details_and_timestamps() {
        let activity = Activity {
            state: "Editing".to_string(),
            details: Some("Project: song.flp".to_string()),
            timestamps: Some(ActivityTimestamps {
                start: Some(1000),
                end: Some(2000),
            }),
            metadata: HashMap::new(),
            application: None,
        };

        let presence = RichPresence::from(&activity);

        assert_eq!(presence.activity_type, PresenceActivityType::Playing);
        assert_eq!(presence.state.as_deref(), Some("Editing"));
        assert_eq!(presence.details.as_deref(), Some("Project: song.flp"));
        assert_eq!(presence.timestamps.unwrap().start, Some(1000));
        assert_eq!(presence.timestamps.unwrap().end, Some(2000));
    }

    #[test]
    fn conversion_explicit_large_image_overrides_derived_slug() {
        let mut metadata = HashMap::new();
        metadata.insert("application".to_string(), "Antigravity".to_string());
        metadata.insert("large_image".to_string(), "custom_antigravity".to_string());
        metadata.insert("small_image".to_string(), "robot".to_string());

        let activity = Activity {
            state: "Coding".to_string(),
            details: None,
            timestamps: None,
            metadata,
            application: None,
        };

        let presence = RichPresence::from(&activity);
        let assets = presence.assets.expect("assets should be present");

        assert_eq!(
            assets.large_text.as_deref(),
            Some("Antigravity"),
            "large text still comes from application"
        );
        assert_eq!(
            assets.large_image.as_deref(),
            Some("custom_antigravity"),
            "explicit large_image must override the derived 'antigravity' slug"
        );
        assert_eq!(assets.small_image.as_deref(), Some("robot"));
        assert!(assets.small_text.is_none());
    }

    #[test]
    fn conversion_still_derives_slug_when_no_explicit_key() {
        let mut metadata = HashMap::new();
        metadata.insert("application".to_string(), "FL Studio".to_string());

        let activity = Activity {
            state: "Editing".to_string(),
            details: None,
            timestamps: None,
            metadata,
            application: None,
        };

        let presence = RichPresence::from(&activity);
        let assets = presence.assets.expect("assets should be derived");
        assert_eq!(assets.large_image.as_deref(), Some("flstudio"));
    }

    #[test]
    fn application_takes_precedence_over_metadata() {
        // The explicit `application` field is the first-class identity and
        // must win over a conflicting legacy metadata key.
        let mut metadata = HashMap::new();
        metadata.insert("application".to_string(), "FL Studio".to_string());

        let activity = Activity {
            state: "In Game".to_string(),
            details: None,
            timestamps: None,
            application: Some("Antigravity".to_string()),
            metadata,
        };

        let presence = RichPresence::from(&activity);
        let assets = presence.assets.expect("assets should be present");
        assert_eq!(assets.large_text.as_deref(), Some("Antigravity"));
        assert_eq!(
            assets.large_image.as_deref(),
            Some("antigravity"),
            "the derived slug follows the field, not the metadata"
        );
    }

    #[test]
    fn application_falls_back_to_metadata() {
        // A plugin that predates the field keeps working unchanged: the
        // metadata key is used when `application` is None.
        let mut metadata = HashMap::new();
        metadata.insert("application".to_string(), "FL Studio".to_string());

        let activity = Activity {
            state: "Editing".to_string(),
            details: None,
            timestamps: None,
            application: None,
            metadata,
        };

        let presence = RichPresence::from(&activity);
        let assets = presence.assets.expect("assets should be derived");
        assert_eq!(assets.large_text.as_deref(), Some("FL Studio"));
        assert_eq!(assets.large_image.as_deref(), Some("flstudio"));
    }

    #[test]
    fn no_presencehub_application_fallback() {
        // Regression: the generic layer must never fall back to a generic
        // "PresenceHub" application identity when a plugin does not supply
        // one. No application identity means no assets at all.
        let activity = Activity {
            state: "Idle".to_string(),
            details: None,
            timestamps: None,
            application: None,
            metadata: HashMap::new(),
        };

        let presence = RichPresence::from(&activity);
        assert!(presence.assets.is_none(), "no application -> no assets");
        assert_eq!(presence.state.as_deref(), Some("Idle"));
    }

    #[test]
    fn conversion_uses_large_text_metadata_key_without_application() {
        // A plugin (e.g. OpenCode) that wants the hover tooltip to be the
        // file type name rather than the application name sets
        // metadata["large_text"] and leaves `application` unset.
        let mut metadata = HashMap::new();
        metadata.insert("large_image".to_string(), "rust".to_string());
        metadata.insert("large_text".to_string(), "Rust".to_string());
        metadata.insert("small_image".to_string(), "opencode".to_string());
        metadata.insert("small_text".to_string(), "OpenCode".to_string());

        let activity = Activity {
            state: "Editing main.rs".to_string(),
            details: Some("Project: PresenceHUB".to_string()),
            timestamps: None,
            application: None,
            metadata,
        };

        let presence = RichPresence::from(&activity);
        let assets = presence.assets.expect("assets should be present");

        assert_eq!(assets.large_text.as_deref(), Some("Rust"));
        assert_eq!(assets.large_image.as_deref(), Some("rust"));
        assert_eq!(assets.small_image.as_deref(), Some("opencode"));
        assert_eq!(assets.small_text.as_deref(), Some("OpenCode"));
    }

    #[test]
    fn conversion_application_takes_precedence_over_large_text_key() {
        // If a plugin sets both an application identity and a `large_text`
        // metadata key, the application identity must win.
        let mut metadata = HashMap::new();
        metadata.insert("large_text".to_string(), "Rust".to_string());

        let activity = Activity {
            state: "Editing".to_string(),
            details: None,
            timestamps: None,
            application: Some("OpenCode".to_string()),
            metadata,
        };

        let presence = RichPresence::from(&activity);
        let assets = presence.assets.expect("assets should be present");
        assert_eq!(assets.large_text.as_deref(), Some("OpenCode"));
    }

    #[test]
    fn conversion_small_text_key_takes_precedence_over_version() {
        let mut metadata = HashMap::new();
        metadata.insert("version".to_string(), "21".to_string());
        metadata.insert("small_text".to_string(), "OpenCode".to_string());

        let activity = Activity {
            state: "Editing".to_string(),
            details: None,
            timestamps: None,
            application: None,
            metadata,
        };

        let presence = RichPresence::from(&activity);
        let assets = presence.assets.expect("assets should be present");
        assert_eq!(assets.small_text.as_deref(), Some("OpenCode"));
    }

    #[test]
    fn conversion_derives_assets_from_metadata() {
        let mut metadata = HashMap::new();
        metadata.insert("application".to_string(), "FL Studio".to_string());
        metadata.insert("version".to_string(), "21".to_string());

        let activity = Activity {
            state: "Editing".to_string(),
            details: None,
            timestamps: None,
            metadata,
            application: None,
        };

        let presence = RichPresence::from(&activity);
        let assets = presence.assets.expect("assets should be derived");

        assert_eq!(assets.large_text.as_deref(), Some("FL Studio"));
        assert_eq!(assets.large_image.as_deref(), Some("flstudio"));
        assert_eq!(assets.small_text.as_deref(), Some("21"));
        assert!(assets.small_image.is_none());
    }

    #[test]
    fn conversion_without_metadata_has_no_assets() {
        let activity = Activity {
            state: "Idle".to_string(),
            details: None,
            timestamps: None,
            metadata: HashMap::new(),
            application: None,
        };

        let presence = RichPresence::from(&activity);

        assert!(presence.assets.is_none());
        assert!(presence.timestamps.is_none());
        assert_eq!(presence.state.as_deref(), Some("Idle"));
    }

    #[test]
    fn conversion_ignores_unknown_metadata_keys() {
        let mut metadata = HashMap::new();
        metadata.insert("bpm".to_string(), "128".to_string());

        let activity = Activity {
            state: "Producing".to_string(),
            details: None,
            timestamps: None,
            metadata,
            application: None,
        };

        let presence = RichPresence::from(&activity);

        // Unknown keys must not surface as assets or any typed field.
        assert!(presence.assets.is_none());
        assert!(presence.buttons.is_empty());
        assert!(presence.party.is_none());
        assert!(presence.secrets.is_none());
    }

    #[test]
    fn conversion_uses_builder_and_matches_direct_construction() {
        let mut metadata = HashMap::new();
        metadata.insert("application".to_string(), "FL Studio".to_string());

        let activity = Activity {
            state: "Editing".to_string(),
            details: Some("Project: song.flp".to_string()),
            timestamps: Some(ActivityTimestamps {
                start: Some(1000),
                end: None,
            }),
            metadata,
            application: None,
        };

        let from_activity = RichPresence::from(&activity);

        let manual = RichPresence::builder()
            .state(activity.state.clone())
            .details("Project: song.flp")
            .timestamps(PresenceTimestamps {
                start: Some(1000),
                end: None,
            })
            .assets(PresenceAssets {
                large_image: Some("flstudio".to_string()),
                large_text: Some("FL Studio".to_string()),
                small_image: None,
                small_text: None,
            })
            .build();

        assert_eq!(from_activity, manual);
    }

    #[test]
    fn conversion_defaults_activity_type_and_instance() {
        let activity = Activity {
            state: "Editing".to_string(),
            details: None,
            timestamps: None,
            metadata: HashMap::new(),
            application: None,
        };

        let presence = RichPresence::from(&activity);

        assert_eq!(presence.activity_type, PresenceActivityType::Playing);
        assert!(!presence.instance);
    }

    // -- Type properties ------------------------------------------------------

    #[test]
    fn rich_presence_is_send() {
        fn assert_send<T: Send>() {}
        assert_send::<RichPresence>();
    }

    #[test]
    fn rich_presence_is_sync() {
        fn assert_sync<T: Sync>() {}
        assert_sync::<RichPresence>();
    }
}
