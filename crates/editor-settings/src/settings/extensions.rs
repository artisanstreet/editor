use artisan_api::{McpMutation, McpServer};
use std::collections::BTreeMap;

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
    fn from_server(server: &McpServer) -> Self {
        Self {
            name: server.name.clone(),
            command: server.command.clone(),
            args: serde_json::to_string(&server.args)
                .expect("invariant: a list of strings always serializes"),
            url: server.url.clone().unwrap_or_default(),
            env: serde_json::to_string(&server.env)
                .expect("invariant: a map of strings always serializes"),
            headers: if server.headers.is_empty() {
                String::new()
            } else {
                serde_json::to_string(&server.headers)
                    .expect("invariant: a map of strings always serializes")
            },
        }
    }

    pub fn parse(&self, previous: Option<&McpServer>) -> Result<McpServer, String> {
        let name = self.name.trim().to_owned();
        let url = self.url.trim().to_owned();
        let command = self.command.trim().to_owned();
        if name.is_empty() {
            return Err("Give the server a name.".into());
        }
        if command.is_empty() == url.is_empty() {
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
            command,
            args,
            env,
            url: (!url.is_empty()).then_some(url),
            headers,
            sse: previous.is_some_and(|server| server.sse),
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum McpEditing {
    New,
    Existing(McpServer),
}

impl McpEditing {
    fn previous(&self) -> Option<McpServer> {
        match self {
            Self::New => None,
            Self::Existing(server) => Some(server.clone()),
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct ExtensionsForm {
    pub(crate) host: Option<String>,
    pub editing: Option<McpEditing>,
    pub fields: ServerFields,
    pub saving: bool,
    pub(crate) error: Option<String>,
    generation: u64,
}

#[derive(Clone, Debug)]
pub struct MutationRequest {
    pub(crate) host: String,
    pub(crate) generation: u64,
    pub(crate) mutation: McpMutation,
    pub(crate) close_form: bool,
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
        self.fields = ServerFields::from_server(&previous.clone().unwrap_or_default());
        self.editing = Some(previous.map_or(McpEditing::New, McpEditing::Existing));
        self.error = None;
    }

    pub(crate) fn cancel(&mut self) {
        if self.saving {
            return;
        }
        self.generation += 1;
        self.saving = false;
        self.editing = None;
        self.error = None;
    }

    pub fn save(&mut self) -> Option<MutationRequest> {
        if self.saving {
            return None;
        }
        let previous = self.editing.as_ref()?.previous();
        self.host.as_ref()?;
        match self.fields.parse(previous.as_ref()) {
            Ok(server) => self.begin(McpMutation::Upsert { previous, server }, true),
            Err(error) => {
                self.error = Some(error);
                None
            }
        }
    }

    pub(crate) fn remove(&mut self, previous: McpServer) -> Option<MutationRequest> {
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
