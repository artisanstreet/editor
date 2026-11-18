use std::path::{Path, PathBuf};
use std::time::Duration;

use artisan_api::ClaudeAiSession;
use futures::{SinkExt as _, StreamExt as _};
use serde_json::{Value, json};
use tokio_tungstenite::tungstenite::{self, Message};

const ANSWER_TIMEOUT: Duration = Duration::from_secs(15);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug, thiserror::Error)]
pub enum DevToolsError {
    #[error("reading the DevTools port file {path}")]
    PortFile {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("the DevTools port file {path} is incomplete")]
    IncompletePortFile { path: PathBuf },
    #[error("connecting to the browser's DevTools on port {port}")]
    Connect {
        port: u16,
        #[source]
        source: tungstenite::Error,
    },
    #[error("the browser did not take the DevTools connection on port {port}")]
    ConnectTimedOut { port: u16 },
    #[error("sending {method} to the browser")]
    Send {
        method: &'static str,
        #[source]
        source: tungstenite::Error,
    },
    #[error("reading the browser's answer to {method}")]
    Receive {
        method: &'static str,
        #[source]
        source: tungstenite::Error,
    },
    #[error("decoding the browser's answer to {method}")]
    Decode {
        method: &'static str,
        #[source]
        source: serde_json::Error,
    },
    #[error("the browser closed before answering {method}")]
    Closed { method: &'static str },
    #[error("the browser did not answer {method}")]
    TimedOut { method: &'static str },
    #[error("the browser refused {method}: {error}")]
    Refused { method: &'static str, error: String },
    #[error("the browser did not say its user agent")]
    NoUserAgent,
}

pub(crate) struct DevTools {
    socket: tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
    next_id: u64,
}

impl DevTools {
    pub(crate) async fn connect(port_file: &Path) -> Result<Self, DevToolsError> {
        let text = tokio::fs::read_to_string(port_file)
            .await
            .map_err(|source| DevToolsError::PortFile {
                path: port_file.to_path_buf(),
                source,
            })?;
        let (port, path) =
            parse_port_file(&text).ok_or_else(|| DevToolsError::IncompletePortFile {
                path: port_file.to_path_buf(),
            })?;
        let url = format!("ws://127.0.0.1:{port}{path}");
        let (socket, _) = tokio::time::timeout(
            CONNECT_TIMEOUT,
            tokio_tungstenite::connect_async(url.as_str()),
        )
        .await
        .map_err(|_elapsed| DevToolsError::ConnectTimedOut { port })?
        .map_err(|source| DevToolsError::Connect { port, source })?;
        Ok(Self { socket, next_id: 0 })
    }

    pub(crate) async fn call(
        &mut self,
        method: &'static str,
        params: Value,
    ) -> Result<Value, DevToolsError> {
        self.next_id += 1;
        let id = self.next_id;
        let request = json!({ "id": id, "method": method, "params": params });
        self.socket
            .send(Message::text(request.to_string()))
            .await
            .map_err(|source| DevToolsError::Send { method, source })?;
        let mut answer = tokio::time::timeout(ANSWER_TIMEOUT, self.answer(method, id))
            .await
            .map_err(|_elapsed| DevToolsError::TimedOut { method })??;
        if let Some(error) = answer.get("error") {
            return Err(DevToolsError::Refused {
                method,
                error: error.to_string(),
            });
        }
        Ok(answer["result"].take())
    }

    async fn answer(&mut self, method: &'static str, id: u64) -> Result<Value, DevToolsError> {
        while let Some(message) = self.socket.next().await {
            let message = message.map_err(|source| DevToolsError::Receive { method, source })?;
            let Message::Text(text) = message else {
                continue;
            };
            let value: Value = serde_json::from_str(&text)
                .map_err(|source| DevToolsError::Decode { method, source })?;
            if value["id"].as_u64() == Some(id) {
                return Ok(value);
            }
        }
        Err(DevToolsError::Closed { method })
    }

    pub(crate) async fn cookies(&mut self) -> Result<Vec<(String, String)>, DevToolsError> {
        let result = self.call("Storage.getCookies", json!({})).await?;
        Ok(crate::claude_ai_cookies(&result["cookies"]))
    }

    pub(crate) async fn session(&mut self) -> Result<ClaudeAiSession, DevToolsError> {
        let cookies = self.cookies().await?;
        let version = self.call("Browser.getVersion", json!({})).await?;
        let user_agent = version["userAgent"]
            .as_str()
            .ok_or(DevToolsError::NoUserAgent)?
            .to_owned();
        Ok(ClaudeAiSession {
            cookies,
            user_agent,
        })
    }
}

pub(crate) fn parse_port_file(text: &str) -> Option<(u16, String)> {
    let mut lines = text.lines();
    let port = lines.next()?.trim().parse().ok()?;
    let path = lines.next()?.trim();
    path.starts_with("/devtools/browser/")
        .then(|| (port, path.to_owned()))
}
