use std::path::PathBuf;
use std::time::Duration;

const CONFIG_SYNC_INTERVAL: Duration = Duration::from_mins(5);
const RELAY_BACKOFF_MIN: Duration = Duration::from_secs(1);
const RELAY_BACKOFF_MAX: Duration = Duration::from_secs(60);
const RELAY_HEALTHY_AFTER: Duration = Duration::from_secs(60);

const DEFAULT_RELAY_URL: &str = "wss://relay.editor.artisan.st";

fn relay_url() -> Option<String> {
    std::env::var("AST_RELAY_URL").map_or_else(
        |_| Some(DEFAULT_RELAY_URL.to_owned()),
        |url| Some(url.trim().to_owned()).filter(|url| !url.is_empty()),
    )
}

fn core_issuer() -> String {
    std::env::var("AST_CORE_ISSUER")
        .ok()
        .map(|issuer| issuer.trim().to_owned())
        .filter(|issuer| !issuer.is_empty())
        .unwrap_or_else(|| crate::core::core_base_url(None).trim().to_owned())
}

pub(crate) async fn run(
    data_dir: PathBuf,
    app: crate::routes::App,
    shutdown: tokio_util::sync::CancellationToken,
    tasks: tokio_util::task::TaskTracker,
) {
    let identity = match crate::sync::load_machine_identity(&data_dir) {
        Ok(Some(identity)) => identity,
        Ok(None) => return,
        Err(error) => {
            tracing::warn!(error = diagnostics::chain(&error), "account services off");
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
        identity.clone(),
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
        tracing::warn!(
            machine = %identity.machine_id,
            error = diagnostics::chain(&error),
            "Relay connector off"
        );
    }
}

#[derive(Debug, thiserror::Error)]
enum RelayLoopError {
    #[error(transparent)]
    Core(#[from] crate::core::BuildCoreClientError),
    #[error(transparent)]
    Verifier(#[from] crate::account_auth::BuildVerifierError),
}

#[derive(Debug, thiserror::Error)]
enum RelayTurnError {
    #[error("issue a Relay grant for machine {machine}")]
    Grant {
        machine: String,
        #[source]
        source: crate::core::CoreCallError,
    },
    #[error("read the id of machine {machine}'s Relay grant")]
    GrantId {
        machine: String,
        #[source]
        source: artisan_account::UnverifiedGrantError,
    },
    #[error(transparent)]
    Serve(#[from] crate::relay::ServeChannelError),
}

async fn relay_loop(
    relay_base_url: &str,
    machine_id: &str,
    account_id: &str,
    secret: &[u8; 32],
    app: crate::routes::App,
    shutdown: &tokio_util::sync::CancellationToken,
) -> Result<(), RelayLoopError> {
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
                tracing::debug!(
                    machine = machine_id,
                    error = diagnostics::chain(&error),
                    "Relay connection ended"
                );
            } else {
                tracing::warn!(
                    machine = machine_id,
                    error = diagnostics::chain(&error),
                    "Relay connection ended"
                );
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

async fn relay_turn(
    client: &crate::core::CoreClient,
    ws_url: &str,
    machine_id: &str,
    account_id: &str,
    secret: &[u8; 32],
    verifier: &std::sync::Arc<crate::account_auth::GrantVerifier>,
    app: &crate::routes::App,
) -> Result<(), RelayTurnError> {
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
        .map_err(|source| RelayTurnError::Grant {
            machine: machine_id.to_owned(),
            source,
        })?;
    let jti = artisan_account::unverified_jti(&issued.token).map_err(|source| {
        RelayTurnError::GrantId {
            machine: machine_id.to_owned(),
            source,
        }
    })?;
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
    .await?;
    Ok(())
}
