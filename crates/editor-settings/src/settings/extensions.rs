use artisan_api::{McpMutation, McpServer};
use editor_core::{HostInstance, Problem};
use std::collections::{BTreeMap, HashMap};

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

#[derive(Clone, Debug)]
struct ParkedForm {
    instance: HostInstance,
    editing: Option<McpEditing>,
    fields: ServerFields,
    saving: bool,
    error: Option<Problem>,
    generation: u64,
}

impl ParkedForm {
    fn is_idle(&self) -> bool {
        self.editing.is_none() && !self.saving && self.error.is_none()
    }
}

fn settle(
    editing: &mut Option<McpEditing>,
    error: &mut Option<Problem>,
    request: &MutationRequest,
    result: Result<(), Problem>,
) {
    match result {
        Ok(()) if request.close_form => *editing = None,
        Ok(()) => {}
        Err(problem) => *error = Some(problem),
    }
}

#[derive(Clone, Debug, Default)]
pub struct ExtensionsForm {
    pub(crate) host: Option<String>,
    instance: Option<HostInstance>,
    pub editing: Option<McpEditing>,
    pub fields: ServerFields,
    pub saving: bool,
    pub(crate) error: Option<Problem>,
    generation: u64,
    parked: HashMap<String, ParkedForm>,
}

#[derive(Clone, Debug)]
pub struct MutationRequest {
    pub(crate) host: String,
    pub(crate) generation: u64,
    pub(crate) mutation: McpMutation,
    pub(crate) close_form: bool,
}

impl ExtensionsForm {
    pub fn sync_host(&mut self, host: Option<(String, HostInstance)>) -> bool {
        let (name, instance) = host.unzip();
        if self.host == name && self.instance == instance {
            return false;
        }
        self.park();
        self.host = name;
        self.instance = instance;
        self.generation += 1;
        if let Some(parked) = self
            .host
            .as_ref()
            .and_then(|host| self.parked.remove(host))
            .filter(|parked| Some(parked.instance) == self.instance)
        {
            self.editing = parked.editing;
            self.fields = parked.fields;
            self.saving = parked.saving;
            self.error = parked.error;
            self.generation = parked.generation;
        }
        true
    }

    fn park(&mut self) {
        let editing = self.editing.take();
        let fields = std::mem::take(&mut self.fields);
        let saving = std::mem::replace(&mut self.saving, false);
        let error = self.error.take();
        if let (Some(host), Some(instance)) = (self.host.take(), self.instance.take()) {
            let parked = ParkedForm {
                instance,
                editing,
                fields,
                saving,
                error,
                generation: self.generation,
            };
            if !parked.is_idle() {
                self.parked.insert(host, parked);
            }
        }
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
                self.error = Some(Problem::new(error));
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

    pub fn complete(&mut self, request: &MutationRequest, result: Result<(), Problem>) {
        if self.host.as_ref() == Some(&request.host) {
            if self.generation == request.generation {
                self.saving = false;
                settle(&mut self.editing, &mut self.error, request, result);
            }
            return;
        }
        if let Some(parked) = self.parked.get_mut(&request.host)
            && parked.generation == request.generation
        {
            parked.saving = false;
            settle(&mut parked.editing, &mut parked.error, request, result);
            if parked.is_idle() {
                self.parked.remove(&request.host);
            }
        }
    }
}
