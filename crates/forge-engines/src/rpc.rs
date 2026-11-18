use crate::process::{self, EngineProcess, ReadError, SendError, SpawnError};
use serde_json::{Value, json};

const MAX_RESPONSE_BYTES: usize = 4 * 1024 * 1024;

#[derive(Debug, thiserror::Error)]
pub enum CallError {
    #[error("start the RPC probe")]
    Spawn(#[source] SpawnError),
    #[error("send {method}")]
    Send {
        method: String,
        #[source]
        source: SendError,
    },
    #[error("read the answer to {method}")]
    Read {
        method: String,
        #[source]
        source: ReadError,
    },
    #[error("the answer to {method} is over {MAX_RESPONSE_BYTES} bytes")]
    Oversized { method: String },
    #[error("decode the answer to {method}")]
    Decode {
        method: String,
        #[source]
        source: serde_json::Error,
    },
    #[error("{method} was refused: {error}")]
    Refused { method: String, error: Value },
    #[error("the answer to {method} has no result")]
    NoResult { method: String },
    #[error("the engine closed stdout while answering {method}: {stderr}")]
    Closed { method: String, stderr: String },
}

pub struct Session {
    process: EngineProcess,
    next_id: u64,
}
impl Session {
    pub(crate) async fn codex(mut command: tokio::process::Command) -> Result<Self, CallError> {
        command
            .args(["app-server", "--stdio"])
            .stdin(std::process::Stdio::piped());
        let process = process::spawn_probe(command).map_err(CallError::Spawn)?;
        let mut session = Self {
            process,
            next_id: 0,
        };
        session
            .call(
                "initialize",
                json!({
                    "clientInfo": { "name": "artisan-probe", "version": env!("CARGO_PKG_VERSION") },
                    "capabilities": { "experimentalApi": false },
                }),
            )
            .await?;
        session
            .process
            .send(&json!({ "method": "initialized", "params": {} }))
            .await
            .map_err(|source| CallError::Send {
                method: "initialized".to_owned(),
                source,
            })?;
        Ok(session)
    }

    pub(crate) async fn call(&mut self, method: &str, params: Value) -> Result<Value, CallError> {
        self.next_id += 1;
        let id = self.next_id;
        self.process
            .send(&json!({ "id": id, "method": method, "params": params }))
            .await
            .map_err(|source| CallError::Send {
                method: method.to_owned(),
                source,
            })?;
        loop {
            let line = self
                .process
                .next_line()
                .await
                .map_err(|source| CallError::Read {
                    method: method.to_owned(),
                    source,
                })?;
            let Some(line) = line else {
                return Err(CallError::Closed {
                    method: method.to_owned(),
                    stderr: self.process.stderr_tail(),
                });
            };
            if line.len() > MAX_RESPONSE_BYTES {
                return Err(CallError::Oversized {
                    method: method.to_owned(),
                });
            }
            let mut message: Value =
                serde_json::from_str(&line).map_err(|source| CallError::Decode {
                    method: method.to_owned(),
                    source,
                })?;
            if message.get("id").and_then(Value::as_u64) != Some(id) {
                continue;
            }
            if let Some(error) = message.get_mut("error") {
                return Err(CallError::Refused {
                    method: method.to_owned(),
                    error: error.take(),
                });
            }
            return message
                .get_mut("result")
                .map(Value::take)
                .ok_or_else(|| CallError::NoResult {
                    method: method.to_owned(),
                });
        }
    }
}

pub const INVALID_PARAMS: i64 = -32602;
pub const METHOD_NOT_FOUND: i64 = -32601;

#[derive(Debug)]
pub struct Response {
    pub id: u64,
    pub method: String,
    pub result: Result<Value, RpcError>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RpcError {
    pub code: Option<i64>,
    pub message: String,
}

impl std::fmt::Display for RpcError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for RpcError {}

#[derive(Default)]
pub struct State {
    pub next_id: u64,
    requests: std::collections::HashMap<u64, String>,
    pub outbox: Vec<Value>,
    pub responses: Vec<Response>,
}
impl State {
    pub fn request(&mut self, method: &str, params: Value) -> u64 {
        self.next_id += 1;
        let id = self.next_id;
        let mut fields = serde_json::Map::new();
        fields.insert("id".to_owned(), id.into());
        fields.insert("method".to_owned(), method.into());
        fields.insert("params".to_owned(), params);
        let message = Value::Object(fields);
        self.sent(&message);
        self.outbox.push(message);
        id
    }
    pub fn sent(&mut self, message: &Value) {
        if let (Some(id), Some(method)) = (message["id"].as_u64(), message["method"].as_str()) {
            self.next_id = self.next_id.max(id);
            self.requests.insert(id, method.to_owned());
        }
    }
    pub fn reply(&mut self, rpc: Value, result: Value) {
        self.answer(rpc, "result", result);
    }

    pub fn reply_error(&mut self, rpc: Value, code: i64, message: &str) {
        self.answer(rpc, "error", json!({ "code": code, "message": message }));
    }

    fn answer(&mut self, rpc: Value, field: &str, value: Value) {
        let mut message = serde_json::Map::new();
        message.insert("id".to_owned(), rpc);
        message.insert(field.to_owned(), value);
        self.outbox.push(Value::Object(message));
    }

    pub fn respond(
        &mut self,
        id: &Value,
        result: Option<Value>,
        error: Option<&Value>,
    ) -> Option<Response> {
        let id = id.as_u64()?;
        let Some(method) = self.requests.remove(&id) else {
            tracing::warn!(id, "engine answered an unknown RPC request");
            return None;
        };
        let result = error.map_or_else(
            || {
                result.ok_or_else(|| RpcError {
                    code: None,
                    message: "engine RPC response omitted result".to_owned(),
                })
            },
            |error| {
                let message = error["message"].as_str().unwrap_or("engine error");
                let detail = error["data"]["message"]
                    .as_str()
                    .or_else(|| error["data"].as_str());
                Err(RpcError {
                    code: error["code"].as_i64(),
                    message: match detail {
                        Some(detail) if detail.starts_with(message) => detail.to_owned(),
                        Some(detail) => format!("{message}: {detail}"),
                        None => message.to_owned(),
                    },
                })
            },
        );
        Some(Response { id, method, result })
    }
}

#[cfg(test)]
#[path = "../tests/rpc/unit.rs"]
mod tests;
