use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use sha2::{Digest as _, Sha256};
use std::{io, time::Duration};
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

const REQUEST_LIMIT: usize = 8 * 1024;
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
    pub const fn slug(self) -> &'static str {
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
    pub fn of(slug: &str) -> Self {
        match slug {
            "denied" | "cancelled" => Self::Denied,
            "unavailable" => Self::Unavailable,
            "no-verified-email" => Self::NoVerifiedEmail,
            "email-taken" => Self::EmailTaken,
            _ => Self::Unfinished,
        }
    }

    #[must_use]
    pub const fn slug(self) -> &'static str {
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
                format!("{name} sign-in didn't finish in the browser. Press the button to try again.")
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

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Handoff {
    verifier: String,
    challenge: String,
    state: String,
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
        let Ok(Ok(head)) = tokio::time::timeout(READ_TIMEOUT, read_head(&mut stream)).await else {
            continue;
        };
        let arrival = request_target(&head).map_or(Arrival::Unrelated, |target| arrival(target, state));
        let response = match &arrival {
            Arrival::Unrelated => {
                "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_owned()
            }
            Arrival::Code(_) | Arrival::Problem(_) => format!(
                "HTTP/1.1 303 See Other\r\nLocation: {}\r\nCache-Control: no-store\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                done_location(auth_origin, &arrival)
            ),
        };
        stream.write_all(response.as_bytes()).await.ok();
        stream.shutdown().await.ok();
        if !matches!(arrival, Arrival::Unrelated) {
            return Ok(arrival);
        }
    }
}

async fn read_head(stream: &mut tokio::net::TcpStream) -> io::Result<String> {
    let mut head = Vec::new();
    let mut chunk = [0_u8; 1024];
    while !head.windows(4).any(|window| window == b"\r\n\r\n") && head.len() < REQUEST_LIMIT {
        let read = stream.read(&mut chunk).await?;
        if read == 0 {
            break;
        }
        head.extend_from_slice(&chunk[..read]);
    }
    Ok(String::from_utf8_lossy(&head).into_owned())
}
