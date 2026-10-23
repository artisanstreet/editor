//! Signing in to the user's HTTP MCP servers. Forge does the OAuth work
//! (`forge_engines::mcp_oauth`) and keeps the tokens as secrets; the Editor
//! opens the browser and catches the redirect. Engines get the token as an
//! `Authorization` header, so one sign-in covers every harness.
//!
//! The sign-in state of each server is the `mcp_auth` setting, which syncs
//! to Editors for the Extensions page.

use crate::Hub;
use anyhow::Result;
use artisan_api::{MCP_AUTH_KEY, McpAuthStatus, McpServer, McpSignInStarted};
use forge_engines::mcp_oauth::{self, Client, Discovery, Pkce, Probe, Tokens};
use forge_store::Refused;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::time::{Duration, Instant};

/// How long a started sign-in waits for its redirect.
const FLOW_LASTS: Duration = Duration::from_mins(10);

/// Sign-ins waiting for their redirect, by `state`.
#[derive(Default)]
pub struct Flows {
    by_state: HashMap<String, Flow>,
}

struct Flow {
    server: String,
    url: String,
    discovery: Discovery,
    client: Client,
    verifier: String,
    authorize_url: String,
    started: Instant,
}

/// What Forge keeps per signed-in server.
#[derive(Serialize, Deserialize)]
struct Saved {
    /// The server URL the tokens are for; a changed URL drops them.
    url: String,
    discovery: Discovery,
    client: Client,
    tokens: Tokens,
}

fn tokens_key(server: &str) -> String {
    format!("mcp_oauth:{server}")
}

/// The client Artisan registered as with a server, reused while the
/// Editor's redirect address stays the same.
fn client_key(server: &str) -> String {
    format!("mcp_oauth_client:{server}")
}

fn refused(message: impl Into<String>) -> anyhow::Error {
    anyhow::Error::new(Refused(message.into()))
}

/// The user set their own token.
fn has_authorization(server: &McpServer) -> bool {
    server
        .headers
        .keys()
        .any(|name| name.eq_ignore_ascii_case("authorization"))
}

impl Hub {
    async fn mcp_server_named(&self, name: &str) -> Result<(McpServer, String)> {
        let servers: Vec<McpServer> = self
            .store()
            .setting("mcp_servers")
            .await?
            .and_then(|value| serde_json::from_value(value).ok())
            .unwrap_or_default();
        let server = servers
            .into_iter()
            .find(|server| server.name == name)
            .ok_or_else(|| refused(format!("There is no MCP server {name}.")))?;
        let url = server
            .url
            .clone()
            .filter(|url| !url.trim().is_empty())
            .ok_or_else(|| {
                refused(format!(
                    "{name} runs as a program; only HTTP servers sign in."
                ))
            })?;
        Ok((server, url))
    }

    fn flows(&self) -> std::sync::MutexGuard<'_, Flows> {
        self.inner
            .mcp_flows
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Starts signing in `name`, coming back to `redirect_uri`. A sign-in
    /// already waiting for the same server and address is reused, so the
    /// card's button and an agent never race.
    pub async fn mcp_sign_in_start(
        &self,
        name: &str,
        redirect_uri: &str,
    ) -> Result<McpSignInStarted> {
        let (server, url) = self.mcp_server_named(name).await?;
        {
            let mut flows = self.flows();
            flows
                .by_state
                .retain(|_, flow| flow.started.elapsed() < FLOW_LASTS);
            if let Some((state, flow)) = flows.by_state.iter().find(|(_, flow)| {
                flow.server == name && flow.url == url && flow.client.redirect_uri == redirect_uri
            }) {
                return Ok(McpSignInStarted {
                    authorize_url: flow.authorize_url.clone(),
                    state: state.clone(),
                });
            }
        }
        let http = self.engines().http();
        let headers: Vec<(String, String)> = server.headers.clone().into_iter().collect();
        let probed = mcp_oauth::probe(http, &url, &headers)
            .await
            .map_err(|error| refused(format!("Couldn't reach {name}: {error:#}")))?;
        let discovery = match probed {
            Probe::SignIn(discovery) => *discovery,
            Probe::Open => {
                self.set_mcp_auth(name, None).await?;
                return Err(refused(format!("{name} answers without signing in.")));
            }
            Probe::NeedsToken => {
                self.set_mcp_auth(name, Some(McpAuthStatus::NeedsToken))
                    .await?;
                return Err(refused(format!(
                    "{name} doesn't offer OAuth sign-in; it needs a token in an Authorization header."
                )));
            }
        };
        let cached: Option<Client> = self
            .store()
            .secret(&client_key(name))
            .await?
            .and_then(|text| serde_json::from_str(&text).ok())
            .filter(|client: &Client| client.redirect_uri == redirect_uri);
        let client = if let Some(client) = cached {
            client
        } else {
            let client = mcp_oauth::register(http, &discovery, redirect_uri)
                .await
                .map_err(|error| refused(format!("Couldn't sign in to {name}: {error:#}")))?;
            self.store()
                .set_secret(&client_key(name), Some(serde_json::to_string(&client)?))
                .await?;
            client
        };
        let pkce = Pkce::new()?;
        let authorize_url = mcp_oauth::authorize_url(&discovery, &client, &pkce)?;
        self.flows().by_state.insert(
            pkce.state.clone(),
            Flow {
                server: name.to_owned(),
                url,
                discovery,
                client,
                verifier: pkce.verifier,
                authorize_url: authorize_url.clone(),
                started: Instant::now(),
            },
        );
        self.set_mcp_auth(name, Some(McpAuthStatus::NeedsSignIn))
            .await?;
        Ok(McpSignInStarted {
            authorize_url,
            state: pkce.state,
        })
    }

    /// Trades the code the redirect brought back for tokens and keeps them.
    /// Returns the server's name.
    pub async fn mcp_sign_in_finish(&self, state: &str, code: &str) -> Result<String> {
        let flow = self
            .flows()
            .by_state
            .remove(state)
            .filter(|flow| flow.started.elapsed() < FLOW_LASTS)
            .ok_or_else(|| refused("That sign-in expired or was cancelled; start it again."))?;
        let tokens = mcp_oauth::exchange(
            self.engines().http(),
            &flow.discovery,
            &flow.client,
            code,
            &flow.verifier,
            artisan_api::now(),
        )
        .await
        .map_err(|error| refused(format!("Couldn't sign in to {}: {error:#}", flow.server)))?;
        let saved = Saved {
            url: flow.url,
            discovery: flow.discovery,
            client: flow.client,
            tokens,
        };
        self.store()
            .set_secret(
                &tokens_key(&flow.server),
                Some(serde_json::to_string(&saved)?),
            )
            .await?;
        self.set_mcp_auth(&flow.server, Some(McpAuthStatus::SignedIn))
            .await?;
        Ok(flow.server)
    }

    /// Drops a waiting sign-in.
    pub fn mcp_sign_in_cancel(&self, state: &str) {
        self.flows().by_state.remove(state);
    }

    /// Forgets the tokens Artisan holds for `name`.
    pub async fn mcp_sign_out(&self, name: &str) -> Result<()> {
        self.store().set_secret(&tokens_key(name), None).await?;
        self.set_mcp_auth(name, Some(McpAuthStatus::NeedsSignIn))
            .await
    }

    /// Asks every HTTP server without a token whether it needs a sign-in,
    /// and records the answers. Unreachable servers are left as they were.
    pub async fn check_mcp_auth(&self) {
        if let Err(error) = self.check_mcp_auth_inner().await {
            tracing::warn!("checking MCP sign-ins: {error:#}");
        }
    }

    async fn check_mcp_auth_inner(&self) -> Result<()> {
        let servers: Vec<McpServer> = self
            .store()
            .setting("mcp_servers")
            .await?
            .and_then(|value| serde_json::from_value(value).ok())
            .unwrap_or_default();
        let mut statuses = self.mcp_auth().await?;
        statuses.retain(|name, _| servers.iter().any(|server| &server.name == name));
        for server in &servers {
            let Some(url) = server.url.as_ref().filter(|url| !url.trim().is_empty()) else {
                statuses.remove(&server.name);
                continue;
            };
            if has_authorization(server) {
                statuses.remove(&server.name);
                continue;
            }
            if self.saved(&server.name, url).await?.is_some() {
                statuses.insert(server.name.clone(), McpAuthStatus::SignedIn);
                continue;
            }
            let headers: Vec<(String, String)> = server.headers.clone().into_iter().collect();
            match mcp_oauth::probe(self.engines().http(), url, &headers).await {
                Ok(Probe::Open) => {
                    statuses.remove(&server.name);
                }
                Ok(Probe::SignIn(_)) => {
                    statuses.insert(server.name.clone(), McpAuthStatus::NeedsSignIn);
                }
                Ok(Probe::NeedsToken) => {
                    statuses.insert(server.name.clone(), McpAuthStatus::NeedsToken);
                }
                Err(error) => {
                    tracing::debug!(server = %server.name, "probing for sign-in: {error:#}");
                }
            }
        }
        self.store()
            .set_setting(MCP_AUTH_KEY.to_owned(), serde_json::to_value(&statuses)?)
            .await
    }

    /// The sign-in state of each server that has one.
    pub async fn mcp_auth(&self) -> Result<BTreeMap<String, McpAuthStatus>> {
        Ok(self
            .store()
            .setting(MCP_AUTH_KEY)
            .await?
            .and_then(|value| serde_json::from_value(value).ok())
            .unwrap_or_default())
    }

    async fn set_mcp_auth(&self, name: &str, status: Option<McpAuthStatus>) -> Result<()> {
        let _guard = self.inner.mcp_auth_writes.lock().await;
        let mut statuses = self.mcp_auth().await?;
        match status {
            Some(status) => statuses.insert(name.to_owned(), status),
            None => statuses.remove(name),
        };
        self.store()
            .set_setting(MCP_AUTH_KEY.to_owned(), serde_json::to_value(&statuses)?)
            .await
    }

    async fn saved(&self, name: &str, url: &str) -> Result<Option<Saved>> {
        Ok(self
            .store()
            .secret(&tokens_key(name))
            .await?
            .and_then(|text| serde_json::from_str::<Saved>(&text).ok())
            .filter(|saved| saved.url == url))
    }

    /// `servers` with the tokens Artisan holds as `Authorization` headers,
    /// refreshed when stale. A refresh the server refuses signs the server
    /// out, so its card asks again.
    pub(crate) async fn with_mcp_tokens(&self, mut servers: Vec<McpServer>) -> Vec<McpServer> {
        for server in &mut servers {
            let Some(url) = server.url.clone() else {
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
                    tracing::warn!(server = %server.name, "MCP token: {error:#}");
                }
            }
        }
        servers
    }

    async fn fresh_token(&self, name: &str, url: &str) -> Result<Option<String>> {
        let Some(mut saved) = self.saved(name, url).await? else {
            return Ok(None);
        };
        let now = artisan_api::now();
        if !saved.tokens.stale(now) {
            return Ok(Some(saved.tokens.access_token));
        }
        let Some(refresh_token) = saved.tokens.refresh_token.clone() else {
            self.mcp_sign_out(name).await?;
            return Ok(None);
        };
        match mcp_oauth::refresh(
            self.engines().http(),
            &saved.discovery,
            &saved.client,
            &refresh_token,
            now,
        )
        .await
        {
            Ok(tokens) => {
                saved.tokens = tokens;
                self.store()
                    .set_secret(&tokens_key(name), Some(serde_json::to_string(&saved)?))
                    .await?;
                Ok(Some(saved.tokens.access_token))
            }
            // Refused: the grant is gone, so the card asks again.
            Err(error) if error.downcast_ref::<mcp_oauth::Rejected>().is_some() => {
                tracing::info!(server = %name, "refresh refused, signing out: {error:#}");
                self.mcp_sign_out(name).await?;
                Ok(None)
            }
            // Out of reach: try the old token; the next launch refreshes again.
            Err(error) => {
                tracing::warn!(server = %name, "refreshing an MCP token: {error:#}");
                Ok(Some(saved.tokens.access_token))
            }
        }
    }
}
