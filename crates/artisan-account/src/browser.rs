use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use sha2::{Digest as _, Sha256};
use std::{io, time::Duration};
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

const REQUEST_LIMIT: usize = 16 * 1024;
const READ_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Provider {
    Google,
    GitHub,
}

impl Provider {
    pub const ALL: [Self; 2] = [Self::Google, Self::GitHub];

    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Google => "Google",
            Self::GitHub => "GitHub",
        }
    }

    #[must_use]
    const fn slug(self) -> &'static str {
        match self {
            Self::Google => "google",
            Self::GitHub => "github",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Problem {
    Denied,
    Unavailable,
    NoVerifiedEmail,
    EmailTaken,
    Unfinished,
}

impl Problem {
    #[must_use]
    fn of(slug: &str) -> Self {
        match slug {
            "denied" | "cancelled" => Self::Denied,
            "unavailable" => Self::Unavailable,
            "no-verified-email" => Self::NoVerifiedEmail,
            "email-taken" => Self::EmailTaken,
            _ => Self::Unfinished,
        }
    }

    #[must_use]
    const fn slug(self) -> &'static str {
        match self {
            Self::Denied => "denied",
            Self::Unavailable => "unavailable",
            Self::NoVerifiedEmail => "no-verified-email",
            Self::EmailTaken => "email-taken",
            Self::Unfinished => "unfinished",
        }
    }

    #[must_use]
    pub fn message(self, provider: Provider) -> String {
        let name = provider.name();
        match self {
            Self::Denied => format!("{name} sign-in was cancelled. Press the button to try again."),
            Self::Unavailable => {
                format!("{name} didn't answer our sign-in request. Try again in a moment.")
            }
            Self::NoVerifiedEmail => format!(
                "{name} didn't share a verified email with us. Verify it there, then try again."
            ),
            Self::EmailTaken => {
                "That email already signs in another way. Use the emailed link instead.".to_owned()
            }
            Self::Unfinished => {
                format!(
                    "{name} sign-in didn't finish in the browser. Press the button to try again."
                )
            }
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Arrival {
    Code(String),
    Problem(Problem),
    Unrelated,
}

#[derive(Clone, Eq, PartialEq)]
pub struct Handoff {
    verifier: String,
    challenge: String,
    state: String,
}

impl std::fmt::Debug for Handoff {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Handoff")
            .field("verifier", &"redacted")
            .field("challenge", &self.challenge)
            .field("state", &self.state)
            .finish()
    }
}

impl Handoff {
    pub fn generate() -> Result<Self, getrandom::Error> {
        let mut verifier = [0_u8; 32];
        let mut state = [0_u8; 32];
        getrandom::fill(&mut verifier)?;
        getrandom::fill(&mut state)?;
        Ok(Self::from_secrets(verifier, state))
    }

    #[must_use]
    pub fn from_secrets(verifier: [u8; 32], state: [u8; 32]) -> Self {
        let verifier = URL_SAFE_NO_PAD.encode(verifier);
        let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
        Self {
            verifier,
            challenge,
            state: URL_SAFE_NO_PAD.encode(state),
        }
    }

    #[must_use]
    pub fn verifier(&self) -> &str {
        &self.verifier
    }

    #[must_use]
    pub fn challenge(&self) -> &str {
        &self.challenge
    }

    #[must_use]
    pub fn state(&self) -> &str {
        &self.state
    }

    #[must_use]
    pub fn start_url(&self, auth_origin: &str, provider: Provider, port: u16) -> String {
        format!(
            "{}/desktop?provider={}&port={port}&challenge={}&state={}",
            auth_origin.trim_end_matches('/'),
            provider.slug(),
            self.challenge,
            self.state
        )
    }
}

#[must_use]
pub fn arrival(target: &str, state: &str) -> Arrival {
    let Some(query) = target.strip_prefix("/callback?") else {
        return Arrival::Unrelated;
    };
    let field = |name: &str| {
        query.split('&').find_map(|pair| {
            pair.split_once('=')
                .filter(|(key, _)| *key == name)
                .map(|(_, value)| value)
        })
    };
    match (field("state") == Some(state), field("code"), field("error")) {
        (false, _, _) => Arrival::Unrelated,
        (true, _, Some(error)) => Arrival::Problem(Problem::of(error)),
        (true, Some(code), None) if !code.is_empty() => Arrival::Code(code.to_owned()),
        (true, _, None) => Arrival::Problem(Problem::Unfinished),
    }
}

#[must_use]
pub fn done_location(auth_origin: &str, arrival: &Arrival) -> String {
    let base = format!("{}/desktop/done", auth_origin.trim_end_matches('/'));
    match arrival {
        Arrival::Problem(problem) => format!("{base}?problem={}", problem.slug()),
        Arrival::Code(_) | Arrival::Unrelated => base,
    }
}

#[must_use]
pub fn request_target(head: &str) -> Option<&str> {
    let mut parts = head.lines().next()?.split(' ');
    match (parts.next(), parts.next()) {
        (Some("GET"), Some(target)) => Some(target),
        _ => None,
    }
}

pub fn bind() -> io::Result<(std::net::TcpListener, u16)> {
    let listener = std::net::TcpListener::bind(("127.0.0.1", 0))?;
    listener.set_nonblocking(true)?;
    let port = listener.local_addr()?.port();
    Ok((listener, port))
}

pub async fn receive(
    listener: std::net::TcpListener,
    state: &str,
    auth_origin: &str,
) -> io::Result<Arrival> {
    let listener = tokio::net::TcpListener::from_std(listener)?;
    loop {
        let (mut stream, _) = listener.accept().await?;
        let arrival = match read_get_target(&mut stream).await {
            Ok(target) => arrival(&target, state),
            Err(RequestError::NotGet) => Arrival::Unrelated,
            Err(
                error @ (RequestError::Read(_)
                | RequestError::TooLarge { .. }
                | RequestError::TimedOut),
            ) => {
                tracing::debug!(%error, "skipped an unreadable sign-in callback connection");
                continue;
            }
        };
        let response = match &arrival {
            Arrival::Unrelated => {
                "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                    .to_owned()
            }
            Arrival::Code(_) | Arrival::Problem(_) => format!(
                "HTTP/1.1 303 See Other\r\nLocation: {}\r\nCache-Control: no-store\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                done_location(auth_origin, &arrival)
            ),
        };
        if let Err(error) = answer(&mut stream, &response).await {
            tracing::warn!(%error, "the browser did not receive the sign-in callback page");
        }
        if !matches!(arrival, Arrival::Unrelated) {
            return Ok(arrival);
        }
    }
}

async fn answer(stream: &mut tokio::net::TcpStream, response: &str) -> io::Result<()> {
    stream.write_all(response.as_bytes()).await?;
    stream.shutdown().await
}

#[derive(Debug, thiserror::Error)]
pub enum RequestError {
    #[error("reading the browser's request")]
    Read(#[source] io::Error),
    #[error("the browser's request is over {limit} bytes")]
    TooLarge { limit: usize },
    #[error("the browser didn't finish its request within {READ_TIMEOUT:?}")]
    TimedOut,
    #[error("the browser's request isn't a GET")]
    NotGet,
}

pub async fn read_get_target(stream: &mut tokio::net::TcpStream) -> Result<String, RequestError> {
    let head = tokio::time::timeout(READ_TIMEOUT, read_head(stream))
        .await
        .map_err(|_elapsed| RequestError::TimedOut)??;
    request_target(&head)
        .map(str::to_owned)
        .ok_or(RequestError::NotGet)
}

async fn read_head(stream: &mut tokio::net::TcpStream) -> Result<String, RequestError> {
    let mut head = Vec::new();
    let mut chunk = [0_u8; 1024];
    while !head.windows(4).any(|window| window == b"\r\n\r\n") {
        let read = stream.read(&mut chunk).await.map_err(RequestError::Read)?;
        if read == 0 {
            break;
        }
        head.extend_from_slice(&chunk[..read]);
        if head.len() > REQUEST_LIMIT {
            return Err(RequestError::TooLarge {
                limit: REQUEST_LIMIT,
            });
        }
    }
    Ok(String::from_utf8_lossy(&head).into_owned())
}
