//! Live observation harness for the League plugin.
//!
//! Runs the plugin for a fixed number of polls and prints every emitted
//! activity. Useful for manually verifying the plugin against a running
//! League Client.
//!
//! ```text
//! cargo run -p presencehub-league --example observe
//! ```

use presencehub_league::LeaguePlugin;
use presencehub_plugin_host::Plugin;

fn main() {
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .init();

    let mut plugin = LeaguePlugin::new();
    if let Err(e) = plugin.init() {
        eprintln!("init failed: {e}");
        std::process::exit(1);
    }

    for i in 0..20 {
        match plugin.poll() {
            Ok(Some(activity)) => {
                println!(
                    "[{i:3}] {} | {:?} | start={:?}",
                    activity.state,
                    activity.details,
                    activity.timestamps.as_ref().and_then(|t| t.start),
                );
                for (k, v) in &activity.metadata {
                    println!("          {k} = {v}");
                }
            }
            Ok(None) => println!("[{i:3}] (unchanged)"),
            Err(e) => println!("[{i:3}] err: {e}"),
        }
        std::thread::sleep(std::time::Duration::from_secs(1));
    }

    let _ = plugin.shutdown();
    println!("done");
}
