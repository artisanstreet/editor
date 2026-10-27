//! Account services: what a Forge daemon runs for its enrolled machine.
//!
//! Started automatically from [`crate::start`] when the data dir holds an
//! enrolled machine (`ast_account.json`, written by `ae account
//! enroll/confirm`). Accountless Forges are untouched: this returns
//! immediately without them.
//!
//! - Config sync loop: conditional fetch, verify, apply through the Hub,
//!   report; every 5 minutes plus once at startup.
//! - Relay connector loop: fresh short-lived machine grant per connection
//!   (Core-issued, single-use `jti`), PoP admission, Noise channels,
//!   reconnect with backoff. Grant renewal = reconnect with a new grant;
//!   the TTL is short by design (≤ 300 s).

use anyhow::{Context, Result};
use std::path::PathBuf;
use std::time::Duration;

/// How often config is re-polled (plus once at startup).
pub const CONFIG_SYNC_INTERVAL: Duration = Duration::from_mins(5);
/// Relay reconnect backoff bounds.
pub const RELAY_BACKOFF_MIN: Duration = Duration::from_secs(1);
pub const RELAY_BACKOFF_MAX: Duration = Duration::from_secs(60);

/// Relay URL comes from deployment config (`AST_RELAY_URL`,
/// e.g. `wss://relay.example.invalid`). No default: without an explicit
/// relay the connector stays off while config sync still runs.
pub fn relay_url() -> Option<String> {
    std::env::var("AST_RELAY_URL")
        .ok()
        .map(|url| url.trim().to_owned())
        .filter(|url| !url.is_empty())
}

/// Core issuer the Forge expects in grants (`AST_CORE_ISSUER`, defaulting
/// to the Core dev issuer). Must match Core's `CORE_JWT_ISSUER` exactly.
pub fn core_issuer() -> String {
    std::env::var("AST_CORE_ISSUER")
        .ok()
        .map(|issuer| issuer.trim().to_owned())
        .filter(|issuer| !issuer.is_empty())
        .unwrap_or_else(|| "http://127.0.0.1:8787".into())
}

/// Run account services until `shutdown`. Returns immediately for
/// accountless Forges.
pub async fn run(
    data_dir: PathBuf,
    hub: forge_runs::Hub,
    shutdown: tokio_util::sync::CancellationToken,
    tasks: tokio_util::task::TaskTracker,
) {
    let identity = match crate::sync::load_machine_identity(&data_dir) {
        Ok(identity) => identity,
        Err(error) => {
            tracing::warn!("account services disabled: {error:#}");
            return;
        }
    };
    let Some(identity) = identity else {
        return;
    };
    tracing::info!(machine = %identity.machine_id, "account services starting");
    tasks.spawn(crate::sync::run_config_loop(
        data_dir,
        hub.clone(),
        shutdown.clone(),
        CONFIG_SYNC_INTERVAL,
    ));
    let Some(url) = relay_url() else {
        tracing::info!("no AST_RELAY_URL configured; Relay connector stays off");
        return;
    };
    let Some(account_id) = identity.account_id else {
        tracing::warn!("Relay connector disabled: no account id recorded (re-confirm enrollment)");
        return;
    };
    tasks.spawn(relay_loop(
        url,
        identity.machine_id,
        account_id,
        identity.secret,
        hub,
        shutdown,
    ));
}

async fn relay_loop(
    relay_base_url: String,
    machine_id: String,
    account_id: String,
    secret: [u8; 32],
    hub: forge_runs::Hub,
    shutdown: tokio_util::sync::CancellationToken,
) {
    let client = match crate::core::CoreClient::new(&crate::core::core_base_url(None)) {
        Ok(client) => client,
        Err(error) => {
            tracing::warn!("Relay connector disabled: {error:#}");
            return;
        }
    };
    let verifier = match crate::account_auth::GrantVerifier::new(
        &format!(
            "{}/.well-known/jwks.json",
            crate::core::core_base_url(None).trim_end_matches('/')
        ),
        &core_issuer(),
    ) {
        Ok(verifier) => std::sync::Arc::new(verifier),
        Err(error) => {
            tracing::warn!("Relay connector disabled: {error:#}");
            return;
        }
    };
    let app = match account_app(hub, shutdown.clone()) {
        Ok(app) => app,
        Err(error) => {
            tracing::warn!("Relay connector disabled: {error:#}");
            return;
        }
    };
    let ws_url = format!(
        "{}/v1/route/{account_id}/{machine_id}",
        relay_base_url.trim_end_matches('/')
    );
    let ws_url = ws_url
        .replacen("https://", "wss://", 1)
        .replacen("http://", "ws://", 1);
    let mut backoff = RELAY_BACKOFF_MIN;
    loop {
        if shutdown.is_cancelled() {
            break;
        }
        if let Err(error) = relay_turn(
            &client,
            &ws_url,
            &machine_id,
            &account_id,
            &secret,
            &verifier,
            &app,
            shutdown.clone(),
        )
        .await
        {
            tracing::warn!("Relay connection ended: {error:#}");
        }
        tokio::select! {
            () = shutdown.cancelled() => break,
            () = tokio::time::sleep(backoff) => {},
        }
        backoff = (backoff * 2).min(RELAY_BACKOFF_MAX);
    }
}

/// The local app the tunnel dispatches into (same routes as direct HTTPS).
fn account_app(
    hub: forge_runs::Hub,
    shutdown: tokio_util::sync::CancellationToken,
) -> Result<crate::routes::App> {
    Ok(crate::routes::App {
        hub,
        forge_version: env!("CARGO_PKG_VERSION").to_owned(),
        shutdown,
        tasks: tokio_util::task::TaskTracker::new(),
        repositories: std::sync::Arc::new(crate::repository::Repositories::new()?),
        icons: Default::default(),
    })
}

/// Configuration for one tunneled Relay connection, explicit so external
/// harnesses (integration tests, alternate binaries) never duplicate
/// Forge's private route/repository types: the Hub and shutdown are the
/// only production objects the caller provides.
#[derive(Clone, Debug)]
pub struct AccountRelayConfig {
    /// Full Relay route URL, e.g. `wss://relay.example/v1/route/acct_x/mch_y`.
    pub ws_url: String,
    /// Core base URL for grant issuance + JWKS fetches.
    pub core_base_url: String,
    /// This machine's id (route binding + grant subject).
    pub machine_id: String,
    /// Enrolled account id (route + grant binding).
    pub account_id: String,
    /// Machine proof key; also the admission PoP and confirm key (the
    /// machine grant's `cnf` defaults to the enrolled key, Core §4).
    pub machine_secret: [u8; 32],
    /// Expected Core issuer for grant verification.
    pub expected_iss: String,
}

/// Serve Relay channels until `shutdown`: fresh short-lived machine grant
/// per connection (Core-issued, single-use `jti`), PoP admission, Noise
/// channels, reconnect with backoff. This is the production entry point
/// for harnesses that already own a Hub (same Hub/Store the Forge serves).
pub async fn serve_account_relay(
    config: AccountRelayConfig,
    hub: forge_runs::Hub,
    shutdown: tokio_util::sync::CancellationToken,
) {
    let client = match crate::core::CoreClient::new(&config.core_base_url) {
        Ok(client) => client,
        Err(error) => {
            tracing::warn!("Relay connector disabled: {error:#}");
            return;
        }
    };
    let verifier = match crate::account_auth::GrantVerifier::new(
        &format!(
            "{}/.well-known/jwks.json",
            config.core_base_url.trim_end_matches('/')
        ),
        &config.expected_iss,
    ) {
        Ok(verifier) => std::sync::Arc::new(verifier),
        Err(error) => {
            tracing::warn!("Relay connector disabled: {error:#}");
            return;
        }
    };
    let app = match account_app(hub, shutdown.clone()) {
        Ok(app) => app,
        Err(error) => {
            tracing::warn!("Relay connector disabled: {error:#}");
            return;
        }
    };
    let mut backoff = RELAY_BACKOFF_MIN;
    loop {
        if shutdown.is_cancelled() {
            break;
        }
        if let Err(error) = relay_turn(
            &client,
            &config.ws_url,
            &config.machine_id,
            &config.account_id,
            &config.machine_secret,
            &verifier,
            &app,
            shutdown.clone(),
        )
        .await
        {
            tracing::warn!("Relay connection ended: {error:#}");
        }
        tokio::select! {
            () = shutdown.cancelled() => break,
            () = tokio::time::sleep(backoff) => {},
        }
        backoff = (backoff * 2).min(RELAY_BACKOFF_MAX);
    }
}

/// One connection: fresh grant, PoP key, then serve until the socket
/// closes or shutdown arrives. `serve_channel` owns the socket; dropping
/// the abort handle ends it promptly on shutdown.
#[expect(
    clippy::too_many_arguments,
    reason = "one connection turn needs all of these"
)]
async fn relay_turn(
    client: &crate::core::CoreClient,
    ws_url: &str,
    machine_id: &str,
    account_id: &str,
    secret: &[u8; 32],
    verifier: &std::sync::Arc<crate::account_auth::GrantVerifier>,
    app: &crate::routes::App,
    shutdown: tokio_util::sync::CancellationToken,
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
    let jti = artisan_account::unverified_jti(&issued.token)
        .map_err(|_| anyhow::anyhow!("Core grant has no id"))?;
    // Machine grants bind cnf to the enrolled key: PoP + confirm signatures
    // use the same local secret.
    let serve = crate::relay::serve_channel(
        ws_url.to_owned(),
        issued.token,
        jti,
        *secret,
        *secret,
        account_id.to_owned(),
        machine_id.to_owned(),
        std::sync::Arc::clone(verifier),
        app.clone(),
    );
    tokio::select! {
        result = serve => result,
        () = shutdown.cancelled() => Ok(()),
    }
}
