//! What the Editor talks to besides its Forges: Core (the account service),
//! the installed `ae`, and the Relay.

use anyhow::{Context as _, Result, bail};
use artisan_account::network::{CoreSessionClient, SessionToken};

use crate::account::{AccountMachine, run_http};
use crate::ae::Ae;
use crate::client::ForgeClient;

const RELAY_URL_DEFAULT: &str = "wss://relay.editor.artisan.st";

#[derive(Clone)]
pub struct Services {
    pub(crate) core_url: Option<String>,
    pub(crate) ae: Ae,
    pub(crate) relay: RelayRoute,
}

#[derive(Clone)]
pub enum RelayRoute {
    Url(String),
    /// `AST_RELAY_URL` is set and empty.
    Off,
    #[cfg(any(test, feature = "test-support"))]
    Scripted(RelayScript),
}

/// Opens the channel for a prepared Relay request.
#[cfg(any(test, feature = "test-support"))]
pub type RelayScript =
    std::sync::Arc<dyn Fn(&RelayDialRequest) -> Result<ForgeClient> + Send + Sync>;

impl Services {
    /// Production: Core from the account file or `AST_CORE_BASE_URL`, the
    /// installed `ae`, and the Relay from `AST_RELAY_URL`.
    #[must_use]
    pub fn installed() -> Self {
        let relay = match std::env::var("AST_RELAY_URL") {
            Ok(url) if url.trim().is_empty() => RelayRoute::Off,
            Ok(url) => RelayRoute::Url(url.trim().to_owned()),
            Err(_) => RelayRoute::Url(RELAY_URL_DEFAULT.to_owned()),
        };
        Self {
            core_url: None,
            ae: Ae::Installed,
            relay,
        }
    }

    /// A Core at `core_url`, a scripted `ae`, and a scripted Relay (`None`:
    /// the Relay is off).
    #[cfg(any(test, feature = "test-support"))]
    #[must_use]
    pub fn scripted(core_url: &str, ae: crate::ae::AeScript, relay: Option<RelayScript>) -> Self {
        Self {
            core_url: Some(core_url.to_owned()),
            ae: Ae::Scripted(ae),
            relay: relay.map_or(RelayRoute::Off, RelayRoute::Scripted),
        }
    }
}

/// Everything needed to open a Relay channel to one Forge.
pub struct RelayDialRequest {
    pub account_id: String,
    pub machine_id: String,
    /// The Forge's enrolled Ed25519 key (base64url) its handshake must match.
    pub forge_ed25519_b64u: String,
    /// Short-lived Core grant (`aud=ast-relay`), presented in-band.
    pub grant_jwt: String,
    /// The grant's ephemeral `cnf` key.
    pub cnf_secret: [u8; 32],
}

impl Drop for RelayDialRequest {
    fn drop(&mut self) {
        self.cnf_secret.fill(0);
    }
}

/// Asks Core for a grant to `machine` and opens a Relay channel with it.
pub async fn open_relay(
    route: RelayRoute,
    core: CoreSessionClient,
    session: SessionToken,
    account_id: String,
    machine: AccountMachine,
) -> Result<ForgeClient> {
    if matches!(route, RelayRoute::Off) {
        bail!("the Relay is turned off (AST_RELAY_URL is empty)");
    }
    let keypair = artisan_account::MachineKeypair::generate().context("make a connection key")?;
    let public = keypair.public_b64u();
    let grant = run_http({
        let (core, session, machine_id) = (core.clone(), session.clone(), machine.id.clone());
        async move {
            core.issue_editor_grant(&session, &machine_id, &public)
                .await
        }
    })
    .await
    .context("ask the account service for a Relay grant")?;
    let request = RelayDialRequest {
        account_id,
        machine_id: machine.id,
        forge_ed25519_b64u: machine.public_key.x,
        grant_jwt: grant.token,
        cnf_secret: *keypair.secret(),
    };
    match route {
        RelayRoute::Url(relay_base_url) => {
            let context = artisan_account::relay_client::RelayDialContext {
                relay_base_url,
                core,
                session,
            };
            ForgeClient::via_relay(request, &context).await
        }
        RelayRoute::Off => bail!("the Relay is turned off"),
        #[cfg(any(test, feature = "test-support"))]
        RelayRoute::Scripted(open) => open(&request),
    }
}
