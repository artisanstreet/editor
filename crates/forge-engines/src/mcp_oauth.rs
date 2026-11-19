use artisan_api::Secret;
use base64::Engine as _;
use reqwest::{StatusCode, Url, header};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::time::Duration;

const TIMEOUT: Duration = Duration::from_secs(20);
const MAX_BODY: usize = 256 * 1024;
const MAX_REDIRECTS: usize = 5;
const MAX_EXPIRES_IN_SECONDS: u64 = 10 * 365 * 24 * 60 * 60;

#[derive(Clone, Debug)]
pub struct Http(reqwest::Client);

impl Http {
    #[must_use]
    pub fn new() -> Self {
        let policy = reqwest::redirect::Policy::custom(|attempt| {
            if attempt.previous().len() >= MAX_REDIRECTS || !is_secure(attempt.url()) {
                attempt.stop()
            } else {
                attempt.follow()
            }
        });
        Self(
            reqwest::Client::builder()
                .user_agent(concat!("artisan-forge/", env!("CARGO_PKG_VERSION")))
                .connect_timeout(Duration::from_secs(15))
                .redirect(policy)
                .build()
                .expect("invariant: the OAuth HTTP client builds with the default TLS backend"),
        )
    }
}

impl Default for Http {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct SecureUrl(Box<Url>);

impl SecureUrl {
    #[must_use]
    const fn url(&self) -> &Url {
        &self.0
    }

    #[must_use]
    fn as_str(&self) -> &str {
        self.0.as_str()
    }
}

impl std::fmt::Display for SecureUrl {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.0.as_str())
    }
}

#[derive(Debug, thiserror::Error)]
pub enum InsecureUrl {
    #[error("{value} isn't a URL")]
    Invalid {
        value: String,
        #[source]
        source: url::ParseError,
    },
    #[error("{url} isn't HTTPS")]
    NotHttps { url: Box<Url> },
}

impl TryFrom<String> for SecureUrl {
    type Error = InsecureUrl;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        let url = Url::parse(&value).map_err(|source| InsecureUrl::Invalid { value, source })?;
        Self::try_from(url)
    }
}

impl TryFrom<Url> for SecureUrl {
    type Error = InsecureUrl;

    fn try_from(url: Url) -> Result<Self, Self::Error> {
        if is_secure(&url) {
            Ok(Self(Box::new(url)))
        } else {
            Err(InsecureUrl::NotHttps { url: Box::new(url) })
        }
    }
}

impl From<SecureUrl> for String {
    fn from(url: SecureUrl) -> Self {
        (*url.0).into()
    }
}

fn is_secure(url: &Url) -> bool {
    match url.scheme() {
        "https" => true,
        "http" => match url.host() {
            Some(url::Host::Ipv4(ip)) => ip.is_loopback(),
            Some(url::Host::Ipv6(ip)) => ip.is_loopback(),
            Some(url::Host::Domain(domain)) => domain.eq_ignore_ascii_case("localhost"),
            None => false,
        },
        _ => false,
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Probe {
    Open,
    SignIn(Box<Discovery>),
    NeedsToken,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Discovery {
    issuer: SecureUrl,
    resource: SecureUrl,
    authorization_endpoint: SecureUrl,
    token_endpoint: SecureUrl,
    registration_endpoint: Option<SecureUrl>,
    scope: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Client {
    #[serde(rename = "client_id")]
    id: String,
    #[serde(rename = "client_secret")]
    secret: Option<Secret>,
    redirect_uri: String,
    issuer: SecureUrl,
    resource: SecureUrl,
}

impl Client {
    #[must_use]
    pub fn serves(&self, discovery: &Discovery, redirect_uri: &str) -> bool {
        self.redirect_uri == redirect_uri
            && self.issuer == discovery.issuer
            && self.resource == discovery.resource
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Tokens {
    access_token: Secret,
    refresh_token: Option<Secret>,
    expires_at: Option<i64>,
}

impl Tokens {
    #[must_use]
    pub fn stale(&self, now: i64) -> bool {
        self.expires_at
            .is_some_and(|at| at.saturating_sub(60_000) <= now)
    }

    #[must_use]
    pub const fn access_token(&self) -> &Secret {
        &self.access_token
    }

    #[must_use]
    pub const fn refresh_token(&self) -> Option<&Secret> {
        self.refresh_token.as_ref()
    }
}

#[derive(Clone, Debug)]
pub struct Pkce {
    pub verifier: Secret,
    pub challenge: String,
    pub state: String,
}

impl Pkce {
    pub fn new() -> Result<Self, getrandom::Error> {
        let verifier = Secret::new(random(32)?);
        Ok(Self {
            verifier: verifier.clone(),
            challenge: s256_challenge(verifier.expose()),
            state: random(24)?,
        })
    }
}

fn s256_challenge(verifier: &str) -> String {
    b64(&Sha256::digest(verifier.as_bytes()))
}

fn random(len: usize) -> Result<String, getrandom::Error> {
    let mut bytes = vec![0_u8; len];
    getrandom::fill(&mut bytes)?;
    Ok(b64(&bytes))
}

fn b64(bytes: &[u8]) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

#[derive(Debug, thiserror::Error)]
pub enum ResponseError {
    #[error("couldn't reach {url}")]
    Unreachable {
        url: Box<Url>,
        #[source]
        source: reqwest::Error,
    },
    #[error("{url} redirected to {location}, which isn't HTTPS")]
    InsecureRedirect { url: Box<Url>, location: Box<Url> },
    #[error("reading the answer from {url}")]
    Read {
        url: Box<Url>,
        #[source]
        source: reqwest::Error,
    },
    #[error("the answer from {url} is over {limit} bytes")]
    TooLarge { url: Box<Url>, limit: usize },
    #[error("{url} answered HTTP {status}{}", oauth_reason(*.body))]
    Status {
        url: Box<Url>,
        status: u16,
        body: ErrorBody,
    },
    #[error("the answer from {url} isn't what OAuth expects")]
    Malformed {
        url: Box<Url>,
        #[source]
        source: serde_json::Error,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ErrorBody {
    OAuth(OAuthCode),
    Other,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OAuthCode {
    InvalidRequest,
    InvalidClient,
    InvalidGrant,
    UnauthorizedClient,
    UnsupportedGrantType,
    InvalidScope,
    InvalidRedirectUri,
    InvalidClientMetadata,
    InvalidSoftwareStatement,
    UnapprovedSoftwareStatement,
    InvalidToken,
    InsufficientScope,
    AccessDenied,
    ServerError,
    TemporarilyUnavailable,
    Unrecognized,
}

impl OAuthCode {
    fn of(code: &str) -> Self {
        match code {
            "invalid_request" => Self::InvalidRequest,
            "invalid_client" => Self::InvalidClient,
            "invalid_grant" => Self::InvalidGrant,
            "unauthorized_client" => Self::UnauthorizedClient,
            "unsupported_grant_type" => Self::UnsupportedGrantType,
            "invalid_scope" => Self::InvalidScope,
            "invalid_redirect_uri" => Self::InvalidRedirectUri,
            "invalid_client_metadata" => Self::InvalidClientMetadata,
            "invalid_software_statement" => Self::InvalidSoftwareStatement,
            "unapproved_software_statement" => Self::UnapprovedSoftwareStatement,
            "invalid_token" => Self::InvalidToken,
            "insufficient_scope" => Self::InsufficientScope,
            "access_denied" => Self::AccessDenied,
            "server_error" => Self::ServerError,
            "temporarily_unavailable" => Self::TemporarilyUnavailable,
            _ => Self::Unrecognized,
        }
    }

    const fn code(self) -> &'static str {
        match self {
            Self::InvalidRequest => "invalid_request",
            Self::InvalidClient => "invalid_client",
            Self::InvalidGrant => "invalid_grant",
            Self::UnauthorizedClient => "unauthorized_client",
            Self::UnsupportedGrantType => "unsupported_grant_type",
            Self::InvalidScope => "invalid_scope",
            Self::InvalidRedirectUri => "invalid_redirect_uri",
            Self::InvalidClientMetadata => "invalid_client_metadata",
            Self::InvalidSoftwareStatement => "invalid_software_statement",
            Self::UnapprovedSoftwareStatement => "unapproved_software_statement",
            Self::InvalidToken => "invalid_token",
            Self::InsufficientScope => "insufficient_scope",
            Self::AccessDenied => "access_denied",
            Self::ServerError => "server_error",
            Self::TemporarilyUnavailable => "temporarily_unavailable",
            Self::Unrecognized => "an OAuth error code Artisan doesn't know",
        }
    }
}

fn oauth_reason(body: ErrorBody) -> String {
    match body {
        ErrorBody::OAuth(code) => format!(" ({})", code.code()),
        ErrorBody::Other => String::new(),
    }
}

#[derive(Deserialize)]
struct OAuthErrorAnswer {
    error: String,
}

impl ErrorBody {
    fn of(bytes: &[u8]) -> Self {
        match serde_json::from_slice::<OAuthErrorAnswer>(bytes) {
            Ok(answer) => Self::OAuth(OAuthCode::of(&answer.error)),
            Err(not_oauth) => {
                tracing::debug!(
                    line = not_oauth.line(),
                    column = not_oauth.column(),
                    "an error answer without an OAuth error"
                );
                Self::Other
            }
        }
    }
}

struct Answer {
    url: Box<Url>,
    status: StatusCode,
    body: Vec<u8>,
}

impl Answer {
    fn success<T: serde::de::DeserializeOwned>(self) -> Result<T, ResponseError> {
        if !self.status.is_success() {
            return Err(ResponseError::Status {
                status: self.status.as_u16(),
                body: ErrorBody::of(&self.body),
                url: self.url,
            });
        }
        serde_json::from_slice(&self.body).map_err(|source| ResponseError::Malformed {
            url: self.url,
            source,
        })
    }
}

async fn send(request: reqwest::RequestBuilder, url: &Url) -> Result<Answer, ResponseError> {
    let mut response =
        request
            .timeout(TIMEOUT)
            .send()
            .await
            .map_err(|source| ResponseError::Unreachable {
                url: Box::new(url.clone()),
                source,
            })?;
    if response.status().is_redirection() {
        let location = response
            .headers()
            .get(header::LOCATION)
            .and_then(|location| location.to_str().ok())
            .and_then(|location| response.url().join(location).ok());
        if let Some(location) = location.filter(|location| !is_secure(location)) {
            return Err(ResponseError::InsecureRedirect {
                url: Box::new(url.clone()),
                location: Box::new(location),
            });
        }
    }
    let status = response.status();
    if response
        .content_length()
        .is_some_and(|length| length > MAX_BODY as u64)
    {
        return Err(ResponseError::TooLarge {
            url: Box::new(url.clone()),
            limit: MAX_BODY,
        });
    }
    let mut body = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|source| ResponseError::Read {
            url: Box::new(url.clone()),
            source,
        })?
    {
        if body.len() + chunk.len() > MAX_BODY {
            return Err(ResponseError::TooLarge {
                url: Box::new(url.clone()),
                limit: MAX_BODY,
            });
        }
        body.extend_from_slice(&chunk);
    }
    Ok(Answer {
        url: Box::new(url.clone()),
        status,
        body,
    })
}

#[derive(Debug, thiserror::Error)]
pub enum ProbeError {
    #[error("MCP server {server}'s address {url} isn't a URL")]
    ServerUrl {
        server: String,
        url: String,
        #[source]
        source: url::ParseError,
    },
    #[error("couldn't reach MCP server {server}")]
    Unreachable {
        server: String,
        #[source]
        source: reqwest::Error,
    },
    #[error(
        "MCP server {server} answered HTTP {status}, so it can't be told whether it needs a sign-in"
    )]
    Unavailable { server: String, status: u16 },
    #[error(transparent)]
    Discovery(#[from] DiscoveryError),
}

pub async fn probe(
    http: &Http,
    server: &str,
    server_url: &str,
    headers: &[(String, String)],
) -> Result<Probe, ProbeError> {
    let url = Url::parse(server_url).map_err(|source| ProbeError::ServerUrl {
        server: server.to_owned(),
        url: server_url.to_owned(),
        source,
    })?;
    let mut request = http
        .0
        .post(url.clone())
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
    let response = request
        .send()
        .await
        .map_err(|source| ProbeError::Unreachable {
            server: server.to_owned(),
            source,
        })?;
    let status = response.status();
    if status.is_success() {
        return Ok(Probe::Open);
    }
    if status != StatusCode::UNAUTHORIZED {
        return Err(ProbeError::Unavailable {
            server: server.to_owned(),
            status: status.as_u16(),
        });
    }
    let challenge = response
        .headers()
        .get_all(header::WWW_AUTHENTICATE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .collect::<Vec<_>>()
        .join(", ");
    Ok(discover(http, server, &url, &challenge)
        .await?
        .map_or(Probe::NeedsToken, |discovery| {
            Probe::SignIn(Box::new(discovery))
        }))
}

#[derive(Debug, thiserror::Error)]
pub enum DiscoveryError {
    #[error("MCP server {server} isn't served over HTTPS, so Artisan won't sign in to it")]
    InsecureServer {
        server: String,
        #[source]
        source: InsecureUrl,
    },
    #[error("fetching MCP server {server}'s sign-in metadata")]
    Fetch {
        server: String,
        #[source]
        source: ResponseError,
    },
    #[error("MCP server {server}'s metadata names an unusable {field}")]
    Endpoint {
        server: String,
        field: &'static str,
        #[source]
        source: InsecureUrl,
    },
    #[error("MCP server {server}'s metadata is for {resource}, not {server_url}")]
    ForeignResource {
        server: String,
        server_url: Box<Url>,
        resource: Box<Url>,
    },
    #[error("authorization server {issuer} for MCP server {server} calls itself {claimed}")]
    IssuerMismatch {
        server: String,
        issuer: SecureUrl,
        claimed: String,
    },
    #[error(
        "authorization server {issuer} for MCP server {server} doesn't support PKCE with S256, which MCP requires"
    )]
    NoPkce { server: String, issuer: SecureUrl },
}

#[derive(Deserialize)]
struct ResourceMetadata {
    resource: Option<String>,
    #[serde(default)]
    authorization_servers: Vec<String>,
    scopes_supported: Option<Vec<String>>,
}

#[derive(Deserialize)]
struct AuthorizationMetadata {
    issuer: String,
    authorization_endpoint: String,
    token_endpoint: String,
    registration_endpoint: Option<String>,
    code_challenge_methods_supported: Option<Vec<String>>,
}

async fn discover(
    http: &Http,
    server: &str,
    server_url: &Url,
    challenge: &str,
) -> Result<Option<Discovery>, DiscoveryError> {
    let secure_server = SecureUrl::try_from(server_url.clone()).map_err(|source| {
        DiscoveryError::InsecureServer {
            server: server.to_owned(),
            source,
        }
    })?;
    let origin = server_url.origin().ascii_serialization();
    let path = server_url.path().trim_end_matches('/');
    let endpoint = |field: &'static str, value: String| {
        SecureUrl::try_from(value).map_err(|source| DiscoveryError::Endpoint {
            server: server.to_owned(),
            field,
            source,
        })
    };
    let mut candidates = Vec::new();
    if let Some(url) = param(challenge, "resource_metadata") {
        candidates.push(endpoint("resource_metadata", url)?);
    }
    if !path.is_empty() {
        candidates.push(endpoint(
            "resource_metadata",
            format!("{origin}/.well-known/oauth-protected-resource{path}"),
        )?);
    }
    candidates.push(endpoint(
        "resource_metadata",
        format!("{origin}/.well-known/oauth-protected-resource"),
    )?);
    let resource_metadata: Option<ResourceMetadata> =
        first_document(http, server, &candidates).await?;
    let (resource, issuer, scopes) = match resource_metadata {
        Some(metadata) => (
            match metadata.resource {
                Some(resource) => endpoint("resource", resource)?,
                None => secure_server.clone(),
            },
            match metadata.authorization_servers.into_iter().next() {
                Some(issuer) => endpoint("authorization_servers", issuer)?,
                None => endpoint("issuer", origin.clone())?,
            },
            metadata.scopes_supported.map(|scopes| scopes.join(" ")),
        ),
        None => (secure_server.clone(), endpoint("issuer", origin)?, None),
    };
    if !covers(resource.url(), server_url) {
        return Err(DiscoveryError::ForeignResource {
            server: server.to_owned(),
            server_url: Box::new(server_url.clone()),
            resource: Box::new(resource.url().clone()),
        });
    }
    let Some(metadata) = first_document::<AuthorizationMetadata>(
        http,
        server,
        &authorization_metadata_urls(&issuer).map_err(|source| DiscoveryError::Endpoint {
            server: server.to_owned(),
            field: "issuer",
            source,
        })?,
    )
    .await?
    else {
        return Ok(None);
    };
    if !same_issuer(&metadata.issuer, &issuer) {
        return Err(DiscoveryError::IssuerMismatch {
            server: server.to_owned(),
            issuer,
            claimed: metadata.issuer,
        });
    }
    if let Some(methods) = &metadata.code_challenge_methods_supported
        && !methods.iter().any(|method| method == "S256")
    {
        return Err(DiscoveryError::NoPkce {
            server: server.to_owned(),
            issuer,
        });
    }
    let scope = param(challenge, "scope")
        .or(scopes)
        .filter(|scope| !scope.trim().is_empty());
    Ok(Some(Discovery {
        issuer,
        resource,
        authorization_endpoint: endpoint(
            "authorization_endpoint",
            metadata.authorization_endpoint,
        )?,
        token_endpoint: endpoint("token_endpoint", metadata.token_endpoint)?,
        registration_endpoint: metadata
            .registration_endpoint
            .map(|registration| endpoint("registration_endpoint", registration))
            .transpose()?,
        scope,
    }))
}

fn covers(resource: &Url, server: &Url) -> bool {
    let resource_path = resource.path().trim_end_matches('/');
    let server_path = server.path().trim_end_matches('/');
    resource.origin() == server.origin()
        && server_path
            .strip_prefix(resource_path)
            .is_some_and(|rest| rest.is_empty() || rest.starts_with('/'))
}

fn same_issuer(claimed: &str, issuer: &SecureUrl) -> bool {
    claimed.trim_end_matches('/') == issuer.as_str().trim_end_matches('/')
}

fn authorization_metadata_urls(issuer: &SecureUrl) -> Result<Vec<SecureUrl>, InsecureUrl> {
    let origin = issuer.url().origin().ascii_serialization();
    let path = issuer.url().path().trim_end_matches('/');
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
    candidates.into_iter().map(SecureUrl::try_from).collect()
}

async fn first_document<T: serde::de::DeserializeOwned>(
    http: &Http,
    server: &str,
    candidates: &[SecureUrl],
) -> Result<Option<T>, DiscoveryError> {
    for candidate in candidates {
        let answer = send(
            http.0
                .get(candidate.url().clone())
                .header(header::ACCEPT, "application/json"),
            candidate.url(),
        )
        .await
        .map_err(|source| DiscoveryError::Fetch {
            server: server.to_owned(),
            source,
        })?;
        if answer.status.is_server_error() {
            return Err(DiscoveryError::Fetch {
                server: server.to_owned(),
                source: ResponseError::Status {
                    status: answer.status.as_u16(),
                    body: ErrorBody::of(&answer.body),
                    url: answer.url,
                },
            });
        }
        if !answer.status.is_success() {
            continue;
        }
        match serde_json::from_slice::<T>(&answer.body) {
            Ok(document) => return Ok(Some(document)),
            Err(not_metadata) => {
                tracing::debug!(
                    server,
                    url = %candidate,
                    category = ?not_metadata.classify(),
                    line = not_metadata.line(),
                    column = not_metadata.column(),
                    "not sign-in metadata"
                );
            }
        }
    }
    Ok(None)
}

fn param(challenge: &str, key: &str) -> Option<String> {
    let mut rest = challenge;
    while let Some(at) = rest.find(key) {
        let (before, after) = rest.split_at(at);
        let after = after.get(key.len()..).unwrap_or_default();
        rest = after;
        if before
            .chars()
            .next_back()
            .is_some_and(|c| c.is_alphanumeric() || c == '_')
        {
            continue;
        }
        let Some(value) = after.trim_start().strip_prefix('=') else {
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

#[derive(Debug, thiserror::Error)]
pub enum RegistrationError {
    #[error(
        "authorization server {issuer} for MCP server {server} doesn't let apps register themselves, so it needs a token in a header instead"
    )]
    Unsupported { server: String, issuer: SecureUrl },
    #[error("registering Artisan with authorization server {issuer} for MCP server {server}")]
    Request {
        server: String,
        issuer: SecureUrl,
        #[source]
        source: ResponseError,
    },
    #[error(
        "authorization server {issuer} for MCP server {server} registered Artisan without a client_id"
    )]
    NoClientId { server: String, issuer: SecureUrl },
}

#[derive(Deserialize)]
struct RegistrationAnswer {
    client_id: String,
    client_secret: Option<String>,
}

pub async fn register(
    http: &Http,
    server: &str,
    discovery: &Discovery,
    redirect_uri: &str,
) -> Result<Client, RegistrationError> {
    let endpoint =
        discovery
            .registration_endpoint
            .as_ref()
            .ok_or_else(|| RegistrationError::Unsupported {
                server: server.to_owned(),
                issuer: discovery.issuer.clone(),
            })?;
    let body = json!({
        "client_name": "Artisan",
        "redirect_uris": [redirect_uri],
        "grant_types": ["authorization_code", "refresh_token"],
        "response_types": ["code"],
        "token_endpoint_auth_method": "none",
    });
    let answer: RegistrationAnswer = send(
        http.0
            .post(endpoint.url().clone())
            .header(header::CONTENT_TYPE, "application/json")
            .header(header::ACCEPT, "application/json")
            .body(body.to_string()),
        endpoint.url(),
    )
    .await
    .and_then(Answer::success)
    .map_err(|source| RegistrationError::Request {
        server: server.to_owned(),
        issuer: discovery.issuer.clone(),
        source,
    })?;
    if answer.client_id.trim().is_empty() {
        return Err(RegistrationError::NoClientId {
            server: server.to_owned(),
            issuer: discovery.issuer.clone(),
        });
    }
    Ok(Client {
        id: answer.client_id,
        secret: answer
            .client_secret
            .filter(|secret| !secret.is_empty())
            .map(Secret::new),
        redirect_uri: redirect_uri.to_owned(),
        issuer: discovery.issuer.clone(),
        resource: discovery.resource.clone(),
    })
}

#[must_use]
pub fn authorize_url(discovery: &Discovery, client: &Client, pkce: &Pkce) -> String {
    let mut url = discovery.authorization_endpoint.url().clone();
    {
        let mut query = url.query_pairs_mut();
        query
            .append_pair("response_type", "code")
            .append_pair("client_id", &client.id)
            .append_pair("redirect_uri", &client.redirect_uri)
            .append_pair("code_challenge", &pkce.challenge)
            .append_pair("code_challenge_method", "S256")
            .append_pair("state", &pkce.state)
            .append_pair("resource", discovery.resource.as_str());
        if let Some(scope) = &discovery.scope {
            query.append_pair("scope", scope);
        }
    }
    url.into()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Revocation {
    InvalidGrant,
    InvalidClient,
    UnauthorizedClient,
}

impl Revocation {
    const fn of(code: OAuthCode) -> Option<Self> {
        match code {
            OAuthCode::InvalidGrant => Some(Self::InvalidGrant),
            OAuthCode::InvalidClient => Some(Self::InvalidClient),
            OAuthCode::UnauthorizedClient => Some(Self::UnauthorizedClient),
            OAuthCode::InvalidRequest
            | OAuthCode::UnsupportedGrantType
            | OAuthCode::InvalidScope
            | OAuthCode::InvalidRedirectUri
            | OAuthCode::InvalidClientMetadata
            | OAuthCode::InvalidSoftwareStatement
            | OAuthCode::UnapprovedSoftwareStatement
            | OAuthCode::InvalidToken
            | OAuthCode::InsufficientScope
            | OAuthCode::AccessDenied
            | OAuthCode::ServerError
            | OAuthCode::TemporarilyUnavailable
            | OAuthCode::Unrecognized => None,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum TokenError {
    #[error(
        "authorization server {issuer} no longer accepts Artisan's sign-in to MCP server {server} ({revocation:?})"
    )]
    Revoked {
        server: String,
        issuer: SecureUrl,
        revocation: Revocation,
    },
    #[error("getting a token for MCP server {server} from authorization server {issuer}")]
    Request {
        server: String,
        issuer: SecureUrl,
        #[source]
        source: ResponseError,
    },
    #[error("authorization server {issuer} gave MCP server {server} an empty access token")]
    EmptyAccessToken { server: String, issuer: SecureUrl },
    #[error(
        "authorization server {issuer} gave MCP server {server} a {token_type} token, which Artisan can't send"
    )]
    UnsupportedTokenType {
        server: String,
        issuer: SecureUrl,
        token_type: String,
    },
    #[error(
        "authorization server {issuer} gave MCP server {server} a token expiring in {expires_in}"
    )]
    InvalidExpiry {
        server: String,
        issuer: SecureUrl,
        expires_in: String,
    },
}

#[derive(Deserialize)]
#[serde(untagged)]
enum ExpiresIn {
    Number(serde_json::Number),
    Text(String),
}

impl ExpiresIn {
    fn seconds(&self) -> Option<u64> {
        match self {
            Self::Number(number) => number.as_u64(),
            Self::Text(text) => text.trim().parse().ok(),
        }
        .filter(|seconds| *seconds <= MAX_EXPIRES_IN_SECONDS)
    }

    fn text(&self) -> String {
        match self {
            Self::Number(number) => number.to_string(),
            Self::Text(text) => text.clone(),
        }
    }
}

#[derive(Deserialize)]
struct TokenAnswer {
    access_token: String,
    token_type: Option<String>,
    refresh_token: Option<String>,
    expires_in: Option<ExpiresIn>,
}

pub async fn exchange(
    http: &Http,
    server: &str,
    discovery: &Discovery,
    client: &Client,
    code: &str,
    verifier: &str,
    now: i64,
) -> Result<Tokens, TokenError> {
    let mut form = vec![
        ("grant_type", "authorization_code"),
        ("code", code),
        ("redirect_uri", client.redirect_uri.as_str()),
        ("client_id", client.id.as_str()),
        ("code_verifier", verifier),
        ("resource", discovery.resource.as_str()),
    ];
    if let Some(secret) = &client.secret {
        form.push(("client_secret", secret.expose()));
    }
    tokens(http, server, discovery, &form, None, now).await
}

pub async fn refresh(
    http: &Http,
    server: &str,
    discovery: &Discovery,
    client: &Client,
    refresh_token: &Secret,
    now: i64,
) -> Result<Tokens, TokenError> {
    let mut form = vec![
        ("grant_type", "refresh_token"),
        ("refresh_token", refresh_token.expose()),
        ("client_id", client.id.as_str()),
        ("resource", discovery.resource.as_str()),
    ];
    if let Some(secret) = &client.secret {
        form.push(("client_secret", secret.expose()));
    }
    tokens(http, server, discovery, &form, Some(refresh_token), now).await
}

async fn tokens(
    http: &Http,
    server: &str,
    discovery: &Discovery,
    form: &[(&str, &str)],
    previous_refresh: Option<&Secret>,
    now: i64,
) -> Result<Tokens, TokenError> {
    let issuer = || discovery.issuer.clone();
    let endpoint = discovery.token_endpoint.url();
    let answer = send(
        http.0
            .post(endpoint.clone())
            .header(header::ACCEPT, "application/json")
            .form(form),
        endpoint,
    )
    .await
    .and_then(Answer::success::<TokenAnswer>)
    .map_err(|source| match &source {
        ResponseError::Status {
            status: 400 | 401,
            body: ErrorBody::OAuth(code),
            ..
        } => Revocation::of(*code).map_or_else(
            || TokenError::Request {
                server: server.to_owned(),
                issuer: issuer(),
                source,
            },
            |revocation| TokenError::Revoked {
                server: server.to_owned(),
                issuer: issuer(),
                revocation,
            },
        ),
        ResponseError::Unreachable { .. }
        | ResponseError::InsecureRedirect { .. }
        | ResponseError::Read { .. }
        | ResponseError::TooLarge { .. }
        | ResponseError::Status { .. }
        | ResponseError::Malformed { .. } => TokenError::Request {
            server: server.to_owned(),
            issuer: issuer(),
            source,
        },
    })?;
    if answer.access_token.trim().is_empty() {
        return Err(TokenError::EmptyAccessToken {
            server: server.to_owned(),
            issuer: issuer(),
        });
    }
    if let Some(token_type) = answer
        .token_type
        .filter(|token_type| !token_type.eq_ignore_ascii_case("bearer"))
    {
        return Err(TokenError::UnsupportedTokenType {
            server: server.to_owned(),
            issuer: issuer(),
            token_type,
        });
    }
    let expires_at = answer
        .expires_in
        .map(|expires_in| {
            expires_in
                .seconds()
                .and_then(|seconds| i64::try_from(seconds).ok())
                .and_then(|seconds| seconds.checked_mul(1000))
                .and_then(|millis| now.checked_add(millis))
                .ok_or_else(|| TokenError::InvalidExpiry {
                    server: server.to_owned(),
                    issuer: issuer(),
                    expires_in: expires_in.text(),
                })
        })
        .transpose()?;
    Ok(Tokens {
        access_token: Secret::new(answer.access_token),
        refresh_token: answer
            .refresh_token
            .filter(|refresh| !refresh.is_empty())
            .map(Secret::new)
            .or_else(|| previous_refresh.cloned()),
        expires_at,
    })
}

#[cfg(test)]
#[path = "../tests/mcp_oauth/flows.rs"]
mod tests;
