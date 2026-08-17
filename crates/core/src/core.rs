//! Core runtime lifecycle.
//!
//! This module defines the [`Core`] runtime type, its lifecycle
//! states, and associated error types.

use crate::config::Config;
use crate::context::ApplicationContext;
use tracing::{info, warn};

/// Errors that can occur during Core lifecycle operations.
#[derive(Debug, thiserror::Error)]
pub enum CoreError {
    /// Returned when [`Core::start`] is called but the Core is already running.
    #[error("Core is already running")]
    AlreadyRunning,

    /// Returned when [`Core::stop`] is called but the Core is not currently running.
    #[error("Core is not running")]
    NotRunning,

    /// Returned when [`Core::start`] is called but the Core has already been
    /// stopped and cannot be restarted.
    #[error("Core has already been stopped")]
    AlreadyStopped,
}

/// Tracks the current lifecycle phase of the Core runtime.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CoreState {
    /// The Core has been constructed but not yet started.
    Uninitialized,
    /// The Core is running and accepting lifecycle operations.
    Running,
    /// The Core has been stopped and cannot be restarted.
    Stopped,
}

/// The PresenceHub Core runtime.
///
/// Responsible for coordinating the application's lifecycle,
/// including startup, shutdown, and owning runtime configuration.
///
/// # Lifecycle
///
/// ```text
/// new() ──► Uninitialized ──► start() ──► Running ──► stop() ──► Stopped
/// ```
///
/// # Example
///
/// ```rust
/// use presencehub_core::Core;
///
/// let mut core = Core::new();
/// core.start().expect("core should start successfully");
/// core.stop().expect("core should stop successfully");
/// ```
pub struct Core {
    /// The current lifecycle state.
    state: CoreState,

    /// The application context, owned by the Core.
    context: ApplicationContext,
}

impl Core {
    /// Creates a new Core runtime in the `Uninitialized` state with default
    /// configuration.
    ///
    /// The Core must be started with [`Core::start`] before it can perform
    /// any runtime operations.
    pub fn new() -> Self {
        Self {
            state: CoreState::Uninitialized,
            context: ApplicationContext::default(),
        }
    }

    /// Creates a new Core runtime with the given configuration.
    ///
    /// This allows callers to provide custom configuration before the
    /// runtime is started.
    pub fn with_config(config: Config) -> Self {
        Self {
            state: CoreState::Uninitialized,
            context: ApplicationContext::new(config),
        }
    }

    /// Returns a reference to the application context.
    ///
    /// The context provides immutable access to runtime configuration
    /// and other runtime-owned components.
    pub fn context(&self) -> &ApplicationContext {
        &self.context
    }

    /// Starts the Core runtime.
    ///
    /// Transitions the Core from `Uninitialized` to `Running`.
    ///
    /// # Errors
    ///
    /// Returns [`CoreError::AlreadyRunning`] if the Core is already running.
    /// Returns [`CoreError::AlreadyStopped`] if the Core has already been stopped.
    pub fn start(&mut self) -> Result<(), CoreError> {
        match self.state {
            CoreState::Uninitialized => {
                self.state = CoreState::Running;
                info!(
                    config_version = self.context.config().config_version,
                    log_level = %self.context.config().log_level,
                    "Core runtime started",
                );
                Ok(())
            }
            CoreState::Running => {
                warn!("Attempted to start Core, but it is already running");
                Err(CoreError::AlreadyRunning)
            }
            CoreState::Stopped => {
                warn!("Attempted to start Core, but it has already been stopped");
                Err(CoreError::AlreadyStopped)
            }
        }
    }

    /// Stops the Core runtime.
    ///
    /// Transitions the Core from `Running` to `Stopped`.
    ///
    /// # Errors
    ///
    /// Returns [`CoreError::NotRunning`] if the Core is not currently running.
    pub fn stop(&mut self) -> Result<(), CoreError> {
        match self.state {
            CoreState::Running => {
                self.state = CoreState::Stopped;
                info!("Core runtime stopped");
                Ok(())
            }
            CoreState::Uninitialized | CoreState::Stopped => {
                warn!("Attempted to stop Core, but it is not running");
                Err(CoreError::NotRunning)
            }
        }
    }
}

impl Default for Core {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // -- Core lifecycle tests --------------------------------------------------

    #[test]
    fn new_creates_uninitialized_core() {
        let core = Core::new();
        assert_eq!(core.state, CoreState::Uninitialized);
    }

    #[test]
    fn start_transitions_to_running() {
        let mut core = Core::new();
        assert!(core.start().is_ok());
        assert_eq!(core.state, CoreState::Running);
    }

    #[test]
    fn stop_transitions_to_stopped() {
        let mut core = Core::new();
        core.start().unwrap();
        assert!(core.stop().is_ok());
        assert_eq!(core.state, CoreState::Stopped);
    }

    #[test]
    fn full_lifecycle() {
        let mut core = Core::new();
        assert!(core.start().is_ok());
        assert!(core.stop().is_ok());
    }

    #[test]
    fn start_when_already_running_returns_error() {
        let mut core = Core::new();
        core.start().unwrap();
        let result = core.start();
        assert!(result.is_err());
        match result {
            Err(CoreError::AlreadyRunning) => {}
            _ => panic!("expected AlreadyRunning error"),
        }
    }

    #[test]
    fn start_when_already_stopped_returns_error() {
        let mut core = Core::new();
        core.start().unwrap();
        core.stop().unwrap();
        let result = core.start();
        assert!(result.is_err());
        match result {
            Err(CoreError::AlreadyStopped) => {}
            _ => panic!("expected AlreadyStopped error"),
        }
    }

    #[test]
    fn stop_when_not_running_returns_error() {
        let mut core = Core::new();
        let result = core.stop();
        assert!(result.is_err());
        match result {
            Err(CoreError::NotRunning) => {}
            _ => panic!("expected NotRunning error"),
        }
    }

    #[test]
    fn stop_when_already_stopped_returns_error() {
        let mut core = Core::new();
        core.start().unwrap();
        core.stop().unwrap();
        let result = core.stop();
        assert!(result.is_err());
        match result {
            Err(CoreError::NotRunning) => {}
            _ => panic!("expected NotRunning error"),
        }
    }

    #[test]
    fn default_creates_uninitialized_core() {
        let core: Core = Default::default();
        assert_eq!(core.state, CoreState::Uninitialized);
    }

    // -- Context integration tests ---------------------------------------------

    #[test]
    fn new_core_uses_default_context() {
        let core = Core::new();
        assert_eq!(core.context().config().config_version, 1);
        assert_eq!(core.context().config().log_level, "info");
    }

    #[test]
    fn with_config_uses_provided_config() {
        let config = Config {
            config_version: 2,
            log_level: "debug".to_string(),
            ..Config::default()
        };
        let mut core = Core::with_config(config.clone());
        assert_eq!(core.context().config().config_version, 2);
        assert_eq!(core.context().config().log_level, "debug");

        // Ensure context is still accessible after start/stop
        core.start().unwrap();
        core.stop().unwrap();
        assert_eq!(core.context().config().config_version, 2);
    }

    #[test]
    fn context_returns_immutable_ref() {
        let core = Core::new();
        let ctx: &ApplicationContext = core.context();
        assert_eq!(ctx.config().log_level, "info");
    }
}
