use crate::error::ErrorClass;
use crate::mcp_reach::Endpoint;
use artisan_api::{MCP_AUTH_KEY, McpAuthStatus, McpServer, McpSignInStarted};
use forge_engines::mcp_oauth::{
    self, Client, Discovery, Pkce, Probe, ProbeError, RegistrationError, Revocation, TokenError,
    Tokens,
};
use forge_store::{
    McpSecret, ReadSecretError, ReadSettingError, SetMcpSecretError, SetSecretError,
    SetSettingError, Store,
};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const FLOW_LASTS: Duration = Duration::from_mins(10);

#[derive(Default)]
struct Flows {
    by_state: HashMap<String, Flow>,
    lifetimes: HashMap<String, Arc<tokio::sync::Mutex<Lifetime>>>,
}

#[derive(Default)]
struct Lifetime {
    signed_out: u64,
}

struct Flow {
    server: String,
    url: String,
    signed_out: u64,
    discovery: Discovery,
    client: Client,
    verifier: artisan_api::Secret,
    authorize_url: String,
    started: Instant,
}

impl Flow {
    fn alive(&self) -> bool {
        self.started.elapsed() < FLOW_LASTS
    }
}

#[derive(Serialize, Deserialize)]
struct Saved {
    url: String,
    discovery: Discovery,
    client: Client,
    tokens: Tokens,
}

#[derive(Debug, thiserror::Error)]
pub enum ReadMcpServersError {
    #[error("read the MCP server list")]
    Read(#[source] ReadSettingError),
    #[error("decode the MCP server list")]
    Decode(#[source] serde_json::Error),
}

pub(crate) async fn read_mcp_servers(store: &Store) -> Result<Vec<McpServer>, ReadMcpServersError> {
    store
        .setting("mcp_servers")
        .await
        .map_err(ReadMcpServersError::Read)?
        .map(serde_json::from_value)
        .transpose()
        .map_err(ReadMcpServersError::Decode)
        .map(Option::unwrap_or_default)
}

#[derive(Debug, thiserror::Error)]
pub enum McpAuthError {
    #[error("read Forge's {MCP_AUTH_KEY} setting")]
    Read(#[source] ReadSettingError),
    #[error("decode Forge's {MCP_AUTH_KEY} setting")]
    Decode(#[source] serde_json::Error),
}

impl McpAuthError {
    #[must_use]
    pub const fn class(&self) -> ErrorClass {
        match self {
            Self::Read(_) | Self::Decode(_) => ErrorClass::Internal,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum SetAuthError {
    #[error("read the MCP sign-in states")]
    Read(#[source] McpAuthError),
    #[error("encode the MCP sign-in states")]
    Encode(#[source] serde_json::Error),
    #[error("save the MCP sign-in states")]
    Save(#[source] SetSettingError),
}

#[derive(Debug, thiserror::Error)]
pub enum McpStoreError {
    #[error("read the MCP servers")]
    Servers(#[source] ReadMcpServersError),
    #[error("read the MCP sign-in states")]
    ReadStatuses(#[source] McpAuthError),
    #[error("encode the MCP sign-in states")]
    EncodeStatuses(#[source] serde_json::Error),
    #[error("record the MCP sign-in states")]
    RecordStatuses(#[source] SetSettingError),
    #[error("record the sign-in state of MCP server {server}")]
    RecordAuth {
        server: String,
        #[source]
        source: SetAuthError,
    },
    #[error("{step} for MCP server {server}")]
    ReadSecret {
        server: String,
        step: &'static str,
        #[source]
        source: ReadSecretError,
    },
    #[error("{step} for MCP server {server}")]
    SaveSecret {
        server: String,
        step: &'static str,
        #[source]
        source: SetMcpSecretError,
    },
    #[error("{step} for MCP server {server}")]
    ForgetSecret {
        server: String,
        step: &'static str,
        #[source]
        source: SetSecretError,
    },
    #[error("{step} for MCP server {server}")]
    Encoding {
        server: String,
        step: &'static str,
        #[source]
        source: serde_json::Error,
    },
}

impl McpStoreError {
    #[must_use]
    pub const fn class(&self) -> ErrorClass {
        match self {
            Self::Servers(_)
            | Self::ReadStatuses(_)
            | Self::EncodeStatuses(_)
            | Self::RecordStatuses(_)
            | Self::RecordAuth { .. }
            | Self::ReadSecret { .. }
            | Self::SaveSecret { .. }
            | Self::ForgetSecret { .. }
            | Self::Encoding { .. } => ErrorClass::Internal,
        }
    }
}

fn read_secret_error<'a>(
    server: &'a str,
    step: &'static str,
) -> impl FnOnce(ReadSecretError) -> McpStoreError + 'a {
    move |source| McpStoreError::ReadSecret {
        server: server.to_owned(),
        step,
        source,
    }
}

fn save_secret_error<'a>(
    server: &'a str,
    step: &'static str,
) -> impl FnOnce(SetMcpSecretError) -> McpStoreError + 'a {
    move |source| McpStoreError::SaveSecret {
        server: server.to_owned(),
        step,
        source,
    }
}

fn forget_secret_error<'a>(
    server: &'a str,
    step: &'static str,
) -> impl FnOnce(SetSecretError) -> McpStoreError + 'a {
    move |source| McpStoreError::ForgetSecret {
        server: server.to_owned(),
        step,
        source,
    }
}

fn encoding_error<'a>(
    server: &'a str,
    step: &'static str,
) -> impl FnOnce(serde_json::Error) -> McpStoreError + 'a {
    move |source| McpStoreError::Encoding {
        server: server.to_owned(),
        step,
        source,
    }
}

#[derive(Debug, thiserror::Error)]
pub enum McpSignInStartError {
    #[error("There is no MCP server {server}.")]
    UnknownServer { server: String },
    #[error("{server} runs as a program; only HTTP servers sign in.")]
    NotHttp { server: String },
    #[error("Couldn't check how {server} signs in")]
    Probe {
        server: String,
        #[source]
        source: ProbeError,
    },
    #[error("{server} answers without signing in.")]
    Open { server: String },
    #[error("{server} doesn't offer OAuth sign-in; it needs a token in an Authorization header.")]
    NeedsToken { server: String },
    #[error("Couldn't register Artisan to sign in to {server}")]
    Registration {
        server: String,
        #[source]
        source: RegistrationError,
    },
    #[error("{server} was removed or changed while signing in; nothing was saved.")]
    Removed { server: String },
    #[error("generating the secrets for signing in to {server}")]
    Random {
        server: String,
        #[source]
        source: getrandom::Error,
    },
    #[error(transparent)]
    Store(#[from] McpStoreError),
}

#[derive(Debug, thiserror::Error)]
pub enum McpSignInFinishError {
    #[error("That sign-in expired or was cancelled; start it again.")]
    Expired,
    #[error("{server} was signed out while signing in; nothing was saved.")]
    SignedOut { server: String },
    #[error("Couldn't sign in to {server}")]
    Exchange {
        server: String,
        #[source]
        source: TokenError,
    },
    #[error("{server} was removed or changed while signing in; nothing was saved.")]
    Removed { server: String },
    #[error(transparent)]
    Store(#[from] McpStoreError),
}

fn has_authorization(server: &McpServer) -> bool {
    server
        .headers
        .keys()
        .any(|name| name.eq_ignore_ascii_case("authorization"))
}

fn http_url(server: &McpServer) -> Option<&str> {
    server.url.as_deref().filter(|url| !url.trim().is_empty())
}

fn chain(error: &(dyn std::error::Error + 'static)) -> String {
    std::iter::successors(Some(error), |error| error.source())
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(": ")
}

pub struct Mcp {
    store: Store,
    flows: Mutex<Flows>,
    http: mcp_oauth::Http,
    writes: tokio::sync::Mutex<()>,
    pub(crate) unreachable: Mutex<HashSet<Endpoint>>,
}

impl Mcp {
    pub(crate) fn new(store: Store) -> Self {
        Self {
            store,
            flows: Mutex::default(),
            http: mcp_oauth::Http::new(),
            writes: tokio::sync::Mutex::new(()),
            unreachable: Mutex::default(),
        }
    }

    pub(crate) fn store(&self) -> &Store {
        &self.store
    }

    async fn servers(&self) -> Result<Vec<McpServer>, McpStoreError> {
        read_mcp_servers(self.store())
            .await
            .map_err(McpStoreError::Servers)
    }

    async fn server_named(&self, name: &str) -> Result<(McpServer, String), McpSignInStartError> {
        let server = self
            .servers()
            .await?
            .into_iter()
            .find(|server| server.name == name)
            .ok_or_else(|| McpSignInStartError::UnknownServer {
                server: name.to_owned(),
            })?;
        let url = http_url(&server)
            .ok_or_else(|| McpSignInStartError::NotHttp {
                server: name.to_owned(),
            })?
            .to_owned();
        Ok((server, url))
    }

    fn flows_lock(&self) -> std::sync::MutexGuard<'_, Flows> {
        self.flows
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn lifetime(&self, server: &str) -> Arc<tokio::sync::Mutex<Lifetime>> {
        Arc::clone(
            self.flows_lock()
                .lifetimes
                .entry(server.to_owned())
                .or_default(),
        )
    }

    #[cfg(test)]
    pub(crate) fn lifetime_holders(&self, server: &str) -> usize {
        self.flows_lock()
            .lifetimes
            .get(server)
            .map_or(0, Arc::strong_count)
    }

    pub async fn sign_in_start(
        &self,
        name: &str,
        redirect_uri: &str,
    ) -> Result<McpSignInStarted, McpSignInStartError> {
        let (server, url) = self.server_named(name).await?;
        let lifetime = self.lifetime(name);
        let lifetime = lifetime.lock().await;
        {
            let mut flows = self.flows_lock();
            flows.by_state.retain(|_, flow| flow.alive());
            if let Some((state, flow)) = flows.by_state.iter().find(|(_, flow)| {
                flow.server == name
                    && flow.url == url
                    && flow.signed_out == lifetime.signed_out
                    && flow.client.serves(&flow.discovery, redirect_uri)
            }) {
                return Ok(McpSignInStarted {
                    authorize_url: flow.authorize_url.clone(),
                    state: state.clone(),
                });
            }
        }
        let headers: Vec<(String, String)> = server.headers.clone().into_iter().collect();
        let probed = mcp_oauth::probe(&self.http, name, &url, &headers)
            .await
            .map_err(|source| McpSignInStartError::Probe {
                server: name.to_owned(),
                source,
            })?;
        let discovery = match probed {
            Probe::SignIn(discovery) => *discovery,
            Probe::Open => {
                self.record_auth(name, None).await?;
                return Err(McpSignInStartError::Open {
                    server: name.to_owned(),
                });
            }
            Probe::NeedsToken => {
                self.record_auth(name, Some(McpAuthStatus::NeedsToken))
                    .await?;
                return Err(McpSignInStartError::NeedsToken {
                    server: name.to_owned(),
                });
            }
        };
        let client = self.client(name, &url, &discovery, redirect_uri).await?;
        let pkce = Pkce::new().map_err(|source| McpSignInStartError::Random {
            server: name.to_owned(),
            source,
        })?;
        let authorize_url = mcp_oauth::authorize_url(&discovery, &client, &pkce);
        self.flows_lock().by_state.insert(
            pkce.state.clone(),
            Flow {
                server: name.to_owned(),
                url,
                signed_out: lifetime.signed_out,
                discovery,
                client,
                verifier: pkce.verifier,
                authorize_url: authorize_url.clone(),
                started: Instant::now(),
            },
        );
        self.record_auth(name, Some(McpAuthStatus::NeedsSignIn))
            .await?;
        Ok(McpSignInStarted {
            authorize_url,
            state: pkce.state,
        })
    }

    async fn client(
        &self,
        name: &str,
        url: &str,
        discovery: &Discovery,
        redirect_uri: &str,
    ) -> Result<Client, McpSignInStartError> {
        let cached = self
            .store()
            .secret(&McpSecret::Client.key(name))
            .await
            .map_err(read_secret_error(name, "reading Artisan's registration"))?;
        let reusable = match cached.map(|text| serde_json::from_str::<Client>(&text)) {
            Some(Ok(client)) => {
                Some(client).filter(|client| client.serves(discovery, redirect_uri))
            }
            Some(Err(unreadable)) => {
                tracing::info!(server = %name, "registering again over an unreadable registration: {unreadable}");
                None
            }
            None => None,
        };
        if let Some(client) = reusable {
            return Ok(client);
        }
        let client = mcp_oauth::register(&self.http, name, discovery, redirect_uri)
            .await
            .map_err(|source| McpSignInStartError::Registration {
                server: name.to_owned(),
                source,
            })?;
        let kept = self
            .store()
            .set_mcp_secret(
                name.to_owned(),
                url.to_owned(),
                McpSecret::Client,
                serde_json::to_string(&client)
                    .map_err(encoding_error(name, "encoding Artisan's registration"))?,
            )
            .await
            .map_err(save_secret_error(name, "saving Artisan's registration"))?;
        if !kept {
            return Err(McpSignInStartError::Removed {
                server: name.to_owned(),
            });
        }
        Ok(client)
    }

    pub async fn sign_in_finish(
        &self,
        state: &str,
        code: &str,
    ) -> Result<String, McpSignInFinishError> {
        let flow = self
            .flows_lock()
            .by_state
            .remove(state)
            .filter(Flow::alive)
            .ok_or(McpSignInFinishError::Expired)?;
        let lifetime = self.lifetime(&flow.server);
        let lifetime = lifetime.lock().await;
        if lifetime.signed_out != flow.signed_out {
            return Err(McpSignInFinishError::SignedOut {
                server: flow.server,
            });
        }
        let tokens = mcp_oauth::exchange(
            &self.http,
            &flow.server,
            &flow.discovery,
            &flow.client,
            code,
            flow.verifier.expose(),
            artisan_api::now(),
        )
        .await
        .map_err(|source| McpSignInFinishError::Exchange {
            server: flow.server.clone(),
            source,
        })?;
        let saved = Saved {
            url: flow.url,
            discovery: flow.discovery,
            client: flow.client,
            tokens,
        };
        if !self.keep_tokens(&flow.server, &saved).await? {
            return Err(McpSignInFinishError::Removed {
                server: flow.server,
            });
        }
        self.record_auth(&flow.server, Some(McpAuthStatus::SignedIn))
            .await?;
        drop(lifetime);
        Ok(flow.server)
    }

    pub fn sign_in_cancel(&self, state: &str) {
        self.flows_lock().by_state.remove(state);
    }

    pub async fn sign_out(&self, name: &str) -> Result<(), McpStoreError> {
        self.flows_lock()
            .by_state
            .retain(|_, flow| flow.server != name);
        let lifetime = self.lifetime(name);
        let mut lifetime = lifetime.lock().await;
        lifetime.signed_out += 1;
        self.flows_lock()
            .by_state
            .retain(|_, flow| flow.server != name);
        self.forget_tokens(name).await
    }

    async fn forget_tokens(&self, name: &str) -> Result<(), McpStoreError> {
        self.store()
            .set_secret(&McpSecret::Tokens.key(name), None)
            .await
            .map_err(forget_secret_error(name, "forgetting the tokens"))?;
        self.record_auth(name, Some(McpAuthStatus::NeedsSignIn))
            .await
    }

    async fn keep_tokens(&self, name: &str, saved: &Saved) -> Result<bool, McpStoreError> {
        self.store()
            .set_mcp_secret(
                name.to_owned(),
                saved.url.clone(),
                McpSecret::Tokens,
                serde_json::to_string(saved)
                    .map_err(encoding_error(name, "encoding the tokens"))?,
            )
            .await
            .map_err(save_secret_error(name, "saving the tokens"))
    }

    pub async fn check_auth(&self) {
        if let Err(error) = self.check_auth_inner().await {
            tracing::warn!("checking MCP sign-ins: {}", chain(&error));
        }
    }

    async fn check_auth_inner(&self) -> Result<(), McpStoreError> {
        let mut observed = Vec::new();
        for server in self.servers().await? {
            let Some(url) = http_url(&server).map(str::to_owned) else {
                continue;
            };
            if has_authorization(&server) || self.signed_in(&server.name, &url).await {
                continue;
            }
            let headers: Vec<(String, String)> = server.headers.clone().into_iter().collect();
            match mcp_oauth::probe(&self.http, &server.name, &url, &headers).await {
                Ok(probed) => observed.push((server.name, url, probed)),
                Err(error) => {
                    tracing::debug!(server = %server.name, "probing for sign-in: {}", chain(&error));
                }
            }
        }
        let _writes = self.writes.lock().await;
        let servers = self.servers().await?;
        let recorded = self.auth().await.map_err(McpStoreError::ReadStatuses)?;
        let mut statuses = recorded.clone();
        statuses.retain(|name, _| servers.iter().any(|server| &server.name == name));
        for server in &servers {
            let Some(url) = http_url(server) else {
                statuses.remove(&server.name);
                continue;
            };
            if has_authorization(server) {
                statuses.remove(&server.name);
                continue;
            }
            if self.signed_in(&server.name, url).await {
                statuses.insert(server.name.clone(), McpAuthStatus::SignedIn);
                continue;
            }
            let observation = observed
                .iter()
                .find(|(name, observed_url, _)| name == &server.name && observed_url == url);
            match observation.map(|(_, _, probed)| probed) {
                Some(Probe::Open) => {
                    statuses.remove(&server.name);
                }
                Some(Probe::SignIn(_)) => {
                    statuses.insert(server.name.clone(), McpAuthStatus::NeedsSignIn);
                }
                Some(Probe::NeedsToken) => {
                    statuses.insert(server.name.clone(), McpAuthStatus::NeedsToken);
                }
                None => {}
            }
        }
        if statuses == recorded {
            return Ok(());
        }
        self.store()
            .set_setting(
                MCP_AUTH_KEY.to_owned(),
                serde_json::to_value(&statuses).map_err(McpStoreError::EncodeStatuses)?,
            )
            .await
            .map_err(McpStoreError::RecordStatuses)
    }

    async fn signed_in(&self, name: &str, url: &str) -> bool {
        match self.saved(name, url).await {
            Ok(saved) => saved.is_some(),
            Err(error) => {
                tracing::warn!(server = %name, "reading saved MCP tokens: {}", chain(&error));
                false
            }
        }
    }

    pub async fn auth(&self) -> Result<BTreeMap<String, McpAuthStatus>, McpAuthError> {
        self.store()
            .setting(MCP_AUTH_KEY)
            .await
            .map_err(McpAuthError::Read)?
            .map(serde_json::from_value)
            .transpose()
            .map_err(McpAuthError::Decode)
            .map(Option::unwrap_or_default)
    }

    async fn record_auth(
        &self,
        name: &str,
        status: Option<McpAuthStatus>,
    ) -> Result<(), McpStoreError> {
        self.set_auth(name, status)
            .await
            .map_err(|source| McpStoreError::RecordAuth {
                server: name.to_owned(),
                source,
            })
    }

    async fn set_auth(
        &self,
        name: &str,
        status: Option<McpAuthStatus>,
    ) -> Result<(), SetAuthError> {
        let _guard = self.writes.lock().await;
        let mut statuses = self.auth().await.map_err(SetAuthError::Read)?;
        match status {
            Some(status) => statuses.insert(name.to_owned(), status),
            None => statuses.remove(name),
        };
        self.store()
            .set_setting(
                MCP_AUTH_KEY.to_owned(),
                serde_json::to_value(&statuses).map_err(SetAuthError::Encode)?,
            )
            .await
            .map_err(SetAuthError::Save)
    }

    async fn saved(&self, name: &str, url: &str) -> Result<Option<Saved>, McpStoreError> {
        let Some(text) = self
            .store()
            .secret(&McpSecret::Tokens.key(name))
            .await
            .map_err(read_secret_error(name, "reading the tokens"))?
        else {
            return Ok(None);
        };
        let saved: Saved =
            serde_json::from_str(&text).map_err(encoding_error(name, "decoding the tokens"))?;
        Ok(Some(saved).filter(|saved| saved.url == url))
    }

    pub(crate) async fn with_tokens(&self, mut servers: Vec<McpServer>) -> Vec<McpServer> {
        for server in &mut servers {
            let Some(url) = http_url(server).map(str::to_owned) else {
                continue;
            };
            if has_authorization(server) {
                continue;
            }
            match self.fresh_token(&server.name, &url).await {
                Ok(Some(token)) => {
                    server
                        .headers
                        .insert("Authorization".to_owned(), format!("Bearer {token}"));
                }
                Ok(None) => {}
                Err(error) => {
                    tracing::warn!(server = %server.name, "MCP token: {}", chain(&error));
                }
            }
        }
        servers
    }

    async fn fresh_token(&self, name: &str, url: &str) -> Result<Option<String>, McpStoreError> {
        match self.saved(name, url).await? {
            None => return Ok(None),
            Some(saved) if !saved.tokens.stale(artisan_api::now()) => {
                return Ok(Some(saved.tokens.access_token().expose().to_owned()));
            }
            Some(_) => {}
        }
        let lifetime = self.lifetime(name);
        let lifetime = lifetime.lock().await;
        let Some(mut saved) = self.saved(name, url).await? else {
            return Ok(None);
        };
        let now = artisan_api::now();
        if !saved.tokens.stale(now) {
            return Ok(Some(saved.tokens.access_token().expose().to_owned()));
        }
        let Some(refresh_token) = saved.tokens.refresh_token().cloned() else {
            self.forget_tokens(name).await?;
            return Ok(None);
        };
        let refreshed = mcp_oauth::refresh(
            &self.http,
            name,
            &saved.discovery,
            &saved.client,
            &refresh_token,
            now,
        )
        .await;
        let token = match refreshed {
            Ok(tokens) => {
                saved.tokens = tokens;
                self.keep_tokens(name, &saved)
                    .await?
                    .then(|| saved.tokens.access_token().expose().to_owned())
            }
            Err(TokenError::Revoked { revocation, .. }) => {
                tracing::info!(server = %name, ?revocation, "refresh refused, signing out");
                if matches!(
                    revocation,
                    Revocation::InvalidClient | Revocation::UnauthorizedClient
                ) {
                    self.store()
                        .set_secret(&McpSecret::Client.key(name), None)
                        .await
                        .map_err(forget_secret_error(
                            name,
                            "forgetting Artisan's registration",
                        ))?;
                }
                self.forget_tokens(name).await?;
                None
            }
            Err(error) => {
                tracing::warn!(server = %name, "refreshing an MCP token: {}", chain(&error));
                Some(saved.tokens.access_token().expose().to_owned())
            }
        };
        drop(lifetime);
        Ok(token)
    }
}
