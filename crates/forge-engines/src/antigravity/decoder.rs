use super::protocol::{
    Call, Change, Command, ConfigOption, FormProperty, FormRequest, OTHER_SUFFIX, OptionKind,
    PermissionOption, PermissionRequest, PlanStatus, Prompted, StopReason, ToolContent,
    ToolKind as WireKind, ToolStatus, TurnUsage, dismissed, selected, submitted, withdrawn,
};
use super::wake::{Due, Hold, Seen, Wake};
use crate::harness::{EngineEvent, LiveKind};
use artisan_api::{
    Answer, Approval, ApprovalRequest, Decision, FileChange, FileOp, ItemKind, Output, Permissions,
    Phase, PlanStep, Question, QuestionAnswer, QuestionOption, SearchMode, Status, StepStatus,
    ToolKind, Usage,
};
use serde_json::Value;
use std::collections::{BTreeMap, HashMap};
use std::time::Instant;

const COMMAND_KEYS: [&str; 4] = ["CommandLine", "commandLine", "command_line", "command"];
const CWD_KEYS: [&str; 5] = [
    "Cwd",
    "cwd",
    "WorkingDirectory",
    "workingDir",
    "working_dir",
];
const PATH_KEYS: [&str; 6] = [
    "AbsolutePath",
    "TargetFile",
    "path",
    "file_path",
    "filePath",
    "File",
];
const PATTERN_KEYS: [&str; 5] = ["Query", "query", "Pattern", "pattern", "SearchPath"];
const URL_KEYS: [&str; 3] = ["Url", "url", "URL"];
const OUTPUT_KEYS: [&str; 4] = [
    "combinedOutput",
    "combined_output",
    "formatted_output",
    "output",
];
const EXIT_KEYS: [&str; 2] = ["exitCode", "exit_code"];
const DAEMON_KEYS: [&str; 2] = ["IsDaemon", "isDaemon"];
const COMMAND_OUTPUT: &str = "Command output:\n";
const SCHEDULE_TITLES: [&str; 2] = ["Run schedule", "Running schedule"];

#[derive(Debug, Default)]
pub(super) struct Decoder {
    texts: u64,
    plans: u64,
    compactions: u64,
    text: Option<Text>,
    thinking: bool,
    plan: Option<String>,
    compacting: Option<Compacting>,
    tools: BTreeMap<String, Tool>,
    pending: HashMap<String, Pending>,
    model: Option<String>,
    commands: Option<Vec<artisan_api::SlashCommand>>,
    usage: Usage,
    wake: Wake,
    waiting_on_hold: bool,
}

#[derive(Debug)]
struct Compacting {
    key: String,
    tokens_before: Option<u64>,
}

#[derive(Debug)]
struct Text {
    key: String,
    body: String,
}

#[derive(Debug)]
struct Tool {
    status: Status,
    kind: ItemKind,
    lifetime: Lifetime,
    backgrounded: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Lifetime {
    Turn,
    Command,
    Daemon,
    Schedule,
}

impl Tool {
    fn holds(&self) -> Option<Hold> {
        match (self.lifetime, self.status) {
            (Lifetime::Schedule, status) if !status.is_terminal() => Some(Hold::Schedule),
            (Lifetime::Command, Status::Running) => Some(Hold::Commands),
            _ => None,
        }
    }
}

#[derive(Debug)]
enum Pending {
    Permission {
        rpc: Value,
        options: Vec<PermissionOption>,
    },
    Form {
        rpc: Value,
        fields: Vec<Field>,
    },
}

impl Pending {
    fn withdraw(self) -> (Value, Value) {
        match self {
            Self::Permission { rpc, .. } => (rpc, dismissed()),
            Self::Form { rpc, .. } => (rpc, withdrawn()),
        }
    }
}

#[derive(Debug)]
struct Field {
    key: String,
    other: Option<String>,
    multi: bool,
}

#[derive(Debug, PartialEq)]
pub(super) enum Permitted {
    Ask(Vec<EngineEvent>),
    Reply {
        rpc: Value,
        result: Value,
        events: Vec<EngineEvent>,
    },
}

#[derive(Debug, PartialEq)]
pub(super) enum Formed {
    Asked(Vec<EngineEvent>),
    Refused {
        rpc: Value,
        result: Value,
        events: Vec<EngineEvent>,
    },
}

#[derive(Debug, PartialEq)]
pub(super) enum Answered {
    Reply {
        rpc: Value,
        result: Value,
        events: Vec<EngineEvent>,
    },
    Invalid(Vec<EngineEvent>),
    Unexpected,
}

impl Decoder {
    pub(super) fn update(&mut self, update: Change, now: Instant) -> Vec<EngineEvent> {
        let mut out = Vec::new();
        self.see(Seen::Progress, now, &mut out);
        match update {
            Change::AgentMessageChunk { content } => {
                if let Some(text) = content.text() {
                    self.finish_compaction(Status::Done, None, &mut out);
                    if self.wake.reply_closed() {
                        self.close_text(&mut out);
                    }
                    self.see(Seen::Output, now, &mut out);
                    self.text(text, &mut out);
                }
            }
            Change::AgentThoughtChunk { content } => {
                if let Some(text) = content.text() {
                    self.finish_compaction(Status::Done, None, &mut out);
                    self.see(Seen::Output, now, &mut out);
                    self.think(text, &mut out);
                }
            }
            Change::Call(call) | Change::ToolCallUpdate(call) => self.call(&call, now, &mut out),
            Change::Plan { entries } => {
                self.finish_compaction(Status::Done, None, &mut out);
                self.see(Seen::Output, now, &mut out);
                let key = self.plan.get_or_insert_with(|| {
                    self.plans += 1;
                    format!("plan:{}", self.plans)
                });
                out.push(EngineEvent::Item {
                    key: key.clone(),
                    parent: None,
                    status: Status::Running,
                    kind: ItemKind::Plan {
                        steps: entries
                            .into_iter()
                            .map(|entry| PlanStep {
                                text: entry.content.into(),
                                status: match entry.status {
                                    PlanStatus::Pending => StepStatus::Pending,
                                    PlanStatus::InProgress => StepStatus::InProgress,
                                    PlanStatus::Completed => StepStatus::Done,
                                },
                            })
                            .collect::<Vec<_>>()
                            .into(),
                        proposal: None,
                        approval: None,
                    },
                });
            }
            Change::AvailableCommandsUpdate { available_commands } => {
                let commands: Vec<artisan_api::SlashCommand> =
                    available_commands.into_iter().map(command).collect();
                if self.commands.as_ref() != Some(&commands) {
                    self.commands = Some(commands.clone());
                    out.push(EngineEvent::Commands(commands));
                }
            }
            Change::ConfigOptionUpdate { config_options } => {
                self.configured(&config_options, &mut out);
            }
            Change::SessionInfoUpdate { title: Some(title) } if !title.trim().is_empty() => {
                out.push(EngineEvent::Title { title });
            }
            Change::UsageUpdate { used, size, cost } => {
                self.usage.context_used = used.or(self.usage.context_used);
                self.usage.context_window = size.or(self.usage.context_window);
                if let Some(cost) = cost.filter(|cost| {
                    cost.currency
                        .as_deref()
                        .is_none_or(|currency| currency.eq_ignore_ascii_case("usd"))
                }) {
                    self.usage.cost_usd = Some(cost.amount);
                }
                if let Some(after) = used
                    && self.compacting.as_ref().is_some_and(|active| {
                        active.tokens_before.is_some_and(|before| after < before)
                    })
                {
                    self.finish_compaction(Status::Done, Some(after), &mut out);
                }
                out.push(EngineEvent::Usage(self.usage.clone()));
                self.step_ended(now, &mut out);
            }
            Change::UserMessageChunk { .. }
            | Change::CurrentModeUpdate { .. }
            | Change::SessionInfoUpdate { .. }
            | Change::Unknown => {}
        }
        self.announce(&mut out);
        out
    }

    pub(super) fn configured(&mut self, options: &[ConfigOption], out: &mut Vec<EngineEvent>) {
        let model = options
            .iter()
            .find(|option| option.is(super::protocol::MODEL))
            .and_then(|option| option.current_value.clone());
        if let Some(model) = model
            && self.model.as_ref() != Some(&model)
        {
            self.model = Some(model.clone());
            out.push(EngineEvent::Model { id: model });
        }
    }

    pub(super) fn permission(
        &mut self,
        rpc: Value,
        request: PermissionRequest,
        permissions: Permissions,
        now: Instant,
    ) -> Permitted {
        let PermissionRequest {
            tool_call, options, ..
        } = request;
        let key = tool_call.tool_call_id.clone();
        let mut out = Vec::new();
        self.finish_compaction(Status::Done, None, &mut out);
        self.see(Seen::Output, now, &mut out);
        self.close_text(&mut out);
        self.thinking = false;
        if permissions == Permissions::Full
            && let Some(option) =
                preferred(&options, &[OptionKind::AllowOnce, OptionKind::AllowAlways])
        {
            let result = selected(&option.option_id);
            self.tool(&tool_call, &mut out);
            return Permitted::Reply {
                rpc,
                result,
                events: out,
            };
        }
        let request = approval_request(&tool_call);
        let mut announced = Vec::new();
        self.tool(&tool_call, &mut announced);
        let decisions: Vec<Decision> = options.iter().map(|option| decision(option.kind)).collect();
        if let Some(Tool {
            status,
            kind: ItemKind::Tool { approval, .. },
            ..
        }) = self.tools.get_mut(&key)
        {
            *status = Status::AwaitingUser;
            *approval = Some(Approval {
                request,
                reason: None,
                options: decisions,
                decision: None,
            });
        }
        self.pending
            .insert(key.clone(), Pending::Permission { rpc, options });
        out.push(self.event(&key));
        Permitted::Ask(out)
    }

    pub(super) fn form(&mut self, rpc: Value, request: FormRequest, now: Instant) -> Formed {
        let mut out = Vec::new();
        self.finish_compaction(Status::Done, None, &mut out);
        self.see(Seen::Output, now, &mut out);
        self.close_text(&mut out);
        self.thinking = false;
        let (questions, fields) = match questions(&request) {
            Ok(asked) => asked,
            Err(message) => {
                self.texts += 1;
                out.push(EngineEvent::Item {
                    key: format!("form-error:{}", self.texts),
                    parent: None,
                    status: Status::Done,
                    kind: ItemKind::Error {
                        kind: artisan_api::ErrorKind::Execution,
                        message: message.into(),
                        retryable: false,
                    },
                });
                return Formed::Refused {
                    rpc,
                    result: withdrawn(),
                    events: out,
                };
            }
        };
        self.texts += 1;
        let key = request
            .tool_call_id
            .unwrap_or_else(|| format!("form:{}", self.texts));
        self.tools.insert(
            key.clone(),
            Tool {
                status: Status::AwaitingUser,
                kind: ItemKind::Question {
                    blocking: true,
                    questions: questions.into(),
                    answers: None,
                    answer_text: None,
                },
                lifetime: Lifetime::Turn,
                backgrounded: false,
            },
        );
        self.pending
            .insert(key.clone(), Pending::Form { rpc, fields });
        out.push(self.event(&key));
        Formed::Asked(out)
    }

    pub(super) fn answer(&mut self, key: &str, answer: &Answer, now: Instant) -> Answered {
        let Some(pending) = self.pending.get(key) else {
            return Answered::Unexpected;
        };
        let questions: Vec<Question> = match self.tools.get(key).map(|tool| &tool.kind) {
            Some(ItemKind::Question { questions, .. }) => questions
                .inline()
                .map(<[Question]>::to_vec)
                .unwrap_or_default(),
            _ => Vec::new(),
        };
        let result = match reply(pending, &questions, answer) {
            Ok(result) => result,
            Err(message) => {
                return Answered::Invalid(vec![crate::question::invalid_answer(key, message)]);
            }
        };
        let Some(Pending::Permission { rpc, .. } | Pending::Form { rpc, .. }) =
            self.pending.remove(key)
        else {
            return Answered::Unexpected;
        };
        let mut events = Vec::new();
        self.see(Seen::Output, now, &mut events);
        if let Some(tool) = self.tools.get_mut(key) {
            tool.status = match (&mut tool.kind, answer) {
                (
                    ItemKind::Tool {
                        approval: Some(approval),
                        ..
                    },
                    Answer::Approval { decision },
                ) => {
                    approval.decision = Some(*decision);
                    if decision.allows() {
                        Status::Running
                    } else {
                        Status::Declined
                    }
                }
                (
                    ItemKind::Question {
                        answers: slot,
                        answer_text,
                        ..
                    },
                    Answer::Question { answers },
                ) => {
                    *slot = Some(answers.clone().into());
                    let told: Vec<String> = questions
                        .iter()
                        .zip(answers)
                        .filter_map(|(question, answer)| {
                            question.answer_text(answer.as_ref()?).ok()
                        })
                        .collect();
                    *answer_text = (!told.is_empty()).then(|| told.join("\n").into());
                    if answers.iter().all(Option::is_none) {
                        Status::Cancelled
                    } else {
                        Status::Done
                    }
                }
                _ => unreachable!("invariant: pending {key} was checked against its answer"),
            };
            events.push(self.event(key));
        }
        Answered::Reply {
            rpc,
            result,
            events,
        }
    }

    pub(super) fn ended(
        &mut self,
        prompted: &Prompted,
        superseded: bool,
        now: Instant,
    ) -> Vec<EngineEvent> {
        let mut out = Vec::new();
        if let Some(usage) = &prompted.usage {
            self.add_usage(usage);
            out.push(EngineEvent::Usage(self.usage.clone()));
        }
        if superseded {
            return out;
        }
        let cancelled = prompted.stop_reason == StopReason::Cancelled;
        if cancelled {
            out.extend(self.settle_open(Status::Cancelled));
        } else {
            self.finish_compaction(Status::Done, self.usage.context_used, &mut out);
        }
        let hold = self.hold();
        if let Some(text) = self.text.take() {
            let (status, phase) = match (cancelled, hold) {
                (true, _) => (Status::Cancelled, Phase::Commentary),
                (false, Hold::Commands | Hold::Schedule) => (Status::Done, Phase::Commentary),
                (false, Hold::Free) => (Status::Done, Phase::Final),
            };
            out.insert(0, text_item(text, status, phase));
        }
        self.thinking = false;
        if let Some(key) = self.plan.take() {
            out.push(EngineEvent::Status {
                key,
                status: Status::Done,
            });
        }
        let lingers =
            !cancelled && (hold != Hold::Free || self.tools.values().any(|tool| tool.backgrounded));
        self.see(Seen::PromptEnded { lingers }, now, &mut out);
        out
    }

    pub(super) fn prompted(&mut self, compacting: bool, now: Instant) -> Vec<EngineEvent> {
        let mut out = Vec::new();
        self.see(Seen::PromptSent, now, &mut out);
        if compacting && self.compacting.is_none() {
            self.compactions += 1;
            let key = format!("compaction:{}", self.compactions);
            let tokens_before = self.usage.context_used;
            self.compacting = Some(Compacting {
                key: key.clone(),
                tokens_before,
            });
            out.push(compaction_item(&key, Status::Running, tokens_before, None));
        }
        out
    }

    pub(super) fn abandoned(&mut self) -> Vec<EngineEvent> {
        let mut out = Vec::new();
        self.finish_compaction(Status::Cancelled, None, &mut out);
        self.stopped(&mut out);
        out
    }

    fn finish_compaction(
        &mut self,
        status: Status,
        tokens_after: Option<u64>,
        out: &mut Vec<EngineEvent>,
    ) {
        if let Some(active) = self.compacting.take() {
            out.push(compaction_item(
                &active.key,
                status,
                active.tokens_before,
                tokens_after,
            ));
        }
    }

    pub(super) const fn wake(&self) -> Wake {
        self.wake
    }

    pub(super) fn due(&self) -> Option<(Instant, Due)> {
        if !self.pending.is_empty() {
            return None;
        }
        let busy = self
            .tools
            .values()
            .any(|tool| !tool.status.is_terminal() && tool.lifetime != Lifetime::Daemon);
        self.wake.due(self.hold(), busy)
    }

    pub(super) fn lapse(&mut self, now: Instant) -> Vec<EngineEvent> {
        let mut out = Vec::new();
        self.see(Seen::Lapsed, now, &mut out);
        out
    }

    pub(super) fn probed(&mut self, pending: bool, now: Instant) -> Vec<EngineEvent> {
        let mut out = Vec::new();
        self.see(Seen::Probed { pending }, now, &mut out);
        out
    }

    fn call(&mut self, call: &Call, now: Instant, out: &mut Vec<EngineEvent>) {
        self.finish_compaction(Status::Done, None, out);
        let held = self
            .tools
            .get(&call.tool_call_id)
            .and_then(Tool::holds)
            .is_some();
        if !held {
            self.see(Seen::Output, now, out);
            self.close_text(out);
            self.thinking = false;
        }
        self.tool(call, out);
        let released = held
            && self
                .tools
                .get(&call.tool_call_id)
                .is_some_and(|tool| tool.status.is_terminal());
        if released {
            self.see(Seen::Output, now, out);
        }
    }

    fn step_ended(&mut self, now: Instant, out: &mut Vec<EngineEvent>) {
        for tool in self.tools.values_mut() {
            if tool.holds().is_some() {
                tool.backgrounded = true;
            }
        }
        self.see(Seen::StepEnded, now, out);
        if self.wake.after_prompt() && self.hold() != Hold::Free {
            self.close_text(out);
        }
    }

    fn hold(&self) -> Hold {
        self.tools
            .values()
            .filter_map(Tool::holds)
            .max_by_key(|hold| match hold {
                Hold::Free => 0,
                Hold::Commands => 1,
                Hold::Schedule => 2,
            })
            .unwrap_or(Hold::Free)
    }

    fn see(&mut self, seen: Seen, now: Instant, out: &mut Vec<EngineEvent>) {
        self.wake = self.wake.next(seen, now);
        self.announce(out);
    }

    fn stopped(&mut self, out: &mut Vec<EngineEvent>) {
        self.wake = Wake::Quiet;
        self.announce(out);
    }

    fn announce(&mut self, out: &mut Vec<EngineEvent>) {
        let waiting = self.wake.after_prompt() && self.hold() != Hold::Free;
        if waiting != self.waiting_on_hold {
            self.waiting_on_hold = waiting;
            out.push(EngineEvent::background_wait(
                artisan_api::EngineId::Antigravity,
                waiting,
            ));
        }
    }

    pub(super) fn cancel(&mut self) -> (Vec<(Value, Value)>, Vec<EngineEvent>) {
        let replies = self
            .pending
            .drain()
            .map(|(_, pending)| pending.withdraw())
            .collect();
        let mut out = Vec::new();
        if let Some(text) = self.text.take() {
            out.push(text_item(text, Status::Cancelled, Phase::Commentary));
        }
        out.extend(self.settle_open(Status::Cancelled));
        self.stopped(&mut out);
        (replies, out)
    }

    pub(super) fn finish(&mut self) -> Vec<EngineEvent> {
        let mut out = Vec::new();
        if let Some(text) = self.text.take() {
            out.push(text_item(text, Status::Done, Phase::Final));
        }
        out.extend(self.settle_open(Status::Cancelled));
        self.stopped(&mut out);
        out
    }

    pub(super) fn waiting(&self) -> bool {
        !self.pending.is_empty()
    }

    fn settle_open(&mut self, status: Status) -> Vec<EngineEvent> {
        self.pending.clear();
        let mut out = Vec::new();
        self.finish_compaction(status, None, &mut out);
        out.extend(
            self.tools
                .iter_mut()
                .filter(|(_, tool)| !tool.status.is_terminal())
                .map(|(key, tool)| {
                    tool.status = status;
                    EngineEvent::Status {
                        key: key.clone(),
                        status,
                    }
                }),
        );
        out
    }

    fn add_usage(&mut self, usage: &TurnUsage) {
        self.usage.input += usage.input;
        self.usage.output += usage.output;
        self.usage.reasoning += usage.thought.unwrap_or_default();
        self.usage.cache_read += usage.cached_read.unwrap_or_default();
        if let Some(written) = usage.cached_write {
            self.usage.cache_write = Some(self.usage.cache_write.unwrap_or_default() + written);
        }
    }

    fn text(&mut self, chunk: &str, out: &mut Vec<EngineEvent>) {
        self.thinking = false;
        let text = self.text.get_or_insert_with(|| {
            self.texts += 1;
            let key = format!("text:{}", self.texts);
            out.push(EngineEvent::Item {
                key: key.clone(),
                parent: None,
                status: Status::Running,
                kind: ItemKind::AssistantMessage {
                    text: String::new().into(),
                    phase: Phase::Commentary,
                },
            });
            Text {
                key,
                body: String::new(),
            }
        });
        text.body.push_str(chunk);
        out.push(EngineEvent::Live {
            target: LiveKind::Text {
                key: text.key.clone(),
            },
            text: chunk.to_owned(),
            append: true,
        });
    }

    fn think(&mut self, chunk: &str, out: &mut Vec<EngineEvent>) {
        self.close_text(out);
        if !self.thinking {
            self.thinking = true;
            out.push(EngineEvent::Live {
                target: LiveKind::Reasoning,
                text: String::new(),
                append: false,
            });
        }
        out.push(EngineEvent::Live {
            target: LiveKind::Reasoning,
            text: chunk.to_owned(),
            append: true,
        });
    }

    fn close_text(&mut self, out: &mut Vec<EngineEvent>) {
        if let Some(text) = self.text.take() {
            out.push(text_item(text, Status::Done, Phase::Commentary));
        }
    }

    fn tool(&mut self, call: &Call, out: &mut Vec<EngineEvent>) {
        let key = call.tool_call_id.clone();
        let status = call.status.map(tool_status);
        let entry = self.tools.entry(key.clone()).or_insert_with(|| Tool {
            status: Status::Running,
            kind: ItemKind::Tool {
                tool: ToolKind::Other {
                    name: "tool".to_owned(),
                    input: None,
                },
                title: None,
                output: None,
                exit_code: None,
                approval: None,
            },
            lifetime: Lifetime::Turn,
            backgrounded: false,
        });
        if let ItemKind::Tool {
            tool,
            title,
            output,
            exit_code,
            ..
        } = &mut entry.kind
        {
            if (call.kind.is_some()
                || call.raw_input.is_some()
                || call.locations.is_some()
                || call.content.is_some())
                && let Some(kind) = tool_kind(call, tool)
            {
                *tool = kind;
            }
            if let Some(name) = call.title.as_ref().filter(|name| !name.trim().is_empty()) {
                *title = Some(name.clone());
            }
            if let Some(text) = output_text(call) {
                *output = Some(Output::from_text(&text));
            }
            if let Some(code) = call.raw_output.as_ref().and_then(|raw| {
                EXIT_KEYS
                    .iter()
                    .find_map(|key| raw.get(*key).and_then(Value::as_i64))
            }) {
                *exit_code = i32::try_from(code).ok();
            }
        }
        entry.lifetime = lifetime(call, entry.lifetime);
        if let Some(status) = status {
            entry.status = entry.status.advance(status);
        }
        out.push(self.event(&key));
    }

    fn event(&self, key: &str) -> EngineEvent {
        let tool = &self.tools[key];
        EngineEvent::Item {
            key: key.to_owned(),
            parent: None,
            status: tool.status,
            kind: tool.kind.clone(),
        }
    }
}

fn lifetime(call: &Call, current: Lifetime) -> Lifetime {
    if current == Lifetime::Schedule
        || call
            .title
            .as_deref()
            .is_some_and(|title| SCHEDULE_TITLES.contains(&title))
    {
        return Lifetime::Schedule;
    }
    let daemon = call.raw_input.as_ref().map(|input| {
        DAEMON_KEYS
            .iter()
            .any(|key| input.get(*key).and_then(Value::as_bool) == Some(true))
    });
    match (call.kind, current, daemon) {
        (Some(WireKind::Execute) | None, Lifetime::Command | Lifetime::Daemon, Some(true))
        | (Some(WireKind::Execute), _, Some(true)) => Lifetime::Daemon,
        (Some(WireKind::Execute) | None, Lifetime::Command | Lifetime::Daemon, Some(false))
        | (Some(WireKind::Execute), Lifetime::Turn, Some(false) | None) => Lifetime::Command,
        (_, current, _) => current,
    }
}

fn text_item(text: Text, status: Status, phase: Phase) -> EngineEvent {
    EngineEvent::Item {
        key: text.key,
        parent: None,
        status,
        kind: ItemKind::AssistantMessage {
            text: text.body.into(),
            phase,
        },
    }
}

fn command(command: Command) -> artisan_api::SlashCommand {
    artisan_api::SlashCommand {
        name: command.name,
        description: command.description,
        hint: command.input.and_then(|input| input.hint),
    }
}

const fn tool_status(status: ToolStatus) -> Status {
    match status {
        ToolStatus::Pending | ToolStatus::InProgress => Status::Running,
        ToolStatus::Completed => Status::Done,
        ToolStatus::Failed => Status::Failed,
    }
}

const fn decision(kind: OptionKind) -> Decision {
    match kind {
        OptionKind::AllowOnce => Decision::AllowOnce,
        OptionKind::AllowAlways => Decision::AllowAlways,
        OptionKind::RejectOnce => Decision::DenyOnce,
        OptionKind::RejectAlways => Decision::DenyAlways,
    }
}

fn preferred<'a>(
    options: &'a [PermissionOption],
    kinds: &[OptionKind],
) -> Option<&'a PermissionOption> {
    kinds
        .iter()
        .find_map(|kind| options.iter().find(|option| option.kind == *kind))
}

fn reply(pending: &Pending, questions: &[Question], answer: &Answer) -> Result<Value, String> {
    match (pending, answer) {
        (Pending::Permission { .. }, Answer::Approval { decision })
            if *decision == Decision::Cancelled =>
        {
            Ok(dismissed())
        }
        (Pending::Permission { options, .. }, Answer::Approval { decision }) => options
            .iter()
            .find(|option| self::decision(option.kind) == *decision)
            .map(|option| selected(&option.option_id))
            .ok_or_else(|| "Antigravity didn't offer that decision".to_owned()),
        (Pending::Form { fields, .. }, Answer::Question { answers }) => {
            form_reply(fields, questions, answers)
        }
        (Pending::Permission { .. }, Answer::Question { .. })
        | (Pending::Form { .. }, Answer::Approval { .. }) => {
            Err("That answer doesn't fit what Antigravity asked".to_owned())
        }
    }
}

fn form_reply(
    fields: &[Field],
    questions: &[Question],
    answers: &[Option<QuestionAnswer>],
) -> Result<Value, String> {
    if answers.len() != fields.len() || questions.len() != fields.len() {
        return Err("The answers don't match Antigravity's questions".to_owned());
    }
    if answers.iter().all(Option::is_none) {
        return Ok(withdrawn());
    }
    let mut content = serde_json::Map::new();
    for ((field, question), answer) in fields.iter().zip(questions).zip(answers) {
        question
            .validate_answer(answer.as_ref())
            .map_err(str::to_owned)?;
        match answer {
            None => {}
            Some(QuestionAnswer::Choices { ids }) => {
                let ids = ids.inline().ok_or("submitted choices must be inline")?;
                let value = if field.multi {
                    Value::from(ids.to_vec())
                } else {
                    Value::from(ids.first().ok_or("no choice was selected")?.clone())
                };
                content.insert(field.key.clone(), value);
            }
            Some(QuestionAnswer::Text { text }) => {
                let has_choices = question
                    .options
                    .inline()
                    .is_some_and(|options| !options.is_empty());
                let key = match (&field.other, has_choices) {
                    (Some(other), _) => other.clone(),
                    (None, false) => field.key.clone(),
                    (None, true) => {
                        return Err(
                            "Antigravity only accepts one of the offered answers".to_owned()
                        );
                    }
                };
                content.insert(key, Value::from(text.clone()));
            }
        }
    }
    Ok(submitted(content))
}

fn questions(request: &FormRequest) -> Result<(Vec<Question>, Vec<Field>), String> {
    let properties = &request.requested_schema.properties;
    let mut questions = Vec::new();
    let mut fields = Vec::new();
    for (key, raw) in properties {
        if key
            .strip_suffix(OTHER_SUFFIX)
            .is_some_and(|base| properties.contains_key(base))
        {
            continue;
        }
        let property: FormProperty = serde_json::from_value(raw.clone()).map_err(|error| {
            format!("Antigravity asked field {key} in a shape Artisan can't show: {error}")
        })?;
        let multi = property.kind.as_deref() == Some("array");
        let listed = if multi {
            property.items.as_deref().cloned().unwrap_or_default()
        } else {
            property.clone()
        };
        let mut options: Vec<QuestionOption> = listed
            .one_of
            .iter()
            .chain(&listed.any_of)
            .map(|choice| QuestionOption {
                id: choice.value.clone(),
                label: choice.title.clone().unwrap_or_else(|| choice.value.clone()),
                description: None,
            })
            .collect();
        if options.is_empty() {
            options = listed
                .values
                .iter()
                .map(|value| QuestionOption {
                    id: value.clone(),
                    label: value.clone(),
                    description: None,
                })
                .collect();
        }
        let other =
            Some(format!("{key}{OTHER_SUFFIX}")).filter(|other| properties.contains_key(other));
        let text = property
            .title
            .clone()
            .or_else(|| property.description.clone())
            .or_else(|| request.message.clone())
            .unwrap_or_else(|| key.clone());
        questions.push(Question {
            id: key.clone(),
            header: None,
            text: text.into(),
            multi_select: multi,
            options: options.into(),
        });
        fields.push(Field {
            key: key.clone(),
            other,
            multi,
        });
    }
    if questions.is_empty() {
        return Err("Antigravity asked a form with no questions".to_owned());
    }
    Ok((questions, fields))
}

fn field<'a>(raw: Option<&'a Value>, keys: &[&str]) -> Option<&'a str> {
    let raw = raw?;
    keys.iter()
        .find_map(|key| raw.get(*key).and_then(Value::as_str))
        .filter(|value| !value.trim().is_empty())
}

fn first_path(call: &Call) -> Option<String> {
    call.locations
        .as_ref()
        .and_then(|locations| locations.first())
        .map(|location| location.path.clone())
        .or_else(|| field(call.raw_input.as_ref(), &PATH_KEYS).map(str::to_owned))
}

fn approval_request(call: &Call) -> ApprovalRequest {
    match call.kind {
        Some(WireKind::Execute) => ApprovalRequest::Command {
            command: field(call.raw_input.as_ref(), &COMMAND_KEYS)
                .or(call.title.as_deref())
                .unwrap_or("a command")
                .to_owned()
                .into(),
            cwd: field(call.raw_input.as_ref(), &CWD_KEYS).map(str::to_owned),
        },
        Some(WireKind::Edit | WireKind::Delete | WireKind::Move) => ApprovalRequest::FileChange,
        _ => ApprovalRequest::Action {
            description: call
                .title
                .clone()
                .unwrap_or_else(|| "Antigravity wants to use a tool".to_owned())
                .into(),
        },
    }
}

fn tool_kind(call: &Call, current: &ToolKind) -> Option<ToolKind> {
    let input = call.raw_input.as_ref();
    let name = || {
        call.name
            .clone()
            .or_else(|| call.title.clone())
            .filter(|name| !name.trim().is_empty())
            .unwrap_or_else(|| "tool".to_owned())
    };
    let other = || ToolKind::Other {
        name: name(),
        input: input
            .filter(|input| input.as_object().is_none_or(|fields| !fields.is_empty()))
            .map(|input| input.to_string().into()),
    };
    let kind = match call.kind {
        Some(WireKind::Execute) => match field(input, &COMMAND_KEYS) {
            Some(command) => ToolKind::Shell {
                command: command.to_owned().into(),
                cwd: field(input, &CWD_KEYS).map(str::to_owned),
                background: false,
                watch: false,
            },
            None => other(),
        },
        Some(WireKind::Read) => match first_path(call) {
            Some(path) => ToolKind::Read { path, range: None },
            None => other(),
        },
        Some(WireKind::Edit | WireKind::Delete | WireKind::Move) => {
            let changes = changes(call);
            if changes.is_empty() {
                other()
            } else {
                ToolKind::Edit {
                    changes: changes.into(),
                }
            }
        }
        Some(WireKind::Search) => ToolKind::Search {
            pattern: field(input, &PATTERN_KEYS).unwrap_or_default().to_owned(),
            path: first_path(call),
            mode: SearchMode::Grep,
        },
        Some(WireKind::Fetch) => match field(input, &URL_KEYS) {
            Some(url) => ToolKind::WebFetch {
                url: url.to_owned(),
            },
            None => ToolKind::WebSearch {
                query: field(input, &PATTERN_KEYS).map_or_else(name, str::to_owned),
            },
        },
        Some(WireKind::Think | WireKind::SwitchMode | WireKind::Other) => other(),
        None if matches!(current, ToolKind::Other { .. }) => other(),
        None => return None,
    };
    Some(kind)
}

fn changes(call: &Call) -> Vec<FileChange> {
    let op = match call.kind {
        Some(WireKind::Delete) => FileOp::Delete,
        _ => FileOp::Modify,
    };
    let diffs: Vec<FileChange> = call
        .content
        .iter()
        .flatten()
        .filter_map(|content| match content {
            ToolContent::Diff {
                path,
                old_text,
                new_text,
            } => Some(FileChange {
                path: path.clone(),
                op: match (old_text, new_text) {
                    (None, Some(_)) => FileOp::Add,
                    (_, None) => FileOp::Delete,
                    (Some(_), Some(_)) => op.clone(),
                },
                diff: Some(
                    similar::TextDiff::from_lines(
                        old_text.as_deref().unwrap_or_default(),
                        new_text.as_deref().unwrap_or_default(),
                    )
                    .unified_diff()
                    .to_string()
                    .into(),
                ),
            }),
            ToolContent::Content { .. } | ToolContent::Terminal { .. } | ToolContent::Other => None,
        })
        .collect();
    if !diffs.is_empty() {
        return diffs;
    }
    call.locations
        .iter()
        .flatten()
        .map(|location| FileChange {
            path: location.path.clone(),
            op: op.clone(),
            diff: None,
        })
        .collect()
}

fn command_output(text: &str) -> &str {
    text.split_once(COMMAND_OUTPUT)
        .map_or(text, |(_, output)| output)
}

fn compaction_item(
    key: &str,
    status: Status,
    tokens_before: Option<u64>,
    tokens_after: Option<u64>,
) -> EngineEvent {
    EngineEvent::Item {
        key: key.to_owned(),
        parent: None,
        status,
        kind: ItemKind::Compaction {
            summary: None,
            tokens_before,
            tokens_after,
            switch: None,
        },
    }
}

fn output_text(call: &Call) -> Option<String> {
    if let Some(text) = field(call.raw_output.as_ref(), &OUTPUT_KEYS) {
        return Some(command_output(text).to_owned());
    }
    let texts: Vec<&str> = call
        .content
        .iter()
        .flatten()
        .filter_map(|content| match content {
            ToolContent::Content { content } => content.text(),
            ToolContent::Diff { .. } | ToolContent::Terminal { .. } | ToolContent::Other => None,
        })
        .collect();
    (!texts.is_empty()).then(|| texts.join("\n"))
}
