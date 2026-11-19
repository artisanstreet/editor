use artisan_account::network::{CoreSessionClient, SessionToken};

use crate::core::ae::{Ae, InstalledAe};
use crate::core::client::{ForgeClient, RelayClientError};
use crate::core::clock::Clock;
use crate::core::runtime::run_http;
use artisan_account::network::MachineEntry as AccountMachine;
use futures::future::BoxFuture;
use std::sync::Arc;

const RELAY_URL_DEFAULT: &str = "wss://relay.editor.artisan.st";

#[derive(Debug, Clone)]
pub struct Services {
    pub(crate) core_url: Option<String>,
    pub(crate) ae: Ae,
    pub(crate) relay: RelayRoute,
    pub(crate) clock: Clock,
}

pub trait DialRelay: Send + Sync {
    fn dial(
        &self,
        request: RelayDialRequest,
        core: CoreSessionClient,
        session: SessionToken,
    ) -> BoxFuture<'static, Result<ForgeClient, RelayClientError>>;
}

#[derive(Debug, Clone)]
pub struct HostedRelay {
    url: String,
}

impl DialRelay for HostedRelay {
    fn dial(
        &self,
        request: RelayDialRequest,
        core: CoreSessionClient,
        session: SessionToken,
    ) -> BoxFuture<'static, Result<ForgeClient, RelayClientError>> {
        let context = artisan_account::relay_client::RelayDialContext {
            relay_base_url: self.url.clone(),
            core,
            session,
        };
        Box::pin(async move { ForgeClient::via_relay(request, &context).await })
    }
}

#[derive(Clone)]
pub enum RelayRoute {
    Dial(Arc<dyn DialRelay>),
    Off,
}

impl RelayRoute {
    pub fn dial(relay: impl DialRelay + 'static) -> Self {
        Self::Dial(Arc::new(relay))
    }
}

impl Services {
    #[must_use]
    pub(crate) fn installed() -> Self {
        let relay = match std::env::var("AST_RELAY_URL") {
            Ok(url) if url.trim().is_empty() => RelayRoute::Off,
            Ok(url) => RelayRoute::dial(HostedRelay {
                url: url.trim().to_owned(),
            }),
            Err(_) => RelayRoute::dial(HostedRelay {
                url: RELAY_URL_DEFAULT.to_owned(),
            }),
        };
        Self {
            core_url: None,
            ae: Ae::new(InstalledAe),
            relay,
            clock: Clock::system(),
        }
    }

    #[must_use]
    pub fn new(core_url: &str, ae: Ae, relay: RelayRoute, clock: Clock) -> Self {
        Self {
            core_url: Some(core_url.to_owned()),
            ae,
            relay,
            clock,
        }
    }
}

pub struct RelayDialRequest {
    pub(crate) account_id: String,
    pub(crate) machine_id: String,
    pub(crate) forge_ed25519_b64u: String,
    pub(crate) grant_jwt: String,
    pub(crate) cnf_secret: [u8; 32],
}

impl RelayDialRequest {
    #[must_use]
    pub fn machine_id(&self) -> &str {
        &self.machine_id
    }
}

impl Drop for RelayDialRequest {
    fn drop(&mut self) {
        self.cnf_secret.fill(0);
    }
}

#[derive(Debug, thiserror::Error)]
pub enum OpenRelayError {
    #[error("the Relay is turned off (AST_RELAY_URL is empty), so {machine} cannot be reached")]
    Off { machine: String },
    #[error("make a connection key for {machine}")]
    Key {
        machine: String,
        #[source]
        source: artisan_account::MachineKeyError,
    },
    #[error("ask the account service for a Relay grant to {machine}")]
    Grant {
        machine: String,
        #[source]
        source: artisan_account::network::NetworkError,
    },
    #[error("dial {machine} through the Relay")]
    Dial {
        machine: String,
        #[source]
        source: Box<RelayClientError>,
    },
}

pub(crate) async fn open_relay(
    route: RelayRoute,
    core: CoreSessionClient,
    session: SessionToken,
    account_id: String,
    machine: AccountMachine,
) -> Result<ForgeClient, OpenRelayError> {
    let machine_id = machine.id.clone();
    if matches!(route, RelayRoute::Off) {
        return Err(OpenRelayError::Off {
            machine: machine_id,
        });
    }
    let keypair =
        artisan_account::MachineKeypair::generate().map_err(|source| OpenRelayError::Key {
            machine: machine_id.clone(),
            source,
        })?;
    let public = keypair.public_b64u();
    let grant = run_http({
        let (core, session, machine_id) = (core.clone(), session.clone(), machine.id.clone());
        async move {
            core.issue_editor_grant(&session, &machine_id, &public)
                .await
        }
    })
    .await
    .map_err(|source| OpenRelayError::Grant {
        machine: machine_id.clone(),
        source,
    })?;
    let request = RelayDialRequest {
        account_id,
        machine_id: machine.id,
        forge_ed25519_b64u: machine.public_key.x,
        grant_jwt: grant.token,
        cnf_secret: *keypair.secret(),
    };
    let dialed = match route {
        RelayRoute::Dial(relay) => relay.dial(request, core, session).await,
        RelayRoute::Off => unreachable!("invariant: a Relay that is off was refused above"),
    };
    dialed.map_err(|source| OpenRelayError::Dial {
        machine: machine_id,
        source: Box::new(source),
    })
}

impl std::fmt::Debug for RelayRoute {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Dial(_) => f.write_str("Dial"),
            Self::Off => f.write_str("Off"),
        }
    }
}

impl std::fmt::Debug for RelayDialRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RelayDialRequest")
            .field("account_id", &self.account_id)
            .field("machine_id", &self.machine_id)
            .finish_non_exhaustive()
    }
}
