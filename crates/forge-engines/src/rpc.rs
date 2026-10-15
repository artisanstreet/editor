//! JSON-RPC request correlation and bounded stdio for short provider probes.
//! Provider event normalization remains in each adapter.

use crate::process::{self, EngineProcess};
use anyhow::{Context, Result, bail};
use serde_json::{Value, json};

pub(crate) struct Session {
    process: EngineProcess,
    next_id: u64,
}
impl Session {
    pub(crate) async fn codex(mut command: tokio::process::Command) -> Result<Self> {
        command
            .args(["app-server", "--stdio"])
            .stdin(std::process::Stdio::piped());
        let process = process::spawn_probe(command)?;
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
            .await?;
        Ok(session)
    }

    pub(crate) async fn call(&mut self, method: &str, params: Value) -> Result<Value> {
        self.next_id += 1;
        let id = self.next_id;
        self.process
            .send(&json!({ "id": id, "method": method, "params": params }))
            .await?;
        while let Some(line) = self.process.next_line().await? {
            anyhow::ensure!(
                line.len() <= 4 * 1024 * 1024,
                "RPC probe response exceeds 4 MiB"
            );
            let message: Value = serde_json::from_str(&line).context("decoding RPC response")?;
            if message.get("id").and_then(Value::as_u64) != Some(id) {
                continue;
            }
            if let Some(error) = message.get("error") {
                bail!("{method}: {error}");
            }
            return message
                .get("result")
                .cloned()
                .context("RPC response omitted result");
        }
        bail!(
            "engine closed stdout while awaiting {method}: {}",
            self.process.stderr_tail()
        )
    }
}

#[derive(Debug)]
pub(crate) struct Response {
    pub id: u64,
    pub method: String,
    pub result: Result<Value, String>,
}

/// Correlation and wire queues shared by ACP and Codex; provider semantics stay
/// in their decoders. No process/task lifecycle is hidden in this value.
#[derive(Default)]
pub(crate) struct State {
    jsonrpc: bool,
    pub next_id: u64,
    requests: std::collections::HashMap<u64, String>,
    pub outbox: Vec<Value>,
    pub responses: Vec<Response>,
}
impl State {
    pub fn acp() -> Self {
        Self {
            jsonrpc: true,
            ..Self::default()
        }
    }
    pub fn request(&mut self, method: &str, params: Value) -> u64 {
        self.next_id += 1;
        let id = self.next_id;
        let mut message = json!({ "id": id, "method": method, "params": params });
        if self.jsonrpc {
            message["jsonrpc"] = json!("2.0");
        }
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
    pub fn notify(&mut self, method: &str, params: Option<Value>) {
        let mut message = json!({ "method": method });
        if self.jsonrpc {
            message["jsonrpc"] = json!("2.0");
        }
        if let Some(params) = params {
            message["params"] = params;
        }
        self.outbox.push(message);
    }
    pub fn response(&mut self, message: &Value) -> Option<Response> {
        let id = message["id"].as_u64()?;
        let Some(method) = self.requests.remove(&id) else {
            tracing::warn!(id, "engine answered an unknown RPC request");
            return None;
        };
        let result = if let Some(error) = message.get("error") {
            let message = error["message"].as_str().unwrap_or("engine error");
            let detail = error["data"]["message"]
                .as_str()
                .or_else(|| error["data"].as_str());
            Err(match detail {
                Some(detail) if detail.starts_with(message) => detail.to_owned(),
                Some(detail) => format!("{message}: {detail}"),
                None => message.to_owned(),
            })
        } else {
            message
                .get("result")
                .cloned()
                .ok_or_else(|| "engine RPC response omitted result".to_owned())
        };
        Some(Response { id, method, result })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn replies_correlate_out_of_order_and_unknown_ids_do_not_consume_requests() {
        let mut state = State::default();
        let first = state.request("first", json!({}));
        let second = state.request("second", json!({}));
        assert!(
            state
                .response(&json!({ "id": 999, "result": {} }))
                .is_none()
        );
        assert_eq!(
            state
                .response(&json!({ "id": second, "result": { "ok": true } }))
                .unwrap()
                .method,
            "second"
        );
        assert_eq!(state.response(&json!({ "id": first, "error": { "message": "rejected", "data": { "message": "details" } } })).unwrap().result, Err("rejected: details".into()));
        assert!(state.requests.is_empty());
    }
}
