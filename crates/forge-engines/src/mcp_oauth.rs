//! Signing in to HTTP MCP servers.
//!
//! As MCP's authorization spec lays out, the server answers 401 and points at its protected-resource metadata
//! (RFC 9728), which names an authorization server whose metadata
//! (RFC 8414) gives the endpoints. Artisan registers itself there (RFC 7591),
//! runs an authorization-code flow with PKCE bound to the server with
//! `resource` (RFC 8707), and refreshes the tokens it gets.
//!
//! Everything here is plain HTTP; Forge keeps the results and the Editor
//! catches the browser's redirect.

use anyhow::{Context, Result, anyhow, bail};
use base64::Engine as _;
use reqwest::{StatusCode, Url, header};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::time::Duration;

const TIMEOUT: Duration = Duration::from_secs(20);
/// The most a metadata document or token answer may be.
const MAX_BODY: usize = 256 * 1024;

/// What a server says when asked without a token.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Probe {
    /// It answered; it needs no sign-in.
    Open,
    /// 401, and it offers OAuth sign-in.
    SignIn(Box<Discovery>),
    /// 401, without OAuth: it needs a token in a header.
    NeedsToken,
}

/// Where and how to sign in to one server.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Discovery {
    /// The `resource` the tokens are bound to.
    pub resource: String,
    pub authorization_endpoint: String,
    pub token_endpoint: String,
    #[serde(default)]
    pub registration_endpoint: Option<String>,
    #[serde(default)]
    pub scope: Option<String>,
}

/// The client Artisan registered as, for one redirect address.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Client {
    pub client_id: String,
    #[serde(default)]
    pub client_secret: Option<String>,
    pub redirect_uri: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Tokens {
    pub access_token: String,
    #[serde(default)]
    pub refresh_token: Option<String>,
    /// Milliseconds since the epoch, when the server said.
    #[serde(default)]
    pub expires_at: Option<i64>,
}

impl Tokens {
    /// Expired, or expiring within a minute.
    #[must_use]
    pub fn stale(&self, now: i64) -> bool {
        self.expires_at.is_some_and(|at| at - 60_000 <= now)
    }
}

/// The PKCE secret, its challenge, and the `state` for one flow.
#[derive(Clone, Debug)]
pub struct Pkce {
    pub verifier: String,
    pub challenge: String,
    pub state: String,
}

impl Pkce {
    pub fn new() -> Result<Self> {
        let verifier = random(32)?;
        let challenge = b64(&Sha256::digest(verifier.as_bytes()));
        Ok(Self {
            verifier,
            challenge,
            state: random(24)?,
        })
    }
}

fn random(len: usize) -> Result<String> {
    let mut bytes = vec![0_u8; len];
    getrandom::fill(&mut bytes).map_err(|error| anyhow!("random bytes: {error}"))?;
    Ok(b64(&bytes))
}

fn b64(bytes: &[u8]) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

/// Asks the server to initialize without a token, and finds out how to sign
/// in when it refuses. `headers` are the ones the user configured.
pub async fn probe(
    http: &reqwest::Client,
    server_url: &str,
    headers: &[(String, String)],
) -> Result<Probe> {
    let mut request = http
        .post(server_url)
        .timeout(TIMEOUT)
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::ACCEPT, "application/json, text/event-stream")
        .body(
            json!({
                "jsonrpc": "2.0",
                "id": 0,
                "method": "initialize",
                "params": {
                    "protocolVersion": "2025-06-18",
                    "capabilities": {},
                    "clientInfo": { "name": "artisan", "version": env!("CARGO_PKG_VERSION") },
                },
            })
            .to_string(),
        );
    for (name, value) in headers {
        request = request.header(name.as_str(), value.as_str());
    }
    let response = request.send().await.context("reach the server")?;
    if response.status() != StatusCode::UNAUTHORIZED {
        return Ok(Probe::Open);
    }
    let challenge = response
        .headers()
        .get_all(header::WWW_AUTHENTICATE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .collect::<Vec<_>>()
        .join(", ");
    Ok(discover(http, server_url, &challenge)
        .await?
        .map_or(Probe::NeedsToken, |discovery| {
            Probe::SignIn(Box::new(discovery))
        }))
}

/// The sign-in endpoints of `server_url`, from its 401's `WWW-Authenticate`
/// challenge and the metadata it leads to. `None` when it offers no OAuth.
pub async fn discover(
    http: &reqwest::Client,
    server_url: &str,
    challenge: &str,
) -> Result<Option<Discovery>> {
    let server = Url::parse(server_url).context("the server's URL")?;
    let origin = server.origin().ascii_serialization();
    let path = server.path().trim_end_matches('/');
    let mut candidates = Vec::new();
    if let Some(url) = param(challenge, "resource_metadata") {
        candidates.push(url);
    }
    if !path.is_empty() {
        candidates.push(format!(
            "{origin}/.well-known/oauth-protected-resource{path}"
        ));
    }
    candidates.push(format!("{origin}/.well-known/oauth-protected-resource"));
    let mut resource_metadata = None;
    for candidate in candidates {
        if let Some(found) = fetch_json(http, &candidate).await {
            resource_metadata = Some(found);
            break;
        }
    }
    // Servers of the 2025-03-26 spec serve the authorization server's
    // metadata themselves.
    let (resource, issuer, scopes) = resource_metadata.as_ref().map_or_else(
        || (server_url.to_owned(), origin.clone(), None),
        |metadata| {
            (
                metadata["resource"]
                    .as_str()
                    .map_or_else(|| server_url.to_owned(), str::to_owned),
                metadata["authorization_servers"][0]
                    .as_str()
                    .map_or_else(|| origin.clone(), str::to_owned),
                metadata["scopes_supported"].as_array().map(|scopes| {
                    scopes
                        .iter()
                        .filter_map(Value::as_str)
                        .collect::<Vec<_>>()
                        .join(" ")
                }),
            )
        },
    );
    let Some(metadata) = authorization_metadata(http, &issuer).await else {
        return Ok(None);
    };
    let endpoint = |key: &str| metadata[key].as_str().map(str::to_owned);
    let (Some(authorization_endpoint), Some(token_endpoint)) = (
        endpoint("authorization_endpoint"),
        endpoint("token_endpoint"),
    ) else {
        return Ok(None);
    };
    if let Some(methods) = metadata["code_challenge_methods_supported"].as_array()
        && !methods.iter().any(|method| method == "S256")
    {
        bail!("its sign-in doesn't support PKCE with S256, which MCP requires");
    }
    let scope = param(challenge, "scope")
        .or(scopes)
        .filter(|scope| !scope.trim().is_empty());
    Ok(Some(Discovery {
        resource,
        authorization_endpoint,
        token_endpoint,
        registration_endpoint: endpoint("registration_endpoint"),
        scope,
    }))
}

/// RFC 8414's and OpenID's metadata locations for `issuer`, in the order
/// MCP's spec tries them.
async fn authorization_metadata(http: &reqwest::Client, issuer: &str) -> Option<Value> {
    let url = Url::parse(issuer).ok()?;
    let origin = url.origin().ascii_serialization();
    let path = url.path().trim_end_matches('/');
    let candidates = if path.is_empty() {
        vec![
            format!("{origin}/.well-known/oauth-authorization-server"),
            format!("{origin}/.well-known/openid-configuration"),
        ]
    } else {
        vec![
            format!("{origin}/.well-known/oauth-authorization-server{path}"),
            format!("{origin}/.well-known/openid-configuration{path}"),
            format!("{origin}{path}/.well-known/openid-configuration"),
        ]
    };
    for candidate in candidates {
        if let Some(found) = fetch_json(http, &candidate).await {
            return Some(found);
        }
    }
    None
}

async fn fetch_json(http: &reqwest::Client, url: &str) -> Option<Value> {
    let response = http
        .get(url)
        .timeout(TIMEOUT)
        .header(header::ACCEPT, "application/json")
        .send()
        .await
        .ok()?;
    if !response.status().is_success() {
        return None;
    }
    let bytes = response.bytes().await.ok()?;
    if bytes.len() > MAX_BODY {
        return None;
    }
    serde_json::from_slice::<Value>(&bytes)
        .ok()
        .filter(Value::is_object)
}

/// One `key="value"` (or `key=value`) parameter of a `WWW-Authenticate`
/// challenge.
fn param(challenge: &str, key: &str) -> Option<String> {
    let mut rest = challenge;
    while let Some(at) = rest.find(key) {
        let before = rest[..at].chars().next_back();
        let after = rest[at + key.len()..].trim_start();
        rest = &rest[at + key.len()..];
        if before.is_some_and(|c| c.is_alphanumeric() || c == '_') {
            continue;
        }
        let Some(value) = after.strip_prefix('=') else {
            continue;
        };
        let value = value.trim_start();
        return Some(value.strip_prefix('"').map_or_else(
            || {
                value
                    .split([',', ' '])
                    .next()
                    .unwrap_or_default()
                    .to_owned()
            },
            |quoted| quoted.split('"').next().unwrap_or_default().to_owned(),
        ));
    }
    None
}

/// Registers Artisan as a public client that comes back to `redirect_uri`.
pub async fn register(
    http: &reqwest::Client,
    discovery: &Discovery,
    redirect_uri: &str,
) -> Result<Client> {
    let endpoint = discovery.registration_endpoint.as_deref().context(
        "its sign-in doesn't let apps register themselves, so it needs a token in a header instead",
    )?;
    let body = json!({
        "client_name": "Artisan",
        "redirect_uris": [redirect_uri],
        "grant_types": ["authorization_code", "refresh_token"],
        "response_types": ["code"],
        "token_endpoint_auth_method": "none",
    });
    let response = http
        .post(endpoint)
        .timeout(TIMEOUT)
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::ACCEPT, "application/json")
        .body(body.to_string())
        .send()
        .await
        .context("reach the sign-in server")?;
    let answer = read(response, "registering Artisan").await?;
    Ok(Client {
        client_id: answer["client_id"]
            .as_str()
            .context("registering Artisan: no client_id in the answer")?
            .to_owned(),
        client_secret: answer["client_secret"].as_str().map(str::to_owned),
        redirect_uri: redirect_uri.to_owned(),
    })
}

/// The page the user approves the sign-in on.
pub fn authorize_url(discovery: &Discovery, client: &Client, pkce: &Pkce) -> Result<String> {
    let mut url =
        Url::parse(&discovery.authorization_endpoint).context("the authorization endpoint")?;
    {
        let mut query = url.query_pairs_mut();
        query
            .append_pair("response_type", "code")
            .append_pair("client_id", &client.client_id)
            .append_pair("redirect_uri", &client.redirect_uri)
            .append_pair("code_challenge", &pkce.challenge)
            .append_pair("code_challenge_method", "S256")
            .append_pair("state", &pkce.state)
            .append_pair("resource", &discovery.resource);
        if let Some(scope) = &discovery.scope {
            query.append_pair("scope", scope);
        }
    }
    Ok(url.into())
}

/// Trades the code the redirect brought back for tokens.
pub async fn exchange(
    http: &reqwest::Client,
    discovery: &Discovery,
    client: &Client,
    code: &str,
    verifier: &str,
    now: i64,
) -> Result<Tokens> {
    let mut form = vec![
        ("grant_type", "authorization_code"),
        ("code", code),
        ("redirect_uri", client.redirect_uri.as_str()),
        ("client_id", client.client_id.as_str()),
        ("code_verifier", verifier),
        ("resource", discovery.resource.as_str()),
    ];
    if let Some(secret) = &client.client_secret {
        form.push(("client_secret", secret));
    }
    tokens(http, discovery, &form, None, now).await
}

/// New tokens for a refresh token.
pub async fn refresh(
    http: &reqwest::Client,
    discovery: &Discovery,
    client: &Client,
    refresh_token: &str,
    now: i64,
) -> Result<Tokens> {
    let mut form = vec![
        ("grant_type", "refresh_token"),
        ("refresh_token", refresh_token),
        ("client_id", client.client_id.as_str()),
        ("resource", discovery.resource.as_str()),
    ];
    if let Some(secret) = &client.client_secret {
        form.push(("client_secret", secret));
    }
    tokens(http, discovery, &form, Some(refresh_token), now).await
}

async fn tokens(
    http: &reqwest::Client,
    discovery: &Discovery,
    form: &[(&str, &str)],
    previous_refresh: Option<&str>,
    now: i64,
) -> Result<Tokens> {
    let response = http
        .post(&discovery.token_endpoint)
        .timeout(TIMEOUT)
        .header(header::ACCEPT, "application/json")
        .form(form)
        .send()
        .await
        .context("reach the sign-in server")?;
    let answer = read(response, "getting a token").await?;
    let access_token = answer["access_token"]
        .as_str()
        .context("getting a token: no access_token in the answer")?
        .to_owned();
    Ok(Tokens {
        access_token,
        refresh_token: answer["refresh_token"]
            .as_str()
            .map(str::to_owned)
            .or_else(|| previous_refresh.map(str::to_owned)),
        expires_at: answer["expires_in"]
            .as_i64()
            .map(|seconds| now + seconds * 1000),
    })
}

/// The sign-in server answered and refused (as opposed to being out of
/// reach): a refresh refused this way means signing in again.
#[derive(Debug)]
pub struct Rejected(pub String);

impl std::fmt::Display for Rejected {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Rejected {}

/// A JSON answer, or the OAuth error it carries.
async fn read(response: reqwest::Response, doing: &str) -> Result<Value> {
    let status = response.status();
    let bytes = response.bytes().await.context("read the answer")?;
    if bytes.len() > MAX_BODY {
        bail!("{doing}: the answer is too large");
    }
    let value: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    if !status.is_success() {
        let error = value["error_description"]
            .as_str()
            .or_else(|| value["error"].as_str())
            .unwrap_or("no reason given");
        return Err(Rejected(format!("{doing}: HTTP {} ({error})", status.as_u16())).into());
    }
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn challenge_parameters() {
        let challenge = r#"Bearer error="invalid_token", resource_metadata="https://mcp.example.com/.well-known/oauth-protected-resource/mcp", scope="read write""#;
        assert_eq!(
            param(challenge, "resource_metadata").as_deref(),
            Some("https://mcp.example.com/.well-known/oauth-protected-resource/mcp")
        );
        assert_eq!(param(challenge, "scope").as_deref(), Some("read write"));
        assert_eq!(
            param("Bearer realm=x, scope=a", "scope").as_deref(),
            Some("a")
        );
        assert_eq!(param("Bearer xscope=a", "scope"), None);
        assert_eq!(param("Bearer", "scope"), None);
    }

    #[test]
    fn pkce_challenge_is_the_s256_of_the_verifier() {
        let pkce = Pkce::new().unwrap();
        assert_eq!(
            pkce.challenge,
            b64(&Sha256::digest(pkce.verifier.as_bytes()))
        );
        assert!(pkce.verifier.len() >= 43);
        assert_ne!(pkce.state, Pkce::new().unwrap().state);
    }

    #[test]
    fn authorize_url_carries_pkce_resource_and_scope() {
        let discovery = Discovery {
            resource: "https://mcp.example.com/mcp".into(),
            authorization_endpoint: "https://auth.example.com/authorize?x=1".into(),
            token_endpoint: "https://auth.example.com/token".into(),
            registration_endpoint: None,
            scope: Some("read".into()),
        };
        let client = Client {
            client_id: "abc".into(),
            client_secret: None,
            redirect_uri: "http://127.0.0.1:33418/callback".into(),
        };
        let pkce = Pkce::new().unwrap();
        let url = Url::parse(&authorize_url(&discovery, &client, &pkce).unwrap()).unwrap();
        let pairs: std::collections::HashMap<_, _> = url.query_pairs().into_owned().collect();
        assert_eq!(pairs["x"], "1");
        assert_eq!(pairs["response_type"], "code");
        assert_eq!(pairs["client_id"], "abc");
        assert_eq!(pairs["code_challenge_method"], "S256");
        assert_eq!(pairs["code_challenge"], pkce.challenge);
        assert_eq!(pairs["state"], pkce.state);
        assert_eq!(pairs["resource"], "https://mcp.example.com/mcp");
        assert_eq!(pairs["scope"], "read");
        assert_eq!(pairs["redirect_uri"], "http://127.0.0.1:33418/callback");
    }

    #[test]
    fn tokens_go_stale_a_minute_early() {
        let tokens = Tokens {
            access_token: "a".into(),
            refresh_token: None,
            expires_at: Some(100_000),
        };
        assert!(!tokens.stale(30_000));
        assert!(tokens.stale(40_000));
        assert!(
            !Tokens {
                expires_at: None,
                ..tokens
            }
            .stale(i64::MAX)
        );
    }
}
