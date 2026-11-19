use std::collections::HashSet;
use std::fmt::Write as _;
use std::net::{IpAddr, SocketAddr, ToSocketAddrs as _};
use std::path::{Path, PathBuf};
use std::time::Duration;

use artisan_api::{
    ApprovalRequest, Decision, ItemKind, McpServer, Output, Permissions, TextContent, ToolKind,
};
use regex::Regex;
use serde_json::{Value, json};
use tokio::io::AsyncReadExt as _;
use tokio_util::sync::CancellationToken;

use super::files::{self, ReadFileError};
use super::rules::{Rules, Verdict, changes_files, url_host};
use super::{mcp, shells, skills};
use crate::sandbox::Sandbox;

const MAX_OUTPUT_BYTES: usize = 256 * 1024;
const MAX_FILE_BYTES: u64 = 16 * 1024 * 1024;
const DEFAULT_READ_LINES: u64 = 2000;
const MAX_LINE_CHARS: usize = 2000;
const DEFAULT_COMMAND_TIMEOUT: Duration = Duration::from_secs(120);
const MAX_COMMAND_TIMEOUT: Duration = Duration::from_secs(600);
const MAX_WALKED: usize = 200_000;
const MAX_MATCHES: usize = 2_000;
const WEB_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_WEB_BYTES: usize = 2 * 1024 * 1024;
const MAX_REDIRECTS: usize = 10;

#[derive(Clone, Debug)]
pub struct Call {
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) input: Value,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Plan {
    Run,
    Ask(ApprovalRequest),
    Refuse(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Answer {
    pub(crate) text: String,
    pub(crate) is_error: bool,
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

#[derive(Debug)]
pub(crate) struct Runtime {
    pub(crate) skills: Vec<skills::Skill>,
    pub(crate) mcp: mcp::Servers,
    pub(crate) shells: shells::Shells,
    pub(crate) rules: Rules,
    pub(crate) sandbox: Sandbox,
}

impl Runtime {
    pub(crate) async fn build(home: &Path, cwd: &Path, servers: &[McpServer], sandbox: Sandbox) -> Self {
        let mut servers = servers.to_vec();
        for project in mcp::project_servers(cwd) {
            if !servers.iter().any(|known| known.name == project.name) {
                servers.push(project);
            }
        }
        Self {
            skills: skills::discover(home, cwd),
            mcp: mcp::Servers::connect(&servers, cwd, &sandbox).await,
            shells: shells::Shells::default(),
            rules: Rules::load(home, cwd),
            sandbox,
        }
    }
}

#[derive(Debug, Default)]
pub(crate) struct Allowed {
    file_changes: bool,
    commands: HashSet<String>,
    tools: HashSet<String>,
    domains: HashSet<String>,
}

impl Allowed {
    pub(crate) fn remember(&mut self, call: &Call, decision: Decision) {
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

#[must_use]
pub(super) fn definitions(runtime: &Runtime) -> Vec<Value> {
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

#[must_use]
pub(super) fn questions(call: &Call) -> Vec<artisan_api::Question> {
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
                                    id: format!("{index}.{option}"),
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

#[must_use]
pub(super) fn plan(
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
    if let Some(error) = rules.broken() {
        let mut reason = String::from(
            "Every tool is refused until the user repairs a Claude Code settings file, because the permission rules in it could not be read: ",
        );
        let mut cause: Option<&dyn std::error::Error> = Some(error);
        while let Some(error) = cause {
            write!(reason, "{error}").expect("invariant: writing to a String cannot fail");
            cause = error.source();
            if cause.is_some() {
                reason.push_str(": ");
            }
        }
        reason.push_str(". Tell the user; do not retry.");
        return Plan::Refuse(reason);
    }
    let verdict = rules.verdict(call);
    if let Some(Verdict::Deny(reason)) = verdict {
        return Plan::Refuse(reason);
    }
    if permissions == Permissions::Full {
        return Plan::Run;
    }
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
        _ if changes_files(name) && rules.accept_edits() && rules.inside_root(call) => Plan::Run,
        _ => Plan::Ask(request_for(call, mcp)),
    }
}

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

fn web_request(call: &Call) -> ApprovalRequest {
    let url = call.input["url"].as_str().unwrap_or_default().trim();
    let site = web_host(call).unwrap_or_else(|| "an unknown site".to_owned());
    ApprovalRequest::Action {
        description: TextContent::from(format!("Fetch a page from {site}: {url}")),
    }
}

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

#[must_use]
pub(super) fn item_key(call: &Call) -> String {
    if call.name == "TodoWrite"
        && crate::claude::tools::validate_input("TodoWrite", &call.input).is_ok()
    {
        "plan".to_owned()
    } else {
        call.id.clone()
    }
}

#[must_use]
fn preview(call: &Call, cwd: &Path, mcp: &mcp::Servers) -> ItemKind {
    match call.name.as_str() {
        "TodoWrite" => match crate::claude::tools::validate_input("TodoWrite", &call.input) {
            Ok(()) => ItemKind::Plan {
                steps: crate::claude::tools::todo_steps(&call.input).into(),
                proposal: None,
                approval: None,
            },
            Err(_) => unreadable(call),
        },
        "Task" => agent_kind(call),
        _ => crate::claude::tools::tool_kind(&call.name, &call.input).map_or_else(
            |_| unreadable(call),
            |mut tool| {
                match &mut tool {
                    ToolKind::Shell { cwd: slot, .. } => *slot = Some(cwd.display().to_string()),
                    ToolKind::Mcp { server, tool, .. } => {
                        if let Some((known_server, known_tool)) = mcp.lookup(&call.name) {
                            known_server.clone_into(server);
                            known_tool.clone_into(tool);
                        }
                    }
                    ToolKind::AgentMessage { .. }
                    | ToolKind::Agents { .. }
                    | ToolKind::Read { .. }
                    | ToolKind::Edit { .. }
                    | ToolKind::Search { .. }
                    | ToolKind::WebFetch { .. }
                    | ToolKind::WebSearch { .. }
                    | ToolKind::ViewImage { .. }
                    | ToolKind::Workflow { .. }
                    | ToolKind::Other { .. } => {}
                }
                ItemKind::Tool {
                    tool,
                    title: call.input["description"].as_str().map(str::to_owned),
                    output: None,
                    exit_code: None,
                    approval: None,
                }
            },
        ),
    }
}

#[must_use]
pub(super) fn refused(call: &Call, cwd: &Path, mcp: &mcp::Servers, reason: &str) -> ItemKind {
    let mut item = preview(call, cwd, mcp);
    if let ItemKind::Agent { result, .. } = &mut item {
        *result = Some(reason.to_owned().into());
        return item;
    }
    finished(item, &Answer::error(reason), None)
}

#[must_use]
pub(super) fn approval_card(call: &Call, cwd: &Path, mcp: &mcp::Servers) -> ItemKind {
    let item = preview(call, cwd, mcp);
    if !matches!(item, ItemKind::Agent { .. }) {
        return item;
    }
    ItemKind::Tool {
        tool: ToolKind::Other {
            name: call.name.clone(),
            input: Some(call.input.to_string().into()),
        },
        title: call.input["description"].as_str().map(str::to_owned),
        output: None,
        exit_code: None,
        approval: None,
    }
}

#[must_use]
pub(super) fn agent_kind(call: &Call) -> ItemKind {
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

pub(super) async fn execute(
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
        "Grep" => grep(call, cwd, &runtime.rules, cancel).await,
        "Bash" if call.input["run_in_background"].as_bool() == Some(true) => {
            let answer = match call.input["command"].as_str() {
                Some(command) if !command.trim().is_empty() => runtime
                    .shells
                    .start(command, cwd, &runtime.sandbox)
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
        "Bash" => return bash(call, cwd, cancel, runtime).await,
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

fn finished(item: ItemKind, answer: &Answer, exit_code: Option<i32>) -> ItemKind {
    let ItemKind::Tool { tool, title, .. } = item else {
        return item;
    };
    ItemKind::Tool {
        tool,
        title,
        output: Some(Output::from_text(&answer.text)),
        exit_code,
        approval: None,
    }
}

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

async fn read_text(path: &Path) -> Result<String, Answer> {
    let owned = path.to_path_buf();
    let read = tokio::task::spawn_blocking(move || files::read_bounded(&owned, MAX_FILE_BYTES))
        .await
        .expect("invariant: reading a file does not panic");
    let bytes = read.map_err(|error| {
        Answer::error(match error {
            ReadFileError::Directory { .. } => format!(
                "{} is a directory; use Glob or Bash `ls` to list it.",
                path.display()
            ),
            ReadFileError::NotRegular { .. } => format!(
                "{} is not a regular file (a device, pipe or socket), so it cannot be read.",
                path.display()
            ),
            ReadFileError::TooLarge { limit, .. } => format!(
                "{} is more than {} MB, more than Read takes; read parts of it with Bash (e.g. sed -n).",
                path.display(),
                limit / (1024 * 1024)
            ),
            ReadFileError::Open { source, .. } | ReadFileError::Read { source, .. } => {
                format!("Could not read {}: {source}", path.display())
            }
        })
    })?;
    if bytes.iter().take(8192).any(|byte| *byte == 0) {
        return Err(Answer::error(format!(
            "{} is a binary file.",
            path.display()
        )));
    }
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

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
        writeln!(out, "{:>6}\t{line}", number + 1)
            .expect("invariant: writing to a String cannot fail");
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
        writeln!(out, "[{total} lines in all; pass offset to read on]")
            .expect("invariant: writing to a String cannot fail");
    }
    Answer::ok(truncate(&out))
}

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

fn walk(root: &Path) -> impl Iterator<Item = ignore::DirEntry> {
    ignore::WalkBuilder::new(root)
        .max_depth(Some(32))
        .follow_links(false)
        .require_git(false)
        .sort_by_file_name(Ord::cmp)
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
        .expect("invariant: writing to a String cannot fail");
    }
    Answer::ok(truncate(&listing))
}

async fn grep(call: &Call, cwd: &Path, rules: &Rules, cancel: &CancellationToken) -> Answer {
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
    let stop = cancel.clone();
    let hits =
        tokio::task::spawn_blocking(move || search(&root, &regex, only.as_ref(), &rules, &stop))
            .await
            .unwrap_or_default();
    if cancel.is_cancelled() {
        return Answer::error("The search was stopped.");
    }
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
        .expect("invariant: writing to a String cannot fail");
    }
    Answer::ok(truncate(&listing))
}

fn search(
    root: &Path,
    regex: &Regex,
    only: Option<&globset::GlobMatcher>,
    rules: &Rules,
    cancel: &CancellationToken,
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
        if cancel.is_cancelled() {
            return hits;
        }
        let name = relative(base, &path);
        if only.is_some_and(|only| !only.is_match(&name)) || rules.hides("Grep", &path) {
            continue;
        }
        let Ok(bytes) = files::read_bounded(&path, MAX_FILE_BYTES) else {
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

#[expect(
    clippy::too_many_lines,
    reason = "fetch/redirect/proxy in one vetted hop"
)]
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
        .expect("invariant: writing to a String cannot fail");
    }
    if cut {
        writeln!(
            answer,
            "[truncated: the page is more than {} MB; only its start was read]",
            MAX_WEB_BYTES / (1024 * 1024)
        )
        .expect("invariant: writing to a String cannot fail");
    }
    answer.push('\n');
    answer.push_str(text.trim());
    Answer::ok(truncate(&answer))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Body {
    Html,
    Text,
    Binary,
}

fn binary(url: &reqwest::Url, media: &str, size: &str) -> Answer {
    Answer::error(format!(
        "{url} is binary content ({media}, {size}), not shown. WebFetch reads web pages and text."
    ))
}

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

fn decode(bytes: &[u8], charset: Option<&str>, html: bool) -> String {
    let label = charset
        .map(str::to_owned)
        .or_else(|| html.then(|| meta_charset(bytes)).flatten());
    let encoding = label
        .and_then(|label| encoding_rs::Encoding::for_label(label.trim().as_bytes()))
        .unwrap_or(encoding_rs::UTF_8);
    encoding.decode(bytes).0.into_owned()
}

fn meta_charset(bytes: &[u8]) -> Option<String> {
    let head = String::from_utf8_lossy(&bytes[..bytes.len().min(4096)]).to_ascii_lowercase();
    let mut rest = head.as_str();
    while let Some((_, after)) = rest.split_once("<meta") {
        let tag = after.split_once('>').map_or(after, |(tag, _)| tag);
        if let Some((_, value)) = tag.split_once("charset=") {
            let label: String = value
                .trim_start_matches(['"', '\'', ' '])
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | ':' | '.'))
                .collect();
            if !label.is_empty() {
                return Some(label);
            }
        }
        rest = after;
    }
    None
}

fn same_site(from: &reqwest::Url, to: &reqwest::Url) -> bool {
    let site = |url: &reqwest::Url| {
        url_host(url.as_str()).map(|host| host.strip_prefix("www.").unwrap_or(&host).to_owned())
    };
    site(from).is_some() && site(from) == site(to)
}

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

pub fn html_to_text(html: &str) -> String {
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

fn strip_blocks(html: &str, tag: &str) -> String {
    let lower = html.to_ascii_lowercase();
    let open = format!("<{tag}");
    let close = format!("</{tag}");
    let mut out = String::with_capacity(html.len());
    let mut at = 0;
    while let Some(start) = lower.split_at(at).1.find(&open) {
        let start = at + start;
        out.push_str(
            html.get(at..start)
                .expect("invariant: ASCII lowercasing keeps every byte offset"),
        );
        let Some(end) = lower.split_at(start).1.find(&close) else {
            return out;
        };
        let end = start + end;
        let Some(gt) = lower.split_at(end).1.find('>') else {
            return out;
        };
        at = end + gt + 1;
    }
    out.push_str(html.split_at(at).1);
    out
}

fn decode_entities(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find('&') {
        out.push_str(rest.split_at(start).0);
        let end = start
            + 1
            + rest.as_bytes()[start + 1..]
                .iter()
                .take(12)
                .take_while(|byte| byte.is_ascii_alphanumeric() || **byte == b'#')
                .count();
        let decoded = (rest.as_bytes().get(end) == Some(&b';'))
            .then(|| rest.get(start + 1..end).and_then(entity))
            .flatten();
        if let Some(character) = decoded {
            out.push(character);
            rest = rest.split_at(end + 1).1;
        } else {
            out.push('&');
            rest = rest.split_at(start + 1).1;
        }
    }
    out.push_str(rest);
    out
}

fn entity(name: &str) -> Option<char> {
    match name {
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
        _ => name
            .strip_prefix("#x")
            .or_else(|| name.strip_prefix("#X"))
            .and_then(|hex| u32::from_str_radix(hex, 16).ok())
            .or_else(|| {
                name.strip_prefix('#')
                    .and_then(|decimal| decimal.parse::<u32>().ok())
            })
            .and_then(char::from_u32),
    }
}

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

async fn bash(
    call: &Call,
    cwd: &Path,
    cancel: &CancellationToken,
    runtime: &Runtime,
) -> (Answer, ItemKind) {
    let item = preview(call, cwd, &runtime.mcp);
    let command = command_of(call);
    if command.trim().is_empty() {
        let answer = Answer::error("Bash needs a command.");
        return (answer.clone(), finished(item, &answer, None));
    }
    let timeout = call.input["timeout"]
        .as_u64()
        .map_or(DEFAULT_COMMAND_TIMEOUT, Duration::from_millis)
        .min(MAX_COMMAND_TIMEOUT);
    let mut shell = shells::command_line(command);
    shell
        .current_dir(cwd)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    let mut child = match crate::process::owned_command(shell, runtime.sandbox.clone()) {
        Ok(child) => child,
        Err(error) => {
            let answer = Answer::error(format!("The command could not start: {error:#}"));
            return (answer.clone(), finished(item, &answer, None));
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
    let (code, unwaited) = match status {
        Ok(status) => (status.code().unwrap_or(-1), None),
        Err(error) => (
            -1,
            Some(format!("[the command's exit could not be read: {error}]")),
        ),
    };
    let mut text = out;
    if let Some(unwaited) = unwaited {
        if !text.is_empty() && !text.ends_with('\n') {
            text.push('\n');
        }
        text.push_str(&unwaited);
    }
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
        write!(answer, "[exit code {code}]").expect("invariant: writing to a String cannot fail");
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
        write!(text, "\n[{dropped} more bytes of output cut]")
            .expect("invariant: writing to a String cannot fail");
    }
    text
}

fn unreadable(call: &Call) -> ItemKind {
    ItemKind::Tool {
        tool: ToolKind::Other {
            name: call.name.clone(),
            input: Some(call.input.to_string().into()),
        },
        title: None,
        output: None,
        exit_code: None,
        approval: None,
    }
}

fn truncate(text: &str) -> String {
    if text.len() <= MAX_OUTPUT_BYTES {
        return text.to_owned();
    }
    let (kept, _) = text.split_at(text.floor_char_boundary(MAX_OUTPUT_BYTES));
    format!("{kept}\n[output cut]")
}

#[cfg(test)]
#[path = "../../tests/claudeai/tools.rs"]
mod tests;
