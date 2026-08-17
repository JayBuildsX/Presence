//! Application context.
//!
//! This module defines the [`ApplicationContext`] type, which owns
//! runtime configuration and provides immutable access to it.

use crate::config::Config;

/// Lightweight execution context for the Core runtime.
///
/// This type is intentionally minimal. It currently exists solely to own
/// the application configuration so that the Core does not manage
/// configuration directly.
///
/// Future components should only be added to this type after they
/// demonstrate a real runtime ownership requirement — meaning the
/// component must be:
///
/// 1. Created by the runtime.
/// 2. Owned for the duration of the runtime.
/// 3. Accessible by multiple parts of the system.
///
/// The context provides immutable access to shared runtime configuration.
#[derive(Debug, Clone)]
pub struct ApplicationContext {
    config: Config,
}

impl ApplicationContext {
    /// Creates a new `ApplicationContext` with the given configuration.
    pub fn new(config: Config) -> Self {
        Self { config }
    }

    /// Returns a reference to the runtime configuration.
    pub fn config(&self) -> &Config {
        &self.config
    }
}

impl Default for ApplicationContext {
    fn default() -> Self {
        Self::new(Config::default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;

    #[test]
    fn application_context_default() {
        let ctx = ApplicationContext::default();
        assert_eq!(ctx.config().config_version, 1);
        assert_eq!(ctx.config().log_level, "info");
    }

    #[test]
    fn application_context_new() {
        let config = Config {
            config_version: 2,
            log_level: "debug".to_string(),
            ..Config::default()
        };
        let ctx = ApplicationContext::new(config);
        assert_eq!(ctx.config().config_version, 2);
        assert_eq!(ctx.config().log_level, "debug");
    }
}
