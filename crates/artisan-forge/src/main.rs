//! The standalone Forge host executable.
//!
//! Startup order mirrors `modules/forge/src/entry.ts`: decode config, acquire
//! the database lease, bind loopback, publish the state card; shutdown drains
//! in reverse — stop accepting, end sessions, release the lease, remove the
//! state card.

use std::process::ExitCode;

use artisan_forge::config::ForgeConfig;
use artisan_forge::host::run_forge;

fn main() -> ExitCode {
    let mut arguments = std::env::args().skip(1);
    let config_path = arguments
        .next()
        .or_else(|| std::env::var("ARTISAN_FORGE_CONFIG").ok())
        .unwrap_or_else(|| "forge.toml".to_string());
    let state_path = std::env::var("ARTISAN_FORGE_STATE_PATH")
        .ok()
        .map(std::path::PathBuf::from);

    let runtime = match tokio::runtime::Runtime::new() {
        Ok(runtime) => runtime,
        Err(error) => {
            failure_event("tokio_runtime", &error.to_string());
            return ExitCode::FAILURE;
        }
    };

    runtime.block_on(async move {
        let text = match std::fs::read_to_string(&config_path) {
            Ok(text) => text,
            Err(error) => {
                failure_event("config_read", &error.to_string());
                return ExitCode::FAILURE;
            }
        };
        let config = match ForgeConfig::decode_toml(&text) {
            Ok(config) => config,
            Err(error) => {
                failure_event("config_decode", &error.to_string());
                return ExitCode::FAILURE;
            }
        };
        match run_forge(config, state_path).await {
            Ok(handle) => {
                started_event(&handle.endpoint().to_string());
                if tokio::signal::ctrl_c().await.is_err() {
                    failure_event("shutdown_signal", "ctrl-c listener failed");
                    return ExitCode::FAILURE;
                }
                handle.shutdown().await;
                ExitCode::SUCCESS
            }
            Err(error) => {
                failure_event("forge_start", &error.to_string());
                ExitCode::FAILURE
            }
        }
    })
}

/// One JSON line on stderr per lifecycle event, matching the legacy host's
/// structured console output shape.
fn failure_event(event: &str, message: &str) {
    eprintln!(
        "{}",
        serde_json::json!({ "event": format!("forge.{event}"), "message": message })
    );
}

fn started_event(endpoint: &str) {
    eprintln!(
        "{}",
        serde_json::json!({ "event": "forge.started", "endpoint": endpoint })
    );
}
