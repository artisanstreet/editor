//! The tools the claude.ai transport offers the model, and the runner that
//! executes them on this machine.
//!
//! claude.ai calls a client tool and waits for the result, so the transport
//! needs a tool runtime of its own. The tools carry Claude Code's names and
//! input shapes, so the model calls them the way it is trained to and the
//! Editor renders them the way it renders Claude Code's.

use std::collections::HashSet;
use std::fmt::Write as _;
use std::net::{IpAddr, SocketAddr, ToSocketAddrs as _};
use std::path::{Path, PathBuf};
use std::time::Duration;

use artisan_api::{
    ApprovalRequest, Decision, ErrorKind, ItemKind, McpServer, Output, Permissions, TextContent,
    ToolKind,
};
use regex::Regex;
use serde_json::{Value, json};
use tokio::io::AsyncReadExt as _;
use tokio_util::sync::CancellationToken;

use super::rules::{Rules, Verdict, changes_files, url_host};
use super::{mcp, shells, skills};

/// The most of a tool's output the model is sent.
const MAX_OUTPUT_BYTES: usize = 256 * 1024;
/// The largest file Read, Edit or Grep open, and Write writes.
const MAX_FILE_BYTES: u64 = 16 * 1024 * 1024;
/// The lines Read returns when the model does not say.
const DEFAULT_READ_LINES: u64 = 2000;
/// The longest line Read returns whole.
const MAX_LINE_CHARS: usize = 2000;
/// How long a command runs when the model does not say, and the most it may.
const DEFAULT_COMMAND_TIMEOUT: Duration = Duration::from_secs(120);
const MAX_COMMAND_TIMEOUT: Duration = Duration::from_secs(600);
/// The most entries a glob or a search walks, and the most it returns.
const MAX_WALKED: usize = 200_000;
const MAX_MATCHES: usize = 2_000;
/// How long WebFetch waits for a page, and the most of it that is read.
const WEB_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_WEB_BYTES: usize = 2 * 1024 * 1024;
/// The most redirects WebFetch follows within one site.
const MAX_REDIRECTS: usize = 10;

/// A tool call the model made.
#[derive(Clone, Debug)]
pub struct Call {
    /// The id claude.ai gave the call.
    pub id: String,
    /// The tool's name.
    pub name: String,
    /// The tool's arguments.
    pub input: Value,
}

/// What to do with a call.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Plan {
    /// Run it.
    Run,
    /// Ask the user first.
    Ask(ApprovalRequest),
    /// Do not run it; the model is told why.
    Refuse(String),
}

/// What a call answered.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Answer {
    /// What the model is told.
    pub text: String,
    /// Whether the call failed or was refused.
    pub is_error: bool,
}

impl Answer {
    fn ok(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            is_error: false,
        }
    }

    fn error(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            is_error: true,
        }
    }
}

/// What a run's tools share: the skills, MCP servers, background shells and
/// permission rules the turn's machine and settings carry.
pub struct Runtime {
    pub skills: Vec<skills::Skill>,
    pub mcp: mcp::Servers,
    pub shells: shells::Shells,
    pub rules: Rules,
}

impl Runtime {
    /// Reads the run's skills and rules and connects its MCP servers: the
    /// launch's (Forge's own included) and the project's `.mcp.json`.
    pub async fn build(home: &Path, cwd: &Path, servers: &[McpServer]) -> Self {
        let mut servers = servers.to_vec();
        for project in mcp::project_servers(cwd) {
            if !servers.iter().any(|known| known.name == project.name) {
                servers.push(project);
            }
        }
        Self {
            skills: skills::discover(home, cwd),
            mcp: mcp::Servers::connect(&servers, cwd).await,
            shells: shells::Shells::default(),
            rules: Rules::load(home, cwd),
        }
    }
}

/// The approvals the user gave "always" for, for the rest of the run.
#[derive(Default)]
pub struct Allowed {
    file_changes: bool,
    commands: HashSet<String>,
    /// MCP tools (their `mcp__server__tool` names) the user allowed.
    tools: HashSet<String>,
    /// Hosts WebFetch may read.
    domains: HashSet<String>,
}

impl Allowed {
    /// Remembers an "always" answer to the call.
    pub fn remember(&mut self, call: &Call, decision: Decision) {
        if decision != Decision::AllowAlways {
            return;
        }
        match call.name.as_str() {
            name if changes_files(name) => self.file_changes = true,
            "Bash" => {
                self.commands.insert(command_of(call).to_owned());
            }
            "WebFetch" => {
                if let Some(host) = web_host(call) {
                    self.domains.insert(host);
                }
            }
            name if name.starts_with("mcp__") => {
                self.tools.insert(name.to_owned());
            }
            _ => {}
        }
    }

    fn covers(&self, call: &Call) -> bool {
        match call.name.as_str() {
            name if changes_files(name) => self.file_changes,
            "Bash" => self.commands.contains(command_of(call)),
            "WebFetch" => web_host(call).is_some_and(|host| self.domains.contains(&host)),
            name if name.starts_with("mcp__") => self.tools.contains(name),
            _ => false,
        }
    }
}

fn command_of(call: &Call) -> &str {
    call.input["command"].as_str().unwrap_or_default()
}

fn web_host(call: &Call) -> Option<String> {
    url_host(call.input["url"].as_str().unwrap_or_default())
}

/// The tools Forge offers, in the shape claude.ai takes: Claude Code's,
/// the run's skills and MCP servers, and claude.ai's own web search, which
/// claude.ai runs and answers.
#[must_use]
pub fn definitions(runtime: &Runtime) -> Vec<Value> {
    let mut tools = vec![
        json!({ "type": "web_search_v0", "name": "web_search" }),
        json!({
            "name": "Read",
            "description": "Reads a file from the local filesystem. file_path is absolute. Returns up to 2000 lines, numbered like `cat -n`; pass offset (the first line, from 1) and limit for more.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "file_path": {"type": "string", "description": "The absolute path of the file to read."},
                    "offset": {"type": "integer", "description": "The line to start reading from."},
                    "limit": {"type": "integer", "description": "How many lines to read."}
                },
                "required": ["file_path"]
            }
        }),
        json!({
            "name": "Write",
            "description": "Writes a file to the local filesystem, replacing it if it exists. Read a file before replacing it.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "file_path": {"type": "string", "description": "The absolute path of the file to write."},
                    "content": {"type": "string", "description": "What the file holds."}
                },
                "required": ["file_path", "content"]
            }
        }),
        json!({
            "name": "Edit",
            "description": "Replaces exact text in a file. old_string must appear exactly once unless replace_all is true. Copy it from a Read without the line-number prefix.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "file_path": {"type": "string", "description": "The absolute path of the file to edit."},
                    "old_string": {"type": "string", "description": "The text to replace."},
                    "new_string": {"type": "string", "description": "The text to replace it with."},
                    "replace_all": {"type": "boolean", "description": "Replace every occurrence."}
                },
                "required": ["file_path", "old_string", "new_string"]
            }
        }),
        json!({
            "name": "Glob",
            "description": "Lists the files matching a glob pattern (e.g. `**/*.rs`), honouring .gitignore.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "pattern": {"type": "string", "description": "The glob to match."},
                    "path": {"type": "string", "description": "The directory to search; the working directory when left out."}
                },
                "required": ["pattern"]
            }
        }),
        json!({
            "name": "Grep",
            "description": "Searches file contents for a regular expression, honouring .gitignore. Returns `path:line:text` for each match.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "pattern": {"type": "string", "description": "The regular expression to find."},
                    "path": {"type": "string", "description": "The file or directory to search; the working directory when left out."},
                    "glob": {"type": "string", "description": "Only search files matching this glob."},
                    "-i": {"type": "boolean", "description": "Ignore case."}
                },
                "required": ["pattern"]
            }
        }),
        json!({
            "name": "Bash",
            "description": "Runs a shell command in the working directory on the user's machine and returns its output and exit code. Set run_in_background for a long-running command; its output is read with BashOutput and it is stopped with KillShell.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "command": {"type": "string", "description": "The command to run."},
                    "description": {"type": "string", "description": "What the command does, in a few words."},
                    "timeout": {"type": "integer", "description": "How long it may run, in milliseconds (at most 600000)."},
                    "run_in_background": {"type": "boolean", "description": "Run it in the background and return a shell id."}
                },
                "required": ["command"]
            }
        }),
        json!({
            "name": "AskUserQuestion",
            "description": "Asks the user a question with choices and waits for the answer.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "questions": {
                        "type": "array",
                        "items": {
                            "type": "object",
                            "properties": {
                                "question": {"type": "string"},
                                "header": {"type": "string"},
                                "multiSelect": {"type": "boolean"},
                                "options": {
                                    "type": "array",
                                    "items": {
                                        "type": "object",
                                        "properties": {
                                            "label": {"type": "string"},
                                            "description": {"type": "string"}
                                        },
                                        "required": ["label"]
                                    }
                                }
                            },
                            "required": ["question", "options"]
                        }
                    }
                },
                "required": ["questions"]
            }
        }),
        json!({
            "name": "TodoWrite",
            "description": "Records the task list for this work, replacing it each time. Keep exactly one task in_progress while you work.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "todos": {
                        "type": "array",
                        "items": {
                            "type": "object",
                            "properties": {
                                "content": {"type": "string", "description": "The task, in the imperative."},
                                "status": {"type": "string", "enum": ["pending", "in_progress", "completed", "cancelled"]},
                                "activeForm": {"type": "string", "description": "What is happening, in the present tense."}
                            },
                            "required": ["content", "status"]
                        }
                    }
                },
                "required": ["todos"]
            }
        }),
        json!({
            "name": "WebFetch",
            "description": "Fetches a public http(s) URL on the user's machine and returns its text, converted from HTML when needed; binary files are not shown. A redirect to another host is returned, not followed: fetch the new URL to read it. Prefer web_search for finding things.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "url": {"type": "string", "description": "The http(s) URL to fetch."},
                    "prompt": {"type": "string", "description": "What to find on the page; answer it from the text returned."}
                },
                "required": ["url"]
            }
        }),
        json!({
            "name": "Task",
            "description": "Runs a subagent with its own conversation and tools on one task, and returns its final report. Use it for open-ended searches or research that would fill this conversation with noise.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "description": {"type": "string", "description": "The task, in a few words."},
                    "prompt": {"type": "string", "description": "The task, in full: what to do, and what to report back."},
                    "subagent_type": {"type": "string", "description": "What kind of agent: general-purpose, explore, plan, or a custom type."},
                    "model": {"type": "string", "description": "The model the agent runs on; the thread's when left out."}
                },
                "required": ["description", "prompt"]
            }
        }),
        json!({
            "name": "BashOutput",
            "description": "Reads what a background shell (Bash with run_in_background) has printed since the last read, and whether it still runs.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "bash_id": {"type": "string", "description": "The shell id Bash returned."}
                },
                "required": ["bash_id"]
            }
        }),
        json!({
            "name": "KillShell",
            "description": "Stops a background shell and everything it started.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "shell_id": {"type": "string", "description": "The shell id Bash returned."}
                },
                "required": ["shell_id"]
            }
        }),
    ];
    if !runtime.skills.is_empty() {
        tools.push(skills::definition());
    }
    tools.extend(runtime.mcp.definitions());
    tools
}

/// The questions a call asks the user.
#[must_use]
pub fn questions(call: &Call) -> Vec<artisan_api::Question> {
    call.input["questions"]
        .as_array()
        .map(|questions| {
            questions
                .iter()
                .enumerate()
                .map(|(index, question)| {
                    let options = question["options"]
                        .as_array()
                        .map(|options| {
                            options
                                .iter()
                                .enumerate()
                                .map(|(option, choice)| artisan_api::QuestionOption {
                                    id: choice["label"]
                                        .as_str()
                                        .map_or_else(|| format!("{index}.{option}"), str::to_owned),
                                    label: choice["label"].as_str().unwrap_or_default().to_owned(),
                                    description: choice["description"]
                                        .as_str()
                                        .map(|text| text.to_owned().into()),
                                })
                                .collect::<Vec<_>>()
                                .into()
                        })
                        .unwrap_or_default();
                    artisan_api::Question {
                        id: format!("{index}"),
                        header: question["header"].as_str().map(str::to_owned),
                        text: question["question"]
                            .as_str()
                            .unwrap_or_default()
                            .to_owned()
                            .into(),
                        multi_select: question["multiSelect"].as_bool() == Some(true),
                        options,
                    }
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Whether the call runs as it is, or the user is asked first.
#[must_use]
pub fn plan(
    call: &Call,
    permissions: Permissions,
    allowed: &Allowed,
    rules: &Rules,
    mcp: &mcp::Servers,
) -> Plan {
    let name = call.name.as_str();
    if !offered(name) {
        return Plan::Refuse(format!(
            "There is no tool called {name} on the user's machine. Use one of: Read, Write, Edit, Glob, Grep, Bash, TodoWrite, WebFetch, Task, Skill, AskUserQuestion."
        ));
    }
    let verdict = rules.verdict(call);
    if let Some(Verdict::Deny(reason)) = verdict {
        return Plan::Refuse(reason);
    }
    // Full access runs everything no rule denies, `ask` rules included,
    // as Claude Code's bypassPermissions does.
    if permissions == Permissions::Full {
        return Plan::Run;
    }
    // A change to Claude Code's or Forge's own settings could grant the
    // model more on the next turn (`allow: ["Bash"]`), so it always asks:
    // no rule, "Allow always" or acceptEdits covers it.
    if rules.protects(call) {
        return Plan::Ask(ApprovalRequest::FileChange);
    }
    match verdict {
        Some(Verdict::Ask) => return Plan::Ask(request_for(call, mcp)),
        Some(Verdict::Allow) => return Plan::Run,
        Some(Verdict::Deny(_)) | None => {}
    }
    if allowed.covers(call) {
        return Plan::Run;
    }
    match name {
        "Read" | "Glob" | "Grep" | "AskUserQuestion" | "TodoWrite" | "Task" | "Skill"
        | "BashOutput" | "KillShell" => Plan::Run,
        // acceptEdits covers changes inside the project, not the machine.
        _ if changes_files(name) && rules.accept_edits() && rules.inside_root(call) => Plan::Run,
        _ => Plan::Ask(request_for(call, mcp)),
    }
}

/// Whether Forge offers a tool of that name.
fn offered(name: &str) -> bool {
    matches!(
        name,
        "Read"
            | "Write"
            | "Edit"
            | "Glob"
            | "Grep"
            | "Bash"
            | "AskUserQuestion"
            | "TodoWrite"
            | "WebFetch"
            | "Task"
            | "Skill"
            | "BashOutput"
            | "KillShell"
    ) || name.starts_with("mcp__")
}

/// What the user is shown before a call that asks.
fn request_for(call: &Call, mcp: &mcp::Servers) -> ApprovalRequest {
    match call.name.as_str() {
        "Bash" => ApprovalRequest::Command {
            command: TextContent::from(command_of(call).to_owned()),
            cwd: None,
        },
        "WebFetch" => web_request(call),
        name if changes_files(name) => ApprovalRequest::FileChange,
        name if name.starts_with("mcp__") => mcp_request(name, mcp.lookup(name)),
        other => ApprovalRequest::Action {
            description: TextContent::from(format!("Run {other}")),
        },
    }
}

/// The approval a WebFetch asks for: the site it reads, which "Allow
/// always" then covers for the run. There is no web approval card, so it
/// is an action card naming the domain and the URL.
fn web_request(call: &Call) -> ApprovalRequest {
    let url = call.input["url"].as_str().unwrap_or_default().trim();
    let site = web_host(call).unwrap_or_else(|| "an unknown site".to_owned());
    ApprovalRequest::Action {
        description: TextContent::from(format!("Fetch a page from {site}: {url}")),
    }
}

/// The approval an MCP call asks for, naming the server and the tool as
/// configured: a model-facing name may be shortened or hashed. A tool the
/// servers do not know is named from the part after the last `__`.
fn mcp_request(name: &str, known: Option<(&str, &str)>) -> ApprovalRequest {
    let (server, tool) = known.unwrap_or_else(|| {
        name.strip_prefix("mcp__")
            .and_then(|rest| rest.rsplit_once("__"))
            .unwrap_or((name, ""))
    });
    ApprovalRequest::Action {
        description: TextContent::from(format!("Run {tool} on the {server} MCP server")),
    }
}

/// The key a call's item goes under. The todo list is one item per run,
/// replaced in place, as the Claude engine keys it; a subagent's is keyed
/// under its agent when it is forwarded. Every other call is its own item.
#[must_use]
pub fn item_key(call: &Call) -> String {
    if call.name == "TodoWrite"
        && crate::claude::tools::validate_input("TodoWrite", &call.input).is_ok()
    {
        "plan".to_owned()
    } else {
        call.id.clone()
    }
}

/// The item the Editor shows for a call before it runs: what it will do,
/// from its input alone.
#[must_use]
pub fn preview(call: &Call, cwd: &Path, mcp: &mcp::Servers) -> ItemKind {
    match call.name.as_str() {
        "TodoWrite" => match crate::claude::tools::validate_input("TodoWrite", &call.input) {
            Ok(()) => ItemKind::Plan {
                steps: crate::claude::tools::todo_steps(&call.input).into(),
                proposal: None,
                approval: None,
            },
            Err(message) => failed(&message),
        },
        "Task" => agent_kind(call),
        _ => match crate::claude::tools::tool_kind(&call.name, &call.input) {
            Ok(mut tool) => {
                match &mut tool {
                    ToolKind::Shell { cwd: slot, .. } => *slot = Some(cwd.display().to_string()),
                    ToolKind::Mcp { server, tool, .. } => {
                        if let Some((known_server, known_tool)) = mcp.lookup(&call.name) {
                            known_server.clone_into(server);
                            known_tool.clone_into(tool);
                        }
                    }
                    _ => {}
                }
                ItemKind::Tool {
                    tool,
                    title: call.input["description"].as_str().map(str::to_owned),
                    output: None,
                    exit_code: None,
                    approval: None,
                }
            }
            Err(message) => failed(&message),
        },
    }
}

/// The `Agent` item a `Task` call becomes.
#[must_use]
pub fn agent_kind(call: &Call) -> ItemKind {
    ItemKind::Agent {
        engine: None,
        thread: None,
        name: call.input["subagent_type"].as_str().map(str::to_owned),
        description: call.input["description"]
            .as_str()
            .unwrap_or_default()
            .to_owned()
            .into(),
        prompt: call.input["prompt"]
            .as_str()
            .map(|prompt| prompt.to_owned().into()),
        agent_id: Some(call.id.clone()),
        context_used: None,
        model: call.input["model"].as_str().map(str::to_owned),
        background: false,
        result: None,
        label: None,
    }
}

/// Runs a call and returns what the model is told, and the item the Editor
/// shows once it ran.
pub async fn execute(
    call: &Call,
    cwd: &Path,
    cancel: &CancellationToken,
    runtime: &Runtime,
) -> (Answer, ItemKind) {
    let answer = match call.name.as_str() {
        "Read" => read(call, cwd).await,
        "Write" => write(call, cwd).await,
        "Edit" => edit(call, cwd).await,
        "Glob" => glob(call, cwd, &runtime.rules).await,
        "Grep" => grep(call, cwd, &runtime.rules).await,
        "Bash" if call.input["run_in_background"].as_bool() == Some(true) => {
            let answer = match call.input["command"].as_str() {
                Some(command) if !command.trim().is_empty() => runtime
                    .shells
                    .start(command, cwd)
                    .map_or_else(Answer::error, |id| {
                        Answer::ok(format!(
                            "Command running in the background with ID {id}. Read it with BashOutput, stop it with KillShell."
                        ))
                    }),
                _ => Answer::error("Bash needs a command."),
            };
            let item = finished(preview(call, cwd, &runtime.mcp), &answer, None);
            return (answer, item);
        }
        "Bash" => return bash(call, cwd, cancel, &runtime.mcp).await,
        "BashOutput" => call.input["bash_id"].as_str().map_or_else(
            || Answer::error("BashOutput needs the shell's bash_id."),
            |id| {
                runtime
                    .shells
                    .read(id)
                    .map_or_else(Answer::error, Answer::ok)
            },
        ),
        "KillShell" => call.input["shell_id"].as_str().map_or_else(
            || Answer::error("KillShell needs the shell's shell_id."),
            |id| {
                runtime
                    .shells
                    .kill(id)
                    .map_or_else(Answer::error, Answer::ok)
            },
        ),
        "TodoWrite" => match crate::claude::tools::validate_input("TodoWrite", &call.input) {
            Ok(()) => Answer::ok("Todo list updated."),
            Err(message) => Answer::error(message),
        },
        "WebFetch" => {
            web_fetch(call, &runtime.rules, false, &|name| {
                std::env::var(name).ok()
            })
            .await
        }
        "Skill" => match skills::load(
            &runtime.skills,
            call.input["skill"].as_str().unwrap_or_default(),
        ) {
            Ok(loaded) => Answer::ok(loaded),
            Err(message) => Answer::error(message),
        },
        name if name.starts_with("mcp__") => match runtime.mcp.find(name) {
            Some((client, tool)) => runtime
                .mcp
                .call(client, tool, &call.input)
                .await
                .map_or_else(Answer::error, Answer::ok),
            None => Answer::error(format!(
                "There is no MCP tool called {name}; the server may not have started."
            )),
        },
        other => Answer::error(format!("There is no tool called {other}.")),
    };
    let item = finished(preview(call, cwd, &runtime.mcp), &answer, None);
    (answer, item)
}

/// The preview item, finished with what the call did.
fn finished(item: ItemKind, answer: &Answer, exit_code: Option<i32>) -> ItemKind {
    let ItemKind::Tool { tool, title, .. } = item else {
        return item;
    };
    if answer.is_error && exit_code.is_none() {
        return failed(&answer.text);
    }
    ItemKind::Tool {
        tool,
        title,
        output: Some(Output::from_text(&answer.text)),
        exit_code,
        approval: None,
    }
}

/// The path a call names, resolved against the working directory.
fn resolve(cwd: &Path, path: &str) -> PathBuf {
    let candidate = Path::new(path);
    if candidate.is_absolute() {
        candidate.to_path_buf()
    } else {
        cwd.join(candidate)
    }
}

fn path_input<'a>(call: &'a Call, tool: &str) -> Result<&'a str, Answer> {
    call.input["file_path"]
        .as_str()
        .or_else(|| call.input["path"].as_str())
        .filter(|path| !path.trim().is_empty())
        .ok_or_else(|| Answer::error(format!("{tool} needs a file_path.")))
}

/// Reads a text file no larger than the limit.
async fn read_text(path: &Path) -> Result<String, Answer> {
    let metadata = tokio::fs::metadata(path)
        .await
        .map_err(|error| Answer::error(format!("Could not read {}: {error}", path.display())))?;
    if metadata.is_dir() {
        return Err(Answer::error(format!(
            "{} is a directory; use Glob or Bash `ls` to list it.",
            path.display()
        )));
    }
    if metadata.len() > MAX_FILE_BYTES {
        return Err(Answer::error(format!(
            "{} is {} MB, more than Read takes; read parts of it with Bash (e.g. sed -n).",
            path.display(),
            metadata.len() / (1024 * 1024)
        )));
    }
    let bytes = tokio::fs::read(path)
        .await
        .map_err(|error| Answer::error(format!("Could not read {}: {error}", path.display())))?;
    if bytes.iter().take(8192).any(|byte| *byte == 0) {
        return Err(Answer::error(format!(
            "{} is a binary file.",
            path.display()
        )));
    }
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

/// Reads a file, numbered like `cat -n`.
async fn read(call: &Call, cwd: &Path) -> Answer {
    let path = match path_input(call, "Read") {
        Ok(path) => resolve(cwd, path),
        Err(answer) => return answer,
    };
    let text = match read_text(&path).await {
        Ok(text) => text,
        Err(answer) => return answer,
    };
    let offset = call.input["offset"].as_u64().unwrap_or(1).max(1);
    let limit = call.input["limit"]
        .as_u64()
        .unwrap_or(DEFAULT_READ_LINES)
        .max(1);
    let mut out = String::new();
    let total = text.lines().count();
    for (number, line) in text
        .lines()
        .enumerate()
        .skip(usize::try_from(offset - 1).unwrap_or(usize::MAX))
        .take(usize::try_from(limit).unwrap_or(usize::MAX))
    {
        let line: String = if line.chars().count() > MAX_LINE_CHARS {
            line.chars()
                .take(MAX_LINE_CHARS)
                .chain("…".chars())
                .collect()
        } else {
            line.to_owned()
        };
        writeln!(out, "{:>6}\t{line}", number + 1).ok();
    }
    if out.is_empty() {
        return Answer::ok(if total == 0 {
            format!("{} is empty.", path.display())
        } else {
            format!(
                "{} has {total} lines; offset {offset} is past the end.",
                path.display()
            )
        });
    }
    let shown_to = (offset - 1).saturating_add(limit);
    if usize::try_from(shown_to).is_ok_and(|shown| shown < total) {
        writeln!(out, "[{total} lines in all; pass offset to read on]").ok();
    }
    Answer::ok(truncate(&out))
}

/// Writes a file.
async fn write(call: &Call, cwd: &Path) -> Answer {
    let path = match path_input(call, "Write") {
        Ok(path) => resolve(cwd, path),
        Err(answer) => return answer,
    };
    let Some(content) = call.input["content"].as_str() else {
        return Answer::error("Write needs the content.");
    };
    if content.len() as u64 > MAX_FILE_BYTES {
        return Answer::error("That is more than Write takes in one file.");
    }
    if let Some(parent) = path.parent()
        && let Err(error) = tokio::fs::create_dir_all(parent).await
    {
        return Answer::error(format!("Could not create {}: {error}", parent.display()));
    }
    if let Err(error) = tokio::fs::write(&path, content).await {
        return Answer::error(format!("Could not write {}: {error}", path.display()));
    }
    Answer::ok(format!(
        "Wrote {} ({} bytes).",
        path.display(),
        content.len()
    ))
}

/// Replaces exact text in a file.
async fn edit(call: &Call, cwd: &Path) -> Answer {
    let path = match path_input(call, "Edit") {
        Ok(path) => resolve(cwd, path),
        Err(answer) => return answer,
    };
    let (Some(old), Some(new)) = (
        call.input["old_string"].as_str(),
        call.input["new_string"].as_str(),
    ) else {
        return Answer::error("Edit needs an old_string and a new_string.");
    };
    if old.is_empty() {
        return Answer::error("old_string is empty; use Write to create a file.");
    }
    if old == new {
        return Answer::error("old_string and new_string are the same.");
    }
    let text = match read_text(&path).await {
        Ok(text) => text,
        Err(answer) => return answer,
    };
    let all = call.input["replace_all"].as_bool() == Some(true);
    let count = text.matches(old).count();
    if count == 0 {
        return Answer::error(format!(
            "{} does not contain old_string. Read the file again and copy the text exactly.",
            path.display()
        ));
    }
    if count > 1 && !all {
        return Answer::error(format!(
            "old_string appears {count} times in {}. Give more context to pick one, or set replace_all.",
            path.display()
        ));
    }
    let updated = if all {
        text.replace(old, new)
    } else {
        text.replacen(old, new, 1)
    };
    if let Err(error) = tokio::fs::write(&path, &updated).await {
        return Answer::error(format!("Could not write {}: {error}", path.display()));
    }
    Answer::ok(format!(
        "Edited {} ({count} replacement{}).",
        path.display(),
        if count == 1 { "" } else { "s" }
    ))
}

/// The walk Glob and Grep share: .gitignore honoured, hidden files skipped,
/// bounded.
fn walk(root: &Path) -> impl Iterator<Item = ignore::DirEntry> {
    ignore::WalkBuilder::new(root)
        .max_depth(Some(32))
        .follow_links(false)
        .require_git(false)
        .build()
        .take(MAX_WALKED)
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_some_and(|kind| kind.is_file()))
}

fn relative(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

fn matcher(pattern: &str) -> Result<globset::GlobMatcher, Answer> {
    globset::GlobBuilder::new(pattern.trim_start_matches("./"))
        .literal_separator(true)
        .build()
        .map(|glob| glob.compile_matcher())
        .map_err(|error| Answer::error(format!("That is not a valid glob: {error}")))
}

/// Lists the files matching a glob.
async fn glob(call: &Call, cwd: &Path, rules: &Rules) -> Answer {
    let Some(pattern) = call.input["pattern"].as_str() else {
        return Answer::error("Glob needs a pattern.");
    };
    let matcher = match matcher(pattern) {
        Ok(matcher) => matcher,
        Err(answer) => return answer,
    };
    let root = call.input["path"]
        .as_str()
        .map_or_else(|| cwd.to_path_buf(), |path| resolve(cwd, path));
    let rules = rules.clone();
    let found = tokio::task::spawn_blocking(move || {
        let mut found: Vec<String> = walk(&root)
            .filter(|entry| !rules.hides("Glob", entry.path()))
            .map(|entry| relative(&root, entry.path()))
            .filter(|path| matcher.is_match(path))
            .take(MAX_MATCHES + 1)
            .collect();
        found.sort();
        found
    })
    .await
    .unwrap_or_default();
    if found.is_empty() {
        return Answer::ok("No files matched.");
    }
    let more = found.len() > MAX_MATCHES;
    let mut listing = found
        .into_iter()
        .take(MAX_MATCHES)
        .collect::<Vec<_>>()
        .join("\n");
    if more {
        write!(
            listing,
            "\n[more than {MAX_MATCHES} files; narrow the pattern]"
        )
        .ok();
    }
    Answer::ok(truncate(&listing))
}

/// Searches file contents for a pattern.
async fn grep(call: &Call, cwd: &Path, rules: &Rules) -> Answer {
    let Some(pattern) = call.input["pattern"].as_str() else {
        return Answer::error("Grep needs a pattern.");
    };
    let insensitive = call.input["-i"].as_bool() == Some(true);
    let regex = match regex::RegexBuilder::new(pattern)
        .case_insensitive(insensitive)
        .build()
    {
        Ok(regex) => regex,
        Err(error) => return Answer::error(format!("That is not a valid pattern: {error}")),
    };
    let only = match call.input["glob"].as_str().map(matcher).transpose() {
        Ok(only) => only,
        Err(answer) => return answer,
    };
    let root = call.input["path"]
        .as_str()
        .map_or_else(|| cwd.to_path_buf(), |path| resolve(cwd, path));
    let rules = rules.clone();
    let hits = tokio::task::spawn_blocking(move || search(&root, &regex, only.as_ref(), &rules))
        .await
        .unwrap_or_default();
    if hits.is_empty() {
        return Answer::ok("Nothing matched.");
    }
    let more = hits.len() > MAX_MATCHES;
    let mut listing = hits
        .into_iter()
        .take(MAX_MATCHES)
        .collect::<Vec<_>>()
        .join("\n");
    if more {
        write!(
            listing,
            "\n[more than {MAX_MATCHES} matches; narrow the search]"
        )
        .ok();
    }
    Answer::ok(truncate(&listing))
}

fn search(
    root: &Path,
    regex: &Regex,
    only: Option<&globset::GlobMatcher>,
    rules: &Rules,
) -> Vec<String> {
    let mut hits = Vec::new();
    let files: Box<dyn Iterator<Item = PathBuf>> = if root.is_file() {
        Box::new(std::iter::once(root.to_path_buf()))
    } else {
        Box::new(walk(root).map(ignore::DirEntry::into_path))
    };
    let base = if root.is_file() {
        root.parent().unwrap_or(root)
    } else {
        root
    };
    for path in files {
        let name = relative(base, &path);
        if only.is_some_and(|only| !only.is_match(&name)) || rules.hides("Grep", &path) {
            continue;
        }
        if std::fs::metadata(&path).is_ok_and(|metadata| metadata.len() > MAX_FILE_BYTES) {
            continue;
        }
        let Ok(bytes) = std::fs::read(&path) else {
            continue;
        };
        if bytes.iter().take(8192).any(|byte| *byte == 0) {
            continue;
        }
        let text = String::from_utf8_lossy(&bytes);
        for (number, line) in text.lines().enumerate() {
            if regex.is_match(line) {
                let line: String = line.trim_end().chars().take(MAX_LINE_CHARS).collect();
                hits.push(format!("{name}:{}:{line}", number + 1));
                if hits.len() > MAX_MATCHES {
                    return hits;
                }
            }
        }
    }
    hits
}

/// Fetches a URL and returns its text, HTML converted to readable text.
///
/// Every hop is checked before it is connected to: the domain rules, an
/// http(s) scheme, and every address the host resolves to being public;
/// the connection is then pinned to the checked addresses, so a second
/// lookup cannot swap in a private one. Redirects are followed within a
/// site; one to another host is handed back, so its fetch is asked for
/// like any other. Behind the user's HTTP proxy the proxy connects
/// instead (see [`client_for`]). `local_ok` lets tests reach a server on
/// loopback; `vars` reads the environment's proxy settings.
async fn web_fetch(
    call: &Call,
    rules: &Rules,
    local_ok: bool,
    vars: &(dyn Fn(&str) -> Option<String> + Sync),
) -> Answer {
    let Some(url) = call.input["url"]
        .as_str()
        .map(str::trim)
        .filter(|url| !url.is_empty())
    else {
        return Answer::error("WebFetch needs a url.");
    };
    let Ok(start) = reqwest::Url::parse(url) else {
        return Answer::error(format!("That is not a URL: {url}"));
    };
    let prompt = call.input["prompt"]
        .as_str()
        .map(str::trim)
        .filter(|prompt| !prompt.is_empty());
    let mut current = start.clone();
    let mut hops = 0;
    let mut response = loop {
        if !matches!(current.scheme(), "http" | "https") {
            return Answer::error("WebFetch only fetches http and https URLs.");
        }
        let client = match client_for(&current, local_ok, vars).await {
            Ok(client) => client,
            Err(answer) => return answer,
        };
        let response = match client.get(current.clone()).send().await {
            Ok(response) => response,
            Err(error) => return Answer::error(format!("Could not fetch {current}: {error}")),
        };
        if !response.status().is_redirection() {
            break response;
        }
        let Some(next) = response
            .headers()
            .get(reqwest::header::LOCATION)
            .and_then(|location| location.to_str().ok())
            .and_then(|location| current.join(location).ok())
        else {
            break response;
        };
        hops += 1;
        if hops > MAX_REDIRECTS {
            return Answer::error(format!(
                "{url} redirected more than {MAX_REDIRECTS} times; giving up."
            ));
        }
        let hop = Call {
            id: call.id.clone(),
            name: "WebFetch".to_owned(),
            input: json!({ "url": next.as_str() }),
        };
        if let Some(Verdict::Deny(reason)) = rules.verdict(&hop) {
            return Answer::error(format!(
                "{url} redirects to {next}, which is refused. {reason}"
            ));
        }
        if !same_site(&current, &next) {
            return Answer::ok(format!(
                "{url} redirects to another host: {next}\nFetch that URL with WebFetch to read it."
            ));
        }
        current = next;
    };
    let status = response.status();
    if !status.is_success() {
        let body = read_body(&mut response, 4096)
            .await
            .map(|(body, _)| String::from_utf8_lossy(&body).into_owned())
            .unwrap_or_default();
        return Answer::error(format!(
            "The server answered {status}{}",
            if body.trim().is_empty() {
                String::new()
            } else {
                format!(": {}", body.trim())
            }
        ));
    }
    let (media, charset) = media_type(
        response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default(),
    );
    let declared = body_kind(&media);
    if declared == Some(Body::Binary) {
        let size = response.content_length().map_or_else(
            || "size unknown".to_owned(),
            |length| format!("{length} bytes"),
        );
        return binary(&current, &media, &size);
    }
    let (bytes, cut) = match read_body(&mut response, MAX_WEB_BYTES).await {
        Ok(read) => read,
        Err((read, error)) => {
            return Answer::error(format!(
                "The page at {current} broke off after {read} bytes ({error}). Nothing is shown, as it would be incomplete."
            ));
        }
    };
    let kind = match declared {
        _ if sniffs_binary(&bytes) => Body::Binary,
        Some(kind) => kind,
        None if looks_like_html(&bytes) => Body::Html,
        None => Body::Text,
    };
    let text = match kind {
        Body::Binary => {
            let size = if cut {
                format!("more than {} MB", MAX_WEB_BYTES / (1024 * 1024))
            } else {
                format!("{} bytes", bytes.len())
            };
            let media = if media.is_empty() {
                "no type given"
            } else {
                &media
            };
            return binary(&current, media, &size);
        }
        Body::Html => html_to_text(&decode(&bytes, charset.as_deref(), true)),
        Body::Text => decode(&bytes, charset.as_deref(), false),
    };
    let mut answer = if current == start {
        format!("Fetched {url}\n")
    } else {
        format!("Fetched {url} (redirected to {current})\n")
    };
    if let Some(prompt) = prompt {
        writeln!(
            answer,
            "Requested: {prompt}\nAnswer that from the page below."
        )
        .ok();
    }
    if cut {
        writeln!(
            answer,
            "[truncated: the page is more than {} MB; only its start was read]",
            MAX_WEB_BYTES / (1024 * 1024)
        )
        .ok();
    }
    answer.push('\n');
    answer.push_str(text.trim());
    Answer::ok(truncate(&answer))
}

/// What a fetched body is read as.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Body {
    Html,
    Text,
    Binary,
}

/// The refusal for a body that is not text.
fn binary(url: &reqwest::Url, media: &str, size: &str) -> Answer {
    Answer::error(format!(
        "{url} is binary content ({media}, {size}), not shown. WebFetch reads web pages and text."
    ))
}

/// A Content-Type's media type, lowercased, and its charset.
fn media_type(content_type: &str) -> (String, Option<String>) {
    let mut parts = content_type.split(';');
    let media = parts.next().unwrap_or_default().trim().to_ascii_lowercase();
    let charset = parts.find_map(|part| {
        let (key, value) = part.split_once('=')?;
        key.trim()
            .eq_ignore_ascii_case("charset")
            .then(|| value.trim().trim_matches(['"', '\'']).to_owned())
    });
    (media, charset)
}

/// How a media type is read; `None` when none was given.
fn body_kind(media: &str) -> Option<Body> {
    if media.is_empty() {
        return None;
    }
    if matches!(media, "text/html" | "application/xhtml+xml") {
        return Some(Body::Html);
    }
    let text = media.starts_with("text/")
        || media.ends_with("+json")
        || media.ends_with("+xml")
        || matches!(
            media,
            "application/json"
                | "application/xml"
                | "application/javascript"
                | "application/ecmascript"
                | "application/x-javascript"
                | "application/yaml"
                | "application/x-yaml"
                | "application/toml"
                | "application/x-sh"
                | "application/sql"
                | "application/graphql"
        );
    Some(if text { Body::Text } else { Body::Binary })
}

/// Whether a body is binary whatever it was labelled: a NUL early on, or
/// the magic of a PDF, an image or an archive.
fn sniffs_binary(bytes: &[u8]) -> bool {
    const MAGIC: &[&[u8]] = &[
        b"%PDF-",
        b"\x89PNG",
        b"GIF8",
        b"\xFF\xD8\xFF",
        b"PK\x03\x04",
        b"\x1F\x8B",
        b"RIFF",
    ];
    bytes.iter().take(8192).any(|byte| *byte == 0)
        || MAGIC.iter().any(|magic| bytes.starts_with(magic))
}

fn looks_like_html(bytes: &[u8]) -> bool {
    let head = String::from_utf8_lossy(&bytes[..bytes.len().min(512)]).to_ascii_lowercase();
    let head = head.trim_start();
    head.starts_with("<!doctype html") || head.starts_with("<html")
}

/// A body decoded from the charset its header names, or for HTML its
/// `<meta charset>`, else UTF-8 (a byte-order mark wins over both).
fn decode(bytes: &[u8], charset: Option<&str>, html: bool) -> String {
    let label = charset
        .map(str::to_owned)
        .or_else(|| html.then(|| meta_charset(bytes)).flatten());
    let encoding = label
        .and_then(|label| encoding_rs::Encoding::for_label(label.trim().as_bytes()))
        .unwrap_or(encoding_rs::UTF_8);
    encoding.decode(bytes).0.into_owned()
}

/// The charset a page's `<meta>` tags name in its first few KB.
fn meta_charset(bytes: &[u8]) -> Option<String> {
    let head = String::from_utf8_lossy(&bytes[..bytes.len().min(4096)]).to_ascii_lowercase();
    let mut rest = head.as_str();
    while let Some(start) = rest.find("<meta") {
        let tag = &rest[start..];
        let tag = &tag[..tag.find('>').unwrap_or(tag.len())];
        if let Some(at) = tag.find("charset=") {
            let label: String = tag[at + "charset=".len()..]
                .trim_start_matches(['"', '\'', ' '])
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | ':' | '.'))
                .collect();
            if !label.is_empty() {
                return Some(label);
            }
        }
        rest = &rest[start + "<meta".len()..];
    }
    None
}

/// Whether a redirect stays on the site: the same host, give or take
/// `www.`.
fn same_site(from: &reqwest::Url, to: &reqwest::Url) -> bool {
    let site = |url: &reqwest::Url| {
        url_host(url.as_str()).map(|host| host.strip_prefix("www.").unwrap_or(&host).to_owned())
    };
    site(from).is_some() && site(from) == site(to)
}

/// A client for one hop: its host resolved here, every address checked,
/// and the connection pinned to those addresses, with no proxy, which
/// would resolve the name again.
///
/// When the environment's proxy applies to the URL (`HTTPS_PROXY`,
/// `HTTP_PROXY` or `ALL_PROXY`, less `NO_PROXY`), the hop goes through it,
/// or users behind one could not fetch at all. The proxy resolves the host
/// and connects, so the connection cannot be pinned; the host is still
/// checked where it resolves here, and an address literal always is.
async fn client_for(
    url: &reqwest::Url,
    local_ok: bool,
    vars: &(dyn Fn(&str) -> Option<String> + Sync),
) -> Result<reqwest::Client, Answer> {
    let Some(host) = url.host_str() else {
        return Err(Answer::error(format!("{url} names no host.")));
    };
    let port = url.port_or_known_default().unwrap_or(80);
    let literal = host
        .strip_prefix('[')
        .and_then(|host| host.strip_suffix(']'))
        .unwrap_or(host)
        .parse::<IpAddr>()
        .ok();
    let proxy = proxy_for(url, vars);
    let addresses: Vec<SocketAddr> = if let Some(ip) = literal {
        vec![SocketAddr::new(ip, port)]
    } else {
        let target = (host.to_owned(), port);
        let lookup = tokio::task::spawn_blocking(move || {
            target.to_socket_addrs().map(Iterator::collect::<Vec<_>>)
        });
        match tokio::time::timeout(Duration::from_secs(10), lookup).await {
            Ok(Ok(Ok(addresses))) => addresses,
            // Behind a proxy the name may only resolve on the proxy's side.
            _ if proxy.is_some() => Vec::new(),
            Ok(Ok(Err(error))) => {
                return Err(Answer::error(format!("Could not resolve {host}: {error}")));
            }
            _ => return Err(Answer::error(format!("Could not resolve {host} in time."))),
        }
    };
    if addresses.is_empty() && proxy.is_none() {
        return Err(Answer::error(format!("{host} resolves to no address.")));
    }
    if !local_ok && let Some(address) = addresses.iter().find(|address| !is_public(address.ip())) {
        return Err(Answer::error(format!(
            "WebFetch does not fetch local or private addresses: {host} is {}.",
            address.ip()
        )));
    }
    let mut builder = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(15))
        .timeout(WEB_TIMEOUT)
        .redirect(reqwest::redirect::Policy::none())
        .user_agent(format!("artisan/{}", env!("CARGO_PKG_VERSION")));
    if let Some(proxy) = proxy {
        let proxy = reqwest::Proxy::all(&proxy)
            .map_err(|error| Answer::error(format!("The proxy {proxy} is not usable: {error}")))?;
        builder = builder.proxy(proxy);
    } else {
        builder = builder.no_proxy();
        if literal.is_none() {
            builder = builder.resolve_to_addrs(host, &addresses);
        }
    }
    builder
        .build()
        .map_err(|error| Answer::error(format!("Could not fetch: {error}")))
}

/// The proxy the environment sets for `url`, unless `NO_PROXY` exempts its
/// host: the scheme's own variable first, then `ALL_PROXY`, each in either
/// case.
fn proxy_for(url: &reqwest::Url, vars: &(dyn Fn(&str) -> Option<String> + Sync)) -> Option<String> {
    let var = |names: &[&str]| {
        names
            .iter()
            .find_map(|name| vars(name).filter(|value| !value.trim().is_empty()))
    };
    let proxy = match url.scheme() {
        "https" => var(&["HTTPS_PROXY", "https_proxy"]),
        _ => var(&["HTTP_PROXY", "http_proxy"]),
    }
    .or_else(|| var(&["ALL_PROXY", "all_proxy"]))?;
    let host = url.host_str()?;
    let host = host
        .strip_prefix('[')
        .and_then(|host| host.strip_suffix(']'))
        .unwrap_or(host)
        .to_ascii_lowercase();
    let exempt = var(&["NO_PROXY", "no_proxy"]).unwrap_or_default();
    (!exempts(&exempt, &host)).then(|| proxy.trim().to_owned())
}

/// Whether a `NO_PROXY` list names a host: `*`, the host or a domain it is
/// under (`example.com`, `.example.com`, `*.example.com`), an address, or
/// a network holding it (`10.0.0.0/8`).
fn exempts(list: &str, host: &str) -> bool {
    let address = host.parse::<IpAddr>().ok();
    list.split(',')
        .map(str::trim)
        .filter(|entry| !entry.is_empty())
        .any(|entry| {
            let entry = entry.to_ascii_lowercase();
            if entry == "*" {
                return true;
            }
            if let Some(address) = address {
                if let Some((network, bits)) = entry.split_once('/') {
                    let (Ok(network), Ok(bits)) = (network.parse::<IpAddr>(), bits.parse::<u32>())
                    else {
                        return false;
                    };
                    return in_network(address, network, bits);
                }
                return entry
                    .trim_start_matches('[')
                    .trim_end_matches(']')
                    .parse::<IpAddr>()
                    .is_ok_and(|entry| entry == address);
            }
            let domain = entry.trim_start_matches("*.").trim_start_matches('.');
            host == domain
                || host
                    .strip_suffix(domain)
                    .is_some_and(|rest| rest.ends_with('.'))
        })
}

/// Whether `address` is in the network `network/bits`.
fn in_network(address: IpAddr, network: IpAddr, bits: u32) -> bool {
    match (address, network) {
        (IpAddr::V4(address), IpAddr::V4(network)) if bits <= 32 => {
            let mask = u32::MAX.checked_shl(32 - bits).unwrap_or(0);
            u32::from(address) & mask == u32::from(network) & mask
        }
        (IpAddr::V6(address), IpAddr::V6(network)) if bits <= 128 => {
            let mask = u128::MAX.checked_shl(128 - bits).unwrap_or(0);
            u128::from(address) & mask == u128::from(network) & mask
        }
        _ => false,
    }
}

/// Whether an address is on the public internet: not loopback, private,
/// link-local, unique-local, shared, unspecified, multicast or reserved,
/// also when an IPv6 address carries an IPv4 one.
fn is_public(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            let [a, b, ..] = v4.octets();
            !(v4.is_loopback()
                || v4.is_private()
                || v4.is_link_local()
                || v4.is_unspecified()
                || v4.is_multicast()
                || v4.is_broadcast()
                || a == 0
                || (a == 100 && (b & 0xC0) == 64)
                || a >= 240)
        }
        IpAddr::V6(v6) => {
            if let Some(v4) = v6.to_ipv4_mapped() {
                return is_public(IpAddr::V4(v4));
            }
            let segments = v6.segments();
            // IPv4-compatible (`::a.b.c.d`) and NAT64 (`64:ff9b::a.b.c.d`).
            if segments[..6] == [0; 6] || segments[..6] == [0x64, 0xff9b, 0, 0, 0, 0] {
                let [.., high, low] = segments;
                let v4 = std::net::Ipv4Addr::from((u32::from(high) << 16) | u32::from(low));
                if !v6.is_loopback() && !v6.is_unspecified() {
                    return is_public(IpAddr::V4(v4));
                }
            }
            !(v6.is_loopback()
                || v6.is_unspecified()
                || v6.is_multicast()
                || (segments[0] & 0xffc0) == 0xfe80
                || (segments[0] & 0xffc0) == 0xfec0
                || (segments[0] & 0xfe00) == 0xfc00)
        }
    }
}

/// Reads a body up to `limit` bytes: the bytes, and whether more were
/// left. A read that fails part-way is an error, with how much arrived, so
/// a cut-off page is never taken for a whole one.
async fn read_body(
    response: &mut reqwest::Response,
    limit: usize,
) -> Result<(Vec<u8>, bool), (usize, reqwest::Error)> {
    let mut kept = Vec::new();
    loop {
        match response.chunk().await {
            Ok(Some(chunk)) => {
                let room = limit.saturating_sub(kept.len());
                if chunk.len() > room {
                    kept.extend_from_slice(&chunk[..room]);
                    return Ok((kept, true));
                }
                kept.extend_from_slice(&chunk);
            }
            Ok(None) => return Ok((kept, false)),
            Err(error) => return Err((kept.len(), error)),
        }
    }
}

/// HTML as readable text: scripts and styles dropped, block tags as line
/// breaks, entities decoded, blank runs collapsed.
fn html_to_text(html: &str) -> String {
    let mut without = html.to_owned();
    for tag in ["script", "style", "noscript", "template"] {
        without = strip_blocks(&without, tag);
    }
    let mut out = String::with_capacity(without.len());
    let mut chars = without.chars();
    while let Some(character) = chars.next() {
        if character != '<' {
            out.push(character);
            continue;
        }
        let mut tag = String::new();
        for character in chars.by_ref() {
            if character == '>' {
                break;
            }
            tag.push(character);
        }
        let name = tag
            .trim_start_matches('/')
            .split(|c: char| c.is_whitespace() || c == '/')
            .next()
            .unwrap_or_default()
            .to_ascii_lowercase();
        if name == "br" || BLOCK_TAGS.contains(&name.as_str()) {
            out.push('\n');
        } else {
            out.push(' ');
        }
    }
    collapse(&decode_entities(&out))
}

/// The tags that end a line of text.
const BLOCK_TAGS: &[&str] = &[
    "p",
    "div",
    "section",
    "article",
    "header",
    "footer",
    "main",
    "aside",
    "nav",
    "ul",
    "ol",
    "li",
    "h1",
    "h2",
    "h3",
    "h4",
    "h5",
    "h6",
    "table",
    "tr",
    "td",
    "th",
    "thead",
    "tbody",
    "blockquote",
    "pre",
    "figure",
    "figcaption",
    "form",
    "fieldset",
    "hr",
    "title",
    "meta",
];

/// Removes `<tag ...>…</tag>` blocks, case-insensitively.
fn strip_blocks(html: &str, tag: &str) -> String {
    let lower = html.to_ascii_lowercase();
    let open = format!("<{tag}");
    let close = format!("</{tag}");
    let mut out = String::with_capacity(html.len());
    let mut at = 0;
    while let Some(start) = lower[at..].find(&open) {
        let start = at + start;
        out.push_str(&html[at..start]);
        let Some(end) = lower[start..].find(&close) else {
            return out;
        };
        let end = start + end;
        let Some(gt) = lower[end..].find('>') else {
            return out;
        };
        at = end + gt + 1;
    }
    out.push_str(&html[at..]);
    out
}

/// The common named entities, and numeric ones.
fn decode_entities(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find('&') {
        out.push_str(&rest[..start]);
        let Some(end) = rest[start..].find(';').filter(|end| *end <= 12) else {
            out.push_str(&rest[start..=start]);
            rest = &rest[start + 1..];
            continue;
        };
        let entity = &rest[start + 1..start + end];
        let decoded = match entity {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" | "#39" => Some('\''),
            "nbsp" | "#160" => Some(' '),
            "mdash" | "#8212" => Some('—'),
            "ndash" | "#8211" => Some('–'),
            "hellip" | "#8230" => Some('…'),
            "rsquo" | "#8217" => Some('\u{2019}'),
            "lsquo" | "#8216" => Some('\u{2018}'),
            "ldquo" | "#8220" => Some('\u{201C}'),
            "rdquo" | "#8221" => Some('\u{201D}'),
            _ => entity
                .strip_prefix("#x")
                .or_else(|| entity.strip_prefix("#X"))
                .and_then(|hex| u32::from_str_radix(hex, 16).ok())
                .or_else(|| {
                    entity
                        .strip_prefix('#')
                        .and_then(|decimal| decimal.parse::<u32>().ok())
                })
                .and_then(char::from_u32),
        };
        match decoded {
            Some(character) => out.push(character),
            None => out.push_str(&rest[start..=start + end]),
        }
        rest = &rest[start + end + 1..];
    }
    out.push_str(rest);
    out
}

/// Collapses blank runs, trailing whitespace and runs of spaces.
fn collapse(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut blanks = 0usize;
    for line in text.lines() {
        let mut collapsed = String::with_capacity(line.len());
        let mut space = false;
        for character in line.trim_end().chars() {
            if character == ' ' || character == '\t' {
                space = true;
                continue;
            }
            if space && !collapsed.is_empty() {
                collapsed.push(' ');
            }
            space = false;
            collapsed.push(character);
        }
        if collapsed.trim().is_empty() {
            blanks += 1;
            if blanks > 1 {
                continue;
            }
        } else {
            blanks = 0;
        }
        out.push_str(&collapsed);
        out.push('\n');
    }
    out.trim().to_owned()
}

/// Runs a shell command in its own process group (a job object on
/// Windows), so a timeout or a stop ends everything it started.
async fn bash(
    call: &Call,
    cwd: &Path,
    cancel: &CancellationToken,
    mcp: &mcp::Servers,
) -> (Answer, ItemKind) {
    let item = preview(call, cwd, mcp);
    let command = command_of(call);
    if command.trim().is_empty() {
        let answer = Answer::error("Bash needs a command.");
        return (answer.clone(), failed(&answer.text));
    }
    let timeout = call.input["timeout"]
        .as_u64()
        .map_or(DEFAULT_COMMAND_TIMEOUT, Duration::from_millis)
        .min(MAX_COMMAND_TIMEOUT);
    let mut shell = if cfg!(windows) {
        let mut shell = tokio::process::Command::new("cmd");
        shell.arg("/C").arg(command);
        shell
    } else {
        let mut shell = tokio::process::Command::new("sh");
        shell.arg("-c").arg(command);
        shell
    };
    shell
        .current_dir(cwd)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    let mut child = match crate::process::owned_command(shell) {
        Ok(child) => child,
        Err(error) => {
            let answer = Answer::error(format!("The command could not start: {error:#}"));
            return (answer.clone(), failed(&answer.text));
        }
    };
    let stdout = child.stdout().take();
    let stderr = child.stderr().take();
    let reading = async {
        let (out, err) = tokio::join!(capped(stdout), capped(stderr));
        (out, err)
    };
    let waited = async {
        let output = reading.await;
        let status = child.wait().await;
        (output, status)
    };
    let ended = tokio::select! {
        ended = tokio::time::timeout(timeout, waited) => ended.ok(),
        () = cancel.cancelled() => None,
    };
    let Some(((out, err), status)) = ended else {
        let note = if cancel.is_cancelled() {
            "The command was stopped.".to_owned()
        } else {
            format!(
                "The command ran for more than {} seconds and was stopped.",
                timeout.as_secs()
            )
        };
        let answer = Answer::error(note);
        return (answer.clone(), finished(item, &answer, Some(-1)));
    };
    let code = status.ok().and_then(|status| status.code()).unwrap_or(-1);
    let mut text = out;
    if !err.is_empty() {
        if !text.is_empty() && !text.ends_with('\n') {
            text.push('\n');
        }
        text.push_str(&err);
    }
    let mut answer = truncate(&text);
    if code != 0 {
        if !answer.is_empty() && !answer.ends_with('\n') {
            answer.push('\n');
        }
        write!(answer, "[exit code {code}]").ok();
    }
    if answer.is_empty() {
        answer.push_str("(no output)");
    }
    let item = finished(item, &Answer::ok(truncate(&text)), Some(code));
    (
        Answer {
            text: answer,
            is_error: code != 0,
        },
        item,
    )
}

/// Reads a stream to its end, keeping at most the output limit of it.
async fn capped(stream: Option<impl tokio::io::AsyncRead + Unpin>) -> String {
    let Some(mut stream) = stream else {
        return String::new();
    };
    let mut kept = Vec::new();
    let mut dropped = 0usize;
    let mut buffer = vec![0u8; 16 * 1024];
    loop {
        match stream.read(&mut buffer).await {
            Ok(0) | Err(_) => break,
            Ok(read) => {
                let room = MAX_OUTPUT_BYTES.saturating_sub(kept.len());
                kept.extend_from_slice(&buffer[..read.min(room)]);
                dropped += read.saturating_sub(room);
            }
        }
    }
    let mut text = shells::Decoder::new(shells::legacy_encoding().await).push(&kept, true);
    if dropped > 0 {
        write!(text, "\n[{dropped} more bytes of output cut]").ok();
    }
    text
}

/// An error item for a tool that could not do its work.
fn failed(message: &str) -> ItemKind {
    ItemKind::Error {
        kind: ErrorKind::Execution,
        message: message.to_owned().into(),
        retryable: false,
    }
}

/// Cuts a tool's output to what the model can read.
fn truncate(text: &str) -> String {
    if text.len() <= MAX_OUTPUT_BYTES {
        return text.to_owned();
    }
    let mut end = MAX_OUTPUT_BYTES;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}\n[output cut]", &text[..end])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn call(name: &str, input: Value) -> Call {
        Call {
            id: "t1".to_owned(),
            name: name.to_owned(),
            input,
        }
    }

    /// [`super::plan`] with no MCP servers.
    fn plan(call: &Call, permissions: Permissions, allowed: &Allowed, rules: &Rules) -> Plan {
        super::plan(call, permissions, allowed, rules, &mcp::Servers::default())
    }

    /// [`super::preview`] with no MCP servers.
    fn preview(call: &Call, cwd: &Path) -> ItemKind {
        super::preview(call, cwd, &mcp::Servers::default())
    }

    /// [`super::web_fetch`] in an environment that sets no proxy.
    async fn web_fetch(call: &Call, rules: &Rules, local_ok: bool) -> Answer {
        super::web_fetch(call, rules, local_ok, &|_| None).await
    }

    fn runtime() -> Runtime {
        Runtime {
            skills: Vec::new(),
            mcp: mcp::Servers::default(),
            shells: shells::Shells::default(),
            rules: Rules::default(),
        }
    }

    #[test]
    fn a_read_runs_and_a_change_asks_unless_full_or_always_allowed() {
        let allowed = Allowed::default();
        let rules = Rules::default();
        let read = call("Read", json!({ "file_path": "/x" }));
        assert_eq!(plan(&read, Permissions::Auto, &allowed, &rules), Plan::Run);
        let write = call("Write", json!({ "file_path": "/x", "content": "y" }));
        assert_eq!(
            plan(&write, Permissions::Auto, &allowed, &rules),
            Plan::Ask(ApprovalRequest::FileChange)
        );
        assert_eq!(plan(&write, Permissions::Full, &allowed, &rules), Plan::Run);
        let mut allowed = Allowed::default();
        allowed.remember(&write, Decision::AllowAlways);
        assert_eq!(plan(&write, Permissions::Auto, &allowed, &rules), Plan::Run);
        let bash = call("Bash", json!({ "command": "ls" }));
        assert!(matches!(
            plan(&bash, Permissions::Auto, &allowed, &rules),
            Plan::Ask(_)
        ));
        allowed.remember(&bash, Decision::AllowOnce);
        assert!(matches!(
            plan(&bash, Permissions::Auto, &allowed, &rules),
            Plan::Ask(_)
        ));
        allowed.remember(&bash, Decision::AllowAlways);
        assert_eq!(plan(&bash, Permissions::Auto, &allowed, &rules), Plan::Run);
    }

    #[test]
    fn mcp_calls_ask_under_auto_and_run_under_full_or_once_allowed() {
        let rules = Rules::default();
        let tool = call("mcp__docs__search", json!({ "query": "x" }));
        assert!(matches!(
            plan(&tool, Permissions::Auto, &Allowed::default(), &rules),
            Plan::Ask(_)
        ));
        assert_eq!(
            plan(&tool, Permissions::Full, &Allowed::default(), &rules),
            Plan::Run
        );
        let mut allowed = Allowed::default();
        allowed.remember(&tool, Decision::AllowAlways);
        assert_eq!(plan(&tool, Permissions::Auto, &allowed, &rules), Plan::Run);
    }

    #[test]
    fn todo_and_task_run_without_asking_and_keep_their_items() {
        let rules = Rules::default();
        let allowed = Allowed::default();
        let todo = call(
            "TodoWrite",
            json!({ "todos": [
                { "content": "Read the code", "status": "in_progress" },
                { "content": "Write the fix", "status": "pending" }
            ] }),
        );
        assert_eq!(plan(&todo, Permissions::Auto, &allowed, &rules), Plan::Run);
        let ItemKind::Plan { steps, .. } = preview(&todo, Path::new("/")) else {
            panic!("a plan item");
        };
        let steps = steps.inline().expect("inline steps");
        assert_eq!(steps.len(), 2);
        assert_eq!(steps[0].status, artisan_api::StepStatus::InProgress);
        let task = call(
            "Task",
            json!({ "description": "Find things", "prompt": "Find the things." }),
        );
        assert_eq!(plan(&task, Permissions::Auto, &allowed, &rules), Plan::Run);
        let ItemKind::Agent {
            description,
            prompt,
            ..
        } = preview(&task, Path::new("/"))
        else {
            panic!("an agent item");
        };
        assert_eq!(description.inline(), Some("Find things"));
        assert_eq!(
            prompt.and_then(|prompt| prompt.inline().map(str::to_owned)),
            Some("Find the things.".to_owned())
        );
    }

    #[test]
    fn a_malformed_todo_preview_is_an_error_not_a_panic() {
        let item = preview(
            &call("TodoWrite", json!({ "todos": [{ "status": "pending" }] })),
            Path::new("/"),
        );
        assert!(matches!(item, ItemKind::Error { .. }), "{item:?}");
    }

    #[test]
    fn accept_edits_runs_writes_without_asking() {
        let home = tempfile::tempdir().expect("a home");
        let project = tempfile::tempdir().expect("a project");
        let dir = project.path().join(".claude");
        std::fs::create_dir_all(&dir).expect("the settings dir");
        std::fs::write(
            dir.join("settings.json"),
            r#"{"permissions": {"defaultMode": "acceptEdits"}}"#,
        )
        .expect("settings");
        let rules = Rules::load(home.path(), project.path());
        let write = call("Write", json!({ "file_path": "x", "content": "y" }));
        assert_eq!(
            plan(&write, Permissions::Auto, &Allowed::default(), &rules),
            Plan::Run
        );
    }

    #[test]
    fn a_tool_forge_does_not_offer_is_refused_with_the_ones_it_does() {
        let Plan::Refuse(reason) = plan(
            &call("bash_tool", json!({})),
            Permissions::Full,
            &Allowed::default(),
            &Rules::default(),
        ) else {
            panic!("refused");
        };
        assert!(reason.contains("Bash"));
    }

    #[test]
    fn html_becomes_readable_text() {
        let html = "<html><head><title>T</title><style>p { color: red }</style></head>\
                    <body><h1>Hello &amp; welcome</h1><p>One <b>two</b> three.</p>\
                    <script>alert('no')</script><ul><li>a</li><li>b</li></ul></body></html>";
        let text = html_to_text(html);
        assert!(text.contains("Hello & welcome"), "{text}");
        assert!(text.contains("One two three."), "{text}");
        assert!(text.contains("a\n\nb"), "{text}");
        assert!(!text.contains("alert"), "{text}");
        assert!(!text.contains("color:red"), "{text}");
    }

    #[test]
    fn a_preview_shows_the_change_before_anything_runs() {
        let work = tempfile::tempdir().expect("a dir");
        let path = work.path().join("a.txt");
        let edit = call(
            "Edit",
            json!({ "file_path": path.display().to_string(), "old_string": "one", "new_string": "two" }),
        );
        let ItemKind::Tool {
            tool: ToolKind::Edit { changes },
            ..
        } = preview(&edit, work.path())
        else {
            panic!("an edit item");
        };
        assert!(
            changes
                .inline()
                .is_some_and(|changes| changes[0].diff.is_some())
        );
        assert!(!path.exists(), "a preview touches nothing");
    }

    #[tokio::test]
    async fn read_numbers_its_lines_and_edit_changes_exactly_once() {
        let work = tempfile::tempdir().expect("a dir");
        let path = work.path().join("a.txt");
        std::fs::write(&path, "one\ntwo\n").expect("a file");
        let cancel = CancellationToken::new();
        let (answer, _) = execute(
            &call("Read", json!({ "file_path": path.display().to_string() })),
            work.path(),
            &cancel,
            &runtime(),
        )
        .await;
        assert_eq!(answer.text, "     1\tone\n     2\ttwo\n");
        let edit = call(
            "Edit",
            json!({ "file_path": "a.txt", "old_string": "two", "new_string": "three" }),
        );
        let (answer, _) = execute(&edit, work.path(), &cancel, &runtime()).await;
        assert!(!answer.is_error, "{}", answer.text);
        assert_eq!(
            std::fs::read_to_string(&path).expect("the file"),
            "one\nthree\n"
        );
    }

    #[tokio::test]
    async fn glob_and_grep_honour_gitignore() {
        let work = tempfile::tempdir().expect("a dir");
        std::fs::write(work.path().join(".gitignore"), "build/\n").expect("ignore");
        std::fs::create_dir_all(work.path().join("src")).expect("src");
        std::fs::create_dir_all(work.path().join("build")).expect("build");
        std::fs::write(work.path().join("src/lib.rs"), "fn needle() {}\n").expect("lib");
        std::fs::write(work.path().join("build/out.rs"), "fn needle() {}\n").expect("out");
        let cancel = CancellationToken::new();
        let (found, _) = execute(
            &call("Glob", json!({ "pattern": "**/*.rs" })),
            work.path(),
            &cancel,
            &runtime(),
        )
        .await;
        assert_eq!(found.text, "src/lib.rs");
        let (hits, _) = execute(
            &call("Grep", json!({ "pattern": "needle" })),
            work.path(),
            &cancel,
            &runtime(),
        )
        .await;
        assert_eq!(hits.text, "src/lib.rs:1:fn needle() {}");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_failing_command_tells_the_model_its_exit_code() {
        let work = tempfile::tempdir().expect("a dir");
        let cancel = CancellationToken::new();
        let (answer, item) = execute(
            &call(
                "Bash",
                json!({ "command": "echo out; echo err >&2; exit 3" }),
            ),
            work.path(),
            &cancel,
            &runtime(),
        )
        .await;
        assert!(answer.is_error);
        assert_eq!(answer.text, "out\nerr\n[exit code 3]");
        assert!(matches!(
            item,
            ItemKind::Tool {
                exit_code: Some(3),
                ..
            }
        ));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_stop_ends_the_command_and_what_it_started() {
        let work = tempfile::tempdir().expect("a dir");
        let cancel = CancellationToken::new();
        let stopper = cancel.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(200)).await;
            stopper.cancel();
        });
        let started = std::time::Instant::now();
        let (answer, _) = execute(
            &call("Bash", json!({ "command": "sleep 30 & sleep 30" })),
            work.path(),
            &cancel,
            &runtime(),
        )
        .await;
        assert!(started.elapsed() < Duration::from_secs(5));
        assert_eq!(answer.text, "The command was stopped.");
    }

    /// Rules from a project settings file holding `permissions`.
    fn project_rules(home: &Path, project: &Path, permissions: Value) -> Rules {
        let dir = project.join(".claude");
        std::fs::create_dir_all(&dir).expect("the settings dir");
        std::fs::write(
            dir.join("settings.json"),
            json!({ "permissions": permissions }).to_string(),
        )
        .expect("settings");
        Rules::load(home, project)
    }

    #[test]
    fn a_compound_command_is_not_allowed_by_a_rule_for_its_first_part() {
        let home = tempfile::tempdir().expect("a home");
        let project = tempfile::tempdir().expect("a project");
        let rules = project_rules(
            home.path(),
            project.path(),
            json!({ "allow": ["Bash(npm run test:*)"] }),
        );
        let allowed = Allowed::default();
        let fine = call("Bash", json!({ "command": "npm run test -- --ci" }));
        assert_eq!(plan(&fine, Permissions::Auto, &allowed, &rules), Plan::Run);
        let sneaky = call("Bash", json!({ "command": "npm run test && rm -rf ~" }));
        assert_eq!(
            plan(&sneaky, Permissions::Auto, &allowed, &rules),
            Plan::Ask(ApprovalRequest::Command {
                command: TextContent::from("npm run test && rm -rf ~".to_owned()),
                cwd: None,
            })
        );
    }

    #[test]
    fn a_denied_command_is_refused_even_under_full_access_however_it_is_dressed() {
        let home = tempfile::tempdir().expect("a home");
        let project = tempfile::tempdir().expect("a project");
        let rules = project_rules(
            home.path(),
            project.path(),
            json!({ "deny": ["Bash(rm:*)"] }),
        );
        for command in [
            " rm -rf /",
            "FOO=1 rm -rf /",
            "true && rm -rf /",
            "cd /; rm -rf /",
            "(rm -rf /)",
            "bash -c 'rm -rf /'",
            "/bin/rm -rf /",
        ] {
            let bash = call("Bash", json!({ "command": command }));
            assert!(
                matches!(
                    plan(&bash, Permissions::Full, &Allowed::default(), &rules),
                    Plan::Refuse(_)
                ),
                "{command:?} would have run"
            );
        }
    }

    #[test]
    fn always_allowing_a_command_does_not_skip_an_ask_rule() {
        let home = tempfile::tempdir().expect("a home");
        let project = tempfile::tempdir().expect("a project");
        let rules = project_rules(
            home.path(),
            project.path(),
            json!({ "ask": ["Bash(git push:*)"] }),
        );
        let push = call("Bash", json!({ "command": "git push --force" }));
        let mut allowed = Allowed::default();
        allowed.remember(&push, Decision::AllowAlways);
        assert!(matches!(
            plan(&push, Permissions::Auto, &allowed, &rules),
            Plan::Ask(_)
        ));
        assert_eq!(plan(&push, Permissions::Full, &allowed, &rules), Plan::Run);
    }

    #[tokio::test]
    async fn a_search_never_shows_a_denied_file() {
        let home = tempfile::tempdir().expect("a home");
        let project = tempfile::tempdir().expect("a project");
        let rules = project_rules(
            home.path(),
            project.path(),
            json!({ "deny": ["Read(.env)", "Read(secrets/**)"] }),
        );
        std::fs::create_dir_all(project.path().join("secrets")).expect("secrets");
        std::fs::write(project.path().join(".env"), "TOKEN=hunter2\n").expect("env");
        std::fs::write(project.path().join("secrets/key"), "TOKEN=hunter3\n").expect("key");
        std::fs::write(project.path().join("app.rs"), "// TOKEN=placeholder\n").expect("app");
        let grep = call("Grep", json!({ "pattern": "TOKEN", "path": ".env" }));
        assert!(matches!(
            plan(&grep, Permissions::Full, &Allowed::default(), &rules),
            Plan::Refuse(_)
        ));
        let mut runtime = runtime();
        runtime.rules = rules;
        let cancel = CancellationToken::new();
        let (hits, _) = execute(
            &call("Grep", json!({ "pattern": "TOKEN" })),
            project.path(),
            &cancel,
            &runtime,
        )
        .await;
        assert_eq!(hits.text, "app.rs:1:// TOKEN=placeholder");
        let (found, _) = execute(
            &call("Glob", json!({ "pattern": "**/*" })),
            project.path(),
            &cancel,
            &runtime,
        )
        .await;
        assert!(!found.text.contains("secrets/key"), "{}", found.text);
    }

    #[test]
    fn accept_edits_asks_for_a_change_outside_the_project() {
        let home = tempfile::tempdir().expect("a home");
        let project = tempfile::tempdir().expect("a project");
        let elsewhere = tempfile::tempdir().expect("elsewhere");
        let rules = project_rules(
            home.path(),
            project.path(),
            json!({ "defaultMode": "acceptEdits" }),
        );
        let allowed = Allowed::default();
        let inside = call(
            "Edit",
            json!({ "file_path": "src/../a.txt", "old_string": "a", "new_string": "b" }),
        );
        assert_eq!(
            plan(&inside, Permissions::Auto, &allowed, &rules),
            Plan::Run
        );
        for path in [
            elsewhere.path().join("a.txt").display().to_string(),
            "../escape.txt".to_owned(),
        ] {
            let outside = call("Write", json!({ "file_path": path, "content": "x" }));
            assert_eq!(
                plan(&outside, Permissions::Auto, &allowed, &rules),
                Plan::Ask(ApprovalRequest::FileChange),
                "{path}"
            );
        }
    }

    #[test]
    fn a_change_to_permission_settings_always_asks_short_of_full_access() {
        let home = tempfile::tempdir().expect("a home");
        let project = tempfile::tempdir().expect("a project");
        let rules = project_rules(
            home.path(),
            project.path(),
            json!({ "defaultMode": "acceptEdits", "allow": ["Edit", "Write"] }),
        );
        let mut allowed = Allowed::default();
        allowed.remember(
            &call("Write", json!({ "file_path": "a.txt", "content": "" })),
            Decision::AllowAlways,
        );
        for path in [
            ".claude/settings.json".to_owned(),
            ".claude/settings.local.json".to_owned(),
            "sub/.Claude/settings.json".to_owned(),
            ".mcp.json".to_owned(),
            home.path()
                .join(".claude/settings.json")
                .display()
                .to_string(),
            home.path().join("session.json").display().to_string(),
        ] {
            let write = call(
                "Write",
                json!({ "file_path": path, "content": r#"{"permissions":{"allow":["Bash"]}}"# }),
            );
            assert_eq!(
                plan(&write, Permissions::Auto, &allowed, &rules),
                Plan::Ask(ApprovalRequest::FileChange),
                "{path}"
            );
            assert_eq!(plan(&write, Permissions::Full, &allowed, &rules), Plan::Run);
        }
        let ordinary = call("Write", json!({ "file_path": "a.txt", "content": "" }));
        assert_eq!(
            plan(&ordinary, Permissions::Auto, &allowed, &rules),
            Plan::Run
        );
    }

    #[test]
    fn web_fetch_asks_per_site_under_auto() {
        let rules = Rules::default();
        let page = call("WebFetch", json!({ "url": "https://Docs.Example.com/a" }));
        assert_eq!(
            plan(&page, Permissions::Auto, &Allowed::default(), &rules),
            Plan::Ask(ApprovalRequest::Action {
                description: TextContent::from(
                    "Fetch a page from docs.example.com: https://Docs.Example.com/a".to_owned()
                ),
            })
        );
        assert_eq!(
            plan(&page, Permissions::Full, &Allowed::default(), &rules),
            Plan::Run
        );
        let mut allowed = Allowed::default();
        allowed.remember(&page, Decision::AllowAlways);
        let same = call("WebFetch", json!({ "url": "https://docs.example.com/b" }));
        assert_eq!(plan(&same, Permissions::Auto, &allowed, &rules), Plan::Run);
        let other = call("WebFetch", json!({ "url": "https://other.example/b" }));
        assert!(matches!(
            plan(&other, Permissions::Auto, &allowed, &rules),
            Plan::Ask(_)
        ));
        let home = tempfile::tempdir().expect("a home");
        let project = tempfile::tempdir().expect("a project");
        let rules = project_rules(
            home.path(),
            project.path(),
            json!({ "allow": ["WebFetch(domain:other.example)"] }),
        );
        assert_eq!(
            plan(&other, Permissions::Auto, &Allowed::default(), &rules),
            Plan::Run
        );
    }

    #[test]
    fn an_mcp_card_names_the_server_and_tool_as_configured() {
        // A shortened model-facing name still shows the configured names.
        assert_eq!(
            mcp_request("mcp__my_srv__search_7f3a", Some(("my__srv", "search"))),
            ApprovalRequest::Action {
                description: TextContent::from("Run search on the my__srv MCP server".to_owned()),
            }
        );
        assert_eq!(
            mcp_request("mcp__my__srv__search", None),
            ApprovalRequest::Action {
                description: TextContent::from("Run search on the my__srv MCP server".to_owned()),
            }
        );
    }

    #[test]
    fn the_environment_s_proxy_applies_unless_no_proxy_exempts_the_host() {
        let vars = |set: &'static [(&'static str, &'static str)]| {
            move |name: &str| {
                set.iter()
                    .find(|(key, _)| *key == name)
                    .map(|(_, value)| (*value).to_owned())
            }
        };
        let url = |url: &str| reqwest::Url::parse(url).expect("a url");
        let env = vars(&[
            ("HTTPS_PROXY", "http://proxy.corp:3128"),
            ("no_proxy", "internal.corp, .lan,10.0.0.0/8"),
        ]);
        assert_eq!(
            proxy_for(&url("https://docs.rs/x"), &env).as_deref(),
            Some("http://proxy.corp:3128")
        );
        assert_eq!(
            proxy_for(&url("http://docs.rs/x"), &env),
            None,
            "no HTTP_PROXY"
        );
        for exempt in [
            "https://internal.corp/",
            "https://wiki.internal.corp/",
            "https://nas.lan/",
            "https://10.1.2.3/",
        ] {
            assert_eq!(proxy_for(&url(exempt), &env), None, "{exempt}");
        }
        assert!(proxy_for(&url("https://notinternal.corp/"), &env).is_some());
        let all = vars(&[("ALL_PROXY", "http://proxy.corp:3128"), ("NO_PROXY", "*")]);
        assert_eq!(proxy_for(&url("http://docs.rs/"), &all), None);
    }

    #[tokio::test]
    async fn web_fetch_goes_through_the_user_s_proxy() {
        // The proxy answers for a host that does not resolve here.
        let proxy = serve(
            b"HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: 13\r\nConnection: close\r\n\r\nvia the proxy",
        )
        .await;
        let vars = move |name: &str| (name == "HTTP_PROXY").then(|| proxy.clone());
        let answer = super::web_fetch(
            &call(
                "WebFetch",
                json!({ "url": "http://artisan-proxy-test.invalid/page" }),
            ),
            &Rules::default(),
            false,
            &vars,
        )
        .await;
        assert!(!answer.is_error, "{}", answer.text);
        assert!(answer.text.ends_with("via the proxy"), "{}", answer.text);
        let answer = super::web_fetch(
            &call("WebFetch", json!({ "url": "http://127.0.0.1:9/" })),
            &Rules::default(),
            false,
            &vars,
        )
        .await;
        assert!(
            answer.text.contains("local or private"),
            "a private address is refused through the proxy too: {}",
            answer.text
        );
    }

    #[test]
    fn only_public_addresses_are_public() {
        for address in [
            "127.0.0.1",
            "10.1.2.3",
            "172.16.0.1",
            "192.168.1.1",
            "169.254.169.254",
            "0.0.0.0",
            "100.64.0.1",
            "224.0.0.1",
            "255.255.255.255",
            "::1",
            "::",
            "fe80::1",
            "fd00::1",
            "ff02::1",
            "::ffff:127.0.0.1",
            "::ffff:10.0.0.1",
            "64:ff9b::a9fe:a9fe",
        ] {
            assert!(
                !is_public(address.parse().expect("an address")),
                "{address}"
            );
        }
        for address in ["93.184.216.34", "1.1.1.1", "2606:4700::1111"] {
            assert!(is_public(address.parse().expect("an address")), "{address}");
        }
    }

    #[tokio::test]
    async fn web_fetch_refuses_local_and_private_hosts() {
        let rules = Rules::default();
        for url in [
            "http://127.0.0.1:9/",
            "http://localhost:9/",
            "http://[::ffff:127.0.0.1]:9/",
            "http://169.254.169.254/latest/meta-data/",
            "http://10.0.0.1/",
        ] {
            let answer = web_fetch(&call("WebFetch", json!({ "url": url })), &rules, false).await;
            assert!(answer.is_error, "{url}");
            assert!(
                answer.text.contains("local or private"),
                "{url}: {}",
                answer.text
            );
        }
    }

    /// Serves one canned HTTP response to every connection, on loopback.
    async fn serve(response: &'static [u8]) -> String {
        use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("a listener");
        let address = listener.local_addr().expect("an address");
        tokio::spawn(async move {
            while let Ok((mut socket, _)) = listener.accept().await {
                let mut request = vec![0u8; 8192];
                let _ = socket.read(&mut request).await;
                let _ = socket.write_all(response).await;
                let _ = socket.shutdown().await;
            }
        });
        format!("http://{address}/")
    }

    #[tokio::test]
    async fn a_redirect_to_a_denied_domain_is_not_followed() {
        let home = tempfile::tempdir().expect("a home");
        let project = tempfile::tempdir().expect("a project");
        let rules = project_rules(
            home.path(),
            project.path(),
            json!({ "deny": ["WebFetch(domain:denied.example)"] }),
        );
        let url = serve(
            b"HTTP/1.1 302 Found\r\nLocation: http://denied.example/secret\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
        )
        .await;
        let answer = web_fetch(&call("WebFetch", json!({ "url": url })), &rules, true).await;
        assert!(answer.is_error, "{}", answer.text);
        assert!(answer.text.contains("refused"), "{}", answer.text);
        let answer = web_fetch(
            &call("WebFetch", json!({ "url": url })),
            &Rules::default(),
            true,
        )
        .await;
        assert!(
            answer
                .text
                .contains("redirects to another host: http://denied.example/secret"),
            "a redirect off the site is handed back, not followed: {}",
            answer.text
        );
    }

    #[tokio::test]
    async fn web_fetch_shows_no_binary_and_no_cut_off_page() {
        let rules = Rules::default();
        let pdf = serve(
            b"HTTP/1.1 200 OK\r\nContent-Type: application/pdf\r\nContent-Length: 9\r\nConnection: close\r\n\r\n%PDF-1.7\n",
        )
        .await;
        let answer = web_fetch(&call("WebFetch", json!({ "url": pdf })), &rules, true).await;
        assert!(answer.is_error);
        assert!(
            answer
                .text
                .contains("binary content (application/pdf, 9 bytes)"),
            "{}",
            answer.text
        );
        let unlabelled = serve(
            b"HTTP/1.1 200 OK\r\nContent-Length: 12\r\nConnection: close\r\n\r\n\x89PNG\r\n\x1a\n\0\0\0\0",
        )
        .await;
        let answer = web_fetch(
            &call("WebFetch", json!({ "url": unlabelled })),
            &rules,
            true,
        )
        .await;
        assert!(answer.text.contains("binary content"), "{}", answer.text);
        let broken = serve(
            b"HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: 1000\r\nConnection: close\r\n\r\nthe start of",
        )
        .await;
        let answer = web_fetch(&call("WebFetch", json!({ "url": broken })), &rules, true).await;
        assert!(answer.is_error, "{}", answer.text);
        assert!(!answer.text.contains("the start of"), "{}", answer.text);
    }

    #[tokio::test]
    async fn web_fetch_decodes_the_declared_charset_and_carries_the_prompt() {
        let page = serve(
            b"HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: 70\r\nConnection: close\r\n\r\n<html><head><meta charset=\"iso-8859-1\"></head><body>Caf\xe9</body></html>",
        )
        .await;
        let answer = web_fetch(
            &call(
                "WebFetch",
                json!({ "url": page, "prompt": "What is sold?" }),
            ),
            &Rules::default(),
            true,
        )
        .await;
        assert!(!answer.is_error, "{}", answer.text);
        assert!(
            answer.text.contains("Requested: What is sold?"),
            "{}",
            answer.text
        );
        assert!(answer.text.ends_with("Café"), "{}", answer.text);
    }
}
