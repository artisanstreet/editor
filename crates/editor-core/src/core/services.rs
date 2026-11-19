use artisan_account::network::{CoreSessionClient, SessionToken};

use crate::core::ae::Ae;
use crate::core::client::{ForgeClient, RelayClientError};
use crate::core::clock::Clock;
use crate::core::runtime::run_http;
use artisan_account::network::MachineEntry as AccountMachine;

const RELAY_URL_DEFAULT: &str = "wss://relay.editor.artisan.st";

#[derive(Debug, Clone)]
pub struct Services {
    pub(crate) core_url: Option<String>,
    pub(crate) ae: Ae,
    pub(crate) relay: RelayRoute,
    pub(crate) clock: Clock,
}

#[derive(Clone)]
pub enum RelayRoute {
    Url(String),
    Off,
    #[cfg(any(test, feature = "test-support"))]
    Scripted(RelayScript),
}

#[cfg(any(test, feature = "test-support"))]
pub type RelayScript = std::sync::Arc<
    dyn Fn(&RelayDialRequest) -> Result<ForgeClient, RelayClientError> + Send + Sync,
>;

impl Services {
    #[must_use]
    pub(crate) fn installed() -> Self {
        let relay = match std::env::var("AST_RELAY_URL") {
            Ok(url) if url.trim().is_empty() => RelayRoute::Off,
            Ok(url) => RelayRoute::Url(url.trim().to_owned()),
            Err(_) => RelayRoute::Url(RELAY_URL_DEFAULT.to_owned()),
        };
        Self {
            core_url: None,
            ae: Ae::Installed,
            relay,
            clock: Clock::system(),
        }
    }

    #[cfg(feature = "test-support")]
    #[must_use]
    pub(crate) fn with_clock(self, clock: Clock) -> Self {
        Self { clock, ..self }
    }

    #[cfg(any(test, feature = "test-support"))]
    #[must_use]
    pub fn scripted(
        core_url: &str,
        ae: crate::core::ae::AeScript,
        relay: Option<RelayScript>,
    ) -> Self {
        Self {
            core_url: Some(core_url.to_owned()),
            ae: Ae::Scripted(ae),
            relay: relay.map_or(RelayRoute::Off, RelayRoute::Scripted),
            clock: Clock::system(),
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
        RelayRoute::Url(relay_base_url) => {
            let context = artisan_account::relay_client::RelayDialContext {
                relay_base_url,
                core,
                session,
            };
            ForgeClient::via_relay(request, &context).await
        }
        RelayRoute::Off => unreachable!("invariant: a Relay that is off was refused above"),
        #[cfg(any(test, feature = "test-support"))]
        RelayRoute::Scripted(open) => open(&request),
    };
    dialed.map_err(|source| OpenRelayError::Dial {
        machine: machine_id,
        source: Box::new(source),
    })
}

impl std::fmt::Debug for RelayRoute {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Url(url) => f.debug_tuple("Url").field(url).finish(),
            Self::Off => f.write_str("Off"),
            #[cfg(any(test, feature = "test-support"))]
            Self::Scripted(_) => f.write_str("Scripted"),
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
