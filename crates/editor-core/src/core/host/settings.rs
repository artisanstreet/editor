use crate::core::hosts::HostId;
use artisan_api::McpServer;
use serde::Deserialize as _;
use serde_json::{Map, Value};
use std::sync::Arc;

pub const MCP_SERVERS_KEY: &str = "mcp_servers";

#[derive(Debug, thiserror::Error)]
#[error("read the MCP servers {host} holds")]
pub struct McpServersError {
    pub host: HostId,
    #[source]
    pub source: serde_json::Error,
}

pub struct HostSettings {
    host: HostId,
    values: Map<String, Value>,
    mcp_servers: Result<Vec<McpServer>, Arc<McpServersError>>,
}

impl HostSettings {
    #[must_use]
    pub fn new(host: HostId) -> Self {
        Self::from_values(host, Map::new())
    }

    #[must_use]
    pub fn from_values(host: HostId, values: Map<String, Value>) -> Self {
        let mut settings = Self {
            host,
            values,
            mcp_servers: Ok(Vec::new()),
        };
        settings.decode();
        settings
    }

    #[must_use]
    pub fn get(&self, key: &str) -> Option<&Value> {
        self.values.get(key)
    }

    pub fn insert(&mut self, key: String, value: Value) -> Option<Value> {
        let decodes = key == MCP_SERVERS_KEY;
        let previous = self.values.insert(key, value);
        if decodes {
            self.decode();
        }
        previous
    }

    pub fn remove(&mut self, key: &str) -> Option<Value> {
        let previous = self.values.remove(key);
        if key == MCP_SERVERS_KEY {
            self.decode();
        }
        previous
    }

    pub fn mcp_servers(&self) -> Result<&[McpServer], &McpServersError> {
        match &self.mcp_servers {
            Ok(servers) => Ok(servers),
            Err(error) => Err(error),
        }
    }

    fn decode(&mut self) {
        self.mcp_servers = match self.values.get(MCP_SERVERS_KEY) {
            None => Ok(Vec::new()),
            Some(value) => Vec::<McpServer>::deserialize(value).map_err(|source| {
                let error = McpServersError {
                    host: self.host.clone(),
                    source,
                };
                tracing::warn!(
                    host = %self.host,
                    error = %::diagnostics::chain(&error),
                    "Forge holds MCP servers the Editor cannot read"
                );
                Arc::new(error)
            }),
        };
    }
}
