//! PresenceHub Core
//!
//! Coordinates the application runtime.
//!
//! # Responsibilities
//!
//! - Startup
//! - Shutdown
//! - Configuration loading
//! - Service initialization
//!
//! The Core intentionally contains no application-specific logic.
//!
//! # Modules
//!
//! | Module | Responsibility |
//! |---|---|
//! | [`core`] | Runtime lifecycle ([`Core`], [`CoreError`]) |
//! | [`config`] | Configuration loading ([`Config`], [`ConfigError`]) |
//! | [`context`] | Application context ([`ApplicationContext`]) |
//! | [`activity`] | Activity model ([`Activity`], [`ActivityTimestamps`]) |
//! | [`presence`] | Rich Presence model ([`RichPresence`]) |
//! | [`process`] | Process start-time lookup ([`process_start_unix`]) |

pub mod activity;
pub mod output;
pub mod presence;
pub mod process;

mod config;
mod context;
mod core;

pub use config::{
    Config, ConfigError, CustomAppConfig, OutputConfig, OwnershipPolicy, PluginConfig,
    PresenceConfig, RuntimeConfig, UnsupportedForegroundPolicy,
};
pub use context::ApplicationContext;
pub use core::Core;
pub use core::CoreError;
