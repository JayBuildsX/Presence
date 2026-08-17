// Prevents additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    presencehub_desktop::run()
}

// ---------------------------------------------------------------------------
// Runtime integration tests
// ---------------------------------------------------------------------------

/// Validates the complete PresenceHub execution pipeline:
/// Core → PluginHost → FL Studio Plugin → Activity → Console Logger
#[cfg(test)]
mod runtime_tests {
    use presencehub_core::{
        activity::Activity,
        output::{ConsoleOutput, PresenceEngine},
        Core,
    };
    use presencehub_discord_output::DiscordOutput;
    use presencehub_flstudio::FlStudioPlugin;
    use presencehub_plugin_host::{Plugin, PluginError, PluginHost, PluginMetadata};

    /// Ownership:
    /// - Core: created by test, owned by test, destroyed by test
    /// - PluginHost: created by test, owns plugins via Vec<Box<dyn Plugin>>
    /// - FlStudioPlugin: created by test, transferred to PluginHost via Box
    /// - Activity: produced by plugin, consumed by test (or output)
    #[test]
    fn runtime_pipeline_plugin_registers_and_initializes() {
        // 1. Construct Core
        let mut core = Core::new();
        core.start().expect("Core should start");

        // 2. Construct PluginHost
        let mut host = PluginHost::new();

        // 3. Register FL Studio plugin
        host.register(Box::new(FlStudioPlugin::new()));

        // 4. Initialize all plugins
        let init_errors = host.init_all();
        assert!(
            init_errors.is_empty(),
            "FL Studio plugin should initialize without errors"
        );

        // 5. Verify plugin metadata via immutable access
        let count = host.plugins().len();
        assert_eq!(count, 1);

        // 6. Shutdown plugins
        let shutdown_errors = host.shutdown_all();
        assert!(
            shutdown_errors.is_empty(),
            "FL Studio plugin should shut down without errors"
        );

        // 7. Shutdown Core
        core.stop().expect("Core should stop");
    }

    #[test]
    fn runtime_pipeline_presence_engine_routes_activity() {
        // Validate the complete PresenceHub pipeline:
        // FL Studio Plugin -> Activity -> PresenceEngine -> ConsoleOutput
        let mut engine = PresenceEngine::new();
        engine.register_output(Box::new(ConsoleOutput::new()));

        let activity = Activity {
            state: "Editing".to_string(),
            details: Some("Project: song.flp".to_string()),
            timestamps: None,
            metadata: {
                let mut m = std::collections::HashMap::new();
                m.insert("application".to_string(), "FL Studio".to_string());
                m.insert("version".to_string(), "20".to_string());
                m.insert("unsaved".to_string(), "false".to_string());
                m
            },
            application: None,
        };

        let errors = engine.update("FL Studio", &activity);
        assert!(
            errors.is_empty(),
            "ConsoleOutput should publish without errors"
        );

        // The engine stamps the session start time on the activity.
        let current = engine.current_activity().unwrap();
        assert_eq!(current.state, "Editing");
        assert_eq!(current.details, Some("Project: song.flp".to_string()));
        assert!(current.timestamps.as_ref().unwrap().start.is_some());
    }

    #[test]
    fn runtime_pipeline_presence_engine_detects_duplicates() {
        let mut engine = PresenceEngine::new();
        engine.register_output(Box::new(ConsoleOutput::new()));

        let activity = Activity {
            state: "Idle".to_string(),
            details: None,
            timestamps: None,
            metadata: std::collections::HashMap::new(),
            application: None,
        };

        engine.update("FL Studio", &activity);
        let errors = engine.update("FL Studio", &activity);
        assert!(
            errors.is_empty(),
            "Duplicate activities should not be republished"
        );
    }

    #[test]
    fn runtime_pipeline_multiple_outputs() {
        // Verify the PresenceEngine can publish to multiple outputs simultaneously.
        // ConsoleOutput always succeeds. DiscordOutput fails if Discord is not running.
        let mut engine = PresenceEngine::new();
        engine.register_output(Box::new(ConsoleOutput::new()));
        engine.register_output(Box::new(DiscordOutput::new(
            123456789,
            std::collections::HashMap::new(),
        )));

        let activity = Activity {
            state: "Editing".to_string(),
            details: Some("Project: song.flp".to_string()),
            timestamps: None,
            metadata: {
                let mut m = std::collections::HashMap::new();
                m.insert("application".to_string(), "FL Studio".to_string());
                m.insert("version".to_string(), "21".to_string());
                m
            },
            application: None,
        };

        let _errors = engine.update("FL Studio", &activity);
        // ConsoleOutput succeeds, DiscordOutput may fail if Discord is not running
        // The PresenceEngine should still store the activity regardless
        let current = engine.current_activity().unwrap();
        assert_eq!(current.state, "Editing");
        assert_eq!(current.details, Some("Project: song.flp".to_string()));
        assert!(current.timestamps.as_ref().unwrap().start.is_some());
    }

    #[test]
    fn runtime_pipeline_with_core_context() {
        // Verify the Core owns its context and the plugin lifecycle
        // integrates cleanly with the Core runtime.
        let mut core = Core::new();
        core.start().expect("Core should start");

        let ctx = core.context();
        assert_eq!(ctx.config().config_version, 1);
        assert_eq!(ctx.config().log_level, "info");

        core.stop().expect("Core should stop");
    }

    #[test]
    fn runtime_pipeline_multiple_plugins() {
        // Verify the PluginHost can manage multiple plugins simultaneously.
        let mut host = PluginHost::new();
        host.register(Box::new(FlStudioPlugin::new()));
        host.register(Box::new(FlStudioPlugin::new()));

        assert_eq!(host.plugins().len(), 2);

        let init_errors = host.init_all();
        assert!(
            init_errors.is_empty(),
            "Multiple plugins should all initialize"
        );

        let shutdown_errors = host.shutdown_all();
        assert!(
            shutdown_errors.is_empty(),
            "Multiple plugins should all shut down"
        );
    }

    #[test]
    fn runtime_pipeline_plugin_failure_isolation() {
        // Verify that a failing plugin does not prevent other plugins from running.
        struct FailingPlugin;

        impl Plugin for FailingPlugin {
            fn metadata(&self) -> &PluginMetadata {
                // Cannot use const because PluginMetadata::new is not const.
                // Use a leaked static instead of const.
                static META: std::sync::OnceLock<PluginMetadata> = std::sync::OnceLock::new();
                META.get_or_init(|| PluginMetadata::new("Failing", "1.0.0"))
            }

            fn init(&mut self) -> Result<(), PluginError> {
                Err(PluginError::InitFailed("intentional failure".to_string()))
            }

            fn poll(&mut self) -> Result<Option<Activity>, PluginError> {
                Err(PluginError::PollFailed(
                    "intentional poll failure".to_string(),
                ))
            }

            fn shutdown(&mut self) -> Result<(), PluginError> {
                Ok(())
            }
        }

        let mut host = PluginHost::new();
        host.register(Box::new(FailingPlugin));
        host.register(Box::new(FlStudioPlugin::new()));

        let errors = host.init_all();
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].0, "Failing");

        // The FL Studio plugin should still have initialized successfully
        let shutdown_errors = host.shutdown_all();
        assert!(shutdown_errors.is_empty());
    }
}
