//! What a Forge runs for its enrolled machine: config sync and the Relay
//! connector. Enrollment is read once at startup; `ae account confirm`
//! restarts a running Forge, so nothing here waits for it.

use anyhow::{Context, Result};
use std::path::PathBuf;
use std::time::Duration;

const CONFIG_SYNC_INTERVAL: Duration = Duration::from_mins(5);
const RELAY_BACKOFF_MIN: Duration = Duration::from_secs(1);
const RELAY_BACKOFF_MAX: Duration = Duration::from_secs(60);
/// A connection that stayed up this long was healthy, so the next
/// reconnect starts from the minimum backoff again.
const RELAY_HEALTHY_AFTER: Duration = Duration::from_secs(60);

const DEFAULT_RELAY_URL: &str = "wss://relay.editor.artisan.st";

/// `AST_RELAY_URL` when set and non-empty, the production Relay when unset,
/// and `None` (connector off) when set but blank.
fn relay_url() -> Option<String> {
    std::env::var("AST_RELAY_URL").map_or_else(
        |_| Some(DEFAULT_RELAY_URL.to_owned()),
        |url| Some(url.trim().to_owned()).filter(|url| !url.is_empty()),
    )
}

/// The issuer grants must carry: `AST_CORE_ISSUER`, else the Core base URL.
fn core_issuer() -> String {
    std::env::var("AST_CORE_ISSUER")
        .ok()
        .map(|issuer| issuer.trim().to_owned())
        .filter(|issuer| !issuer.is_empty())
        .unwrap_or_else(|| crate::core::core_base_url(None).trim().to_owned())
}

pub async fn run(
    data_dir: PathBuf,
    app: crate::routes::App,
    shutdown: tokio_util::sync::CancellationToken,
    tasks: tokio_util::task::TaskTracker,
) {
    let identity = match crate::sync::load_machine_identity(&data_dir) {
        Ok(Some(identity)) => identity,
        Ok(None) => return,
        Err(error) => {
            tracing::warn!("account services off: {error:#}");
            return;
        }
    };
    let Some(account_id) = identity.account_id.clone() else {
        return;
    };
    tracing::info!(machine = %identity.machine_id, "account services starting");
    tasks.spawn(crate::sync::run_config_loop(
        data_dir,
        app.hub.clone(),
        shutdown.clone(),
        CONFIG_SYNC_INTERVAL,
    ));
    let Some(url) = relay_url() else {
        tracing::info!("AST_RELAY_URL is empty; the Relay connector stays off");
        return;
    };
    if let Err(error) = relay_loop(
        &url,
        &identity.machine_id,
        &account_id,
        &identity.secret,
        app,
        &shutdown,
    )
    .await
    {
        tracing::warn!("Relay connector off: {error:#}");
    }
}

async fn relay_loop(
    relay_base_url: &str,
    machine_id: &str,
    account_id: &str,
    secret: &[u8; 32],
    app: crate::routes::App,
    shutdown: &tokio_util::sync::CancellationToken,
) -> Result<()> {
    let core_base_url = crate::core::core_base_url(None);
    let client = crate::core::CoreClient::new(&core_base_url)?;
    let verifier = std::sync::Arc::new(crate::account_auth::GrantVerifier::new(
        &format!(
            "{}/.well-known/jwks.json",
            core_base_url.trim_end_matches('/')
        ),
        &core_issuer(),
    )?);
    let ws_url = format!(
        "{}/v1/route/{account_id}/{machine_id}",
        relay_base_url.trim_end_matches('/')
    )
    .replacen("https://", "wss://", 1)
    .replacen("http://", "ws://", 1);
    let mut backoff = RELAY_BACKOFF_MIN;
    let mut failing = false;
    while !shutdown.is_cancelled() {
        let started = tokio::time::Instant::now();
        let turn = relay_turn(
            &client, &ws_url, machine_id, account_id, secret, &verifier, &app,
        );
        let result = tokio::select! {
            result = turn => result,
            () = shutdown.cancelled() => return Ok(()),
        };
        if started.elapsed() >= RELAY_HEALTHY_AFTER {
            backoff = RELAY_BACKOFF_MIN;
            failing = false;
        }
        if let Err(error) = result {
            if failing {
                tracing::debug!("Relay connection ended: {error:#}");
            } else {
                tracing::warn!("Relay connection ended: {error:#}");
                failing = true;
            }
        }
        tokio::select! {
            () = shutdown.cancelled() => return Ok(()),
            () = tokio::time::sleep(backoff) => {},
        }
        backoff = (backoff * 2).min(RELAY_BACKOFF_MAX);
    }
    Ok(())
}

/// One connection: a fresh machine grant, then serve until the socket ends.
async fn relay_turn(
    client: &crate::core::CoreClient,
    ws_url: &str,
    machine_id: &str,
    account_id: &str,
    secret: &[u8; 32],
    verifier: &std::sync::Arc<crate::account_auth::GrantVerifier>,
    app: &crate::routes::App,
) -> Result<()> {
    let issued = client
        .issue_machine_grant(
            machine_id,
            secret,
            "ast-relay",
            &["relay:connect"],
            300,
            Some(machine_id),
        )
        .await
        .context("issue a Relay grant")?;
    let jti = artisan_account::unverified_jti(&issued.token).context("read the Relay grant id")?;
    crate::relay::serve_channel(
        ws_url.to_owned(),
        issued.token,
        jti,
        *secret,
        *secret,
        account_id.to_owned(),
        machine_id.to_owned(),
        std::sync::Arc::clone(verifier),
        app.clone(),
    )
    .await
}
