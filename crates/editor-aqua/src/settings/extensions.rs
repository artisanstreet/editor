use artisan_api::{McpMutation, McpServer};
use std::collections::BTreeMap;

pub const MCP_SERVERS_KEY: &str = "mcp_servers";
pub const MANAGE_SKILLS_SELECTOR: &str = "settings-manage-skills";

#[derive(Clone, Debug, Default)]
pub struct ServerFields {
    pub name: String,
    pub command: String,
    pub args: String,
    pub url: String,
    pub env: String,
    pub headers: String,
}

impl ServerFields {
    pub fn from_server(server: &McpServer) -> Self {
        Self {
            name: server.name.clone(),
            command: server.command.clone(),
            args: serde_json::to_string(&server.args).unwrap_or_default(),
            url: server.url.clone().unwrap_or_default(),
            env: serde_json::to_string(&server.env).unwrap_or_default(),
            headers: if server.headers.is_empty() {
                String::new()
            } else {
                serde_json::to_string(&server.headers).unwrap_or_default()
            },
        }
    }

    pub fn parse(&self, previous: Option<&McpServer>) -> Result<McpServer, String> {
        let name = self.name.trim().to_owned();
        let url = self.url.trim().to_owned();
        if name.is_empty() {
            return Err("Give the server a name.".into());
        }
        if self.command.is_empty() == url.is_empty() {
            return Err("Specify either a program or an HTTP URL.".into());
        }
        let args: Vec<String> = if self.args.trim().is_empty() {
            Vec::new()
        } else {
            serde_json::from_str(&self.args)
                .map_err(|error| format!("Arguments must be a JSON array of strings: {error}"))?
        };
        let env: BTreeMap<String, String> = if self.env.trim().is_empty() {
            BTreeMap::new()
        } else {
            serde_json::from_str(&self.env)
                .map_err(|error| format!("Environment must be a JSON object of strings: {error}"))?
        };
        if !url.is_empty() && (!args.is_empty() || !env.is_empty()) {
            return Err(
                "HTTP servers do not take process arguments or environment variables.".into(),
            );
        }
        let headers: BTreeMap<String, String> = if self.headers.trim().is_empty() {
            BTreeMap::new()
        } else {
            serde_json::from_str(&self.headers)
                .map_err(|error| format!("Headers must be a JSON object of strings: {error}"))?
        };
        if url.is_empty() && !headers.is_empty() {
            return Err("Only HTTP servers take headers.".into());
        }
        Ok(McpServer {
            name,
            command: self.command.clone(),
            args,
            env,
            url: (!url.is_empty()).then_some(url),
            headers,
            sse: previous.is_some_and(|server| server.sse),
        })
    }
}

#[derive(Clone, Debug, Default)]
pub struct ExtensionsForm {
    pub host: Option<String>,
    pub editing: Option<Option<McpServer>>,
    pub fields: ServerFields,
    pub saving: bool,
    pub error: Option<String>,
    generation: u64,
}

#[derive(Clone, Debug)]
pub struct MutationRequest {
    pub host: String,
    pub generation: u64,
    pub mutation: McpMutation,
    pub close_form: bool,
}

impl ExtensionsForm {
    pub fn sync_host(&mut self, host: Option<String>) {
        if self.host == host {
            return;
        }
        self.host = host;
        self.generation += 1;
        self.saving = false;
        self.editing = None;
        self.error = None;
    }

    pub fn open(&mut self, previous: Option<McpServer>) {
        self.generation += 1;
        self.saving = false;
        self.fields = ServerFields::from_server(previous.as_ref().unwrap_or(&McpServer::default()));
        self.editing = Some(previous);
        self.error = None;
    }

    pub fn cancel(&mut self) {
        self.generation += 1;
        self.saving = false;
        self.editing = None;
        self.error = None;
    }

    pub fn save(&mut self) -> Option<MutationRequest> {
        if self.saving {
            return None;
        }
        let previous = self.editing.clone()?;
        self.host.as_ref()?;
        match self.fields.parse(previous.as_ref()) {
            Ok(server) => self.begin(McpMutation::Upsert { previous, server }, true),
            Err(error) => {
                self.error = Some(error);
                None
            }
        }
    }

    pub fn remove(&mut self, previous: McpServer) -> Option<MutationRequest> {
        if self.saving {
            return None;
        }
        self.begin(McpMutation::Remove { previous }, false)
    }

    fn begin(&mut self, mutation: McpMutation, close_form: bool) -> Option<MutationRequest> {
        let host = self.host.clone()?;
        self.generation += 1;
        self.saving = true;
        self.error = None;
        Some(MutationRequest {
            host,
            generation: self.generation,
            mutation,
            close_form,
        })
    }

    pub fn complete(&mut self, request: &MutationRequest, result: Result<(), String>) {
        if self.host.as_ref() != Some(&request.host) || self.generation != request.generation {
            return;
        }
        self.saving = false;
        match result {
            Ok(()) if request.close_form => self.editing = None,
            Ok(()) => {}
            Err(error) => self.error = Some(format!("Couldn't save MCP servers: {error}")),
        }
    }
}
