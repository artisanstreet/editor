use crate::plan::{Action, Ending, Policy, Stop};
use serde_json::json;
use test_support::transcript::{ANY, Source, Step, Transcript};

const SESSION: &str = "00000000-0000-4000-8000-00000000517a";
const CONTINUE: &str = "Artisan restarted and interrupted this turn. Continue from where you left off; check the current state before repeating anything that may already have finished.";
const KILLED: u8 = 137;

pub(crate) fn message(number: usize) -> String {
    format!("Step {number}")
}

pub(crate) fn reply(number: usize) -> String {
    format!("Finished step {number}.")
}

pub(crate) fn progress(number: usize) -> String {
    format!("Working on step {number}.")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Opening {
    Resumed,
    Fresh,
    HandedOff,
}

impl Opening {
    const fn of(session: bool, history: bool) -> Self {
        match (session, history) {
            (true, _) => Self::Resumed,
            (false, false) => Self::Fresh,
            (false, true) => Self::HandedOff,
        }
    }

    fn sent(self, text: &str) -> String {
        match self {
            Self::Resumed | Self::Fresh => text.to_owned(),
            Self::HandedOff => format!("{ANY}\n\n{text}"),
        }
    }

    const fn titled(self) -> bool {
        match self {
            Self::Resumed => false,
            Self::Fresh | Self::HandedOff => true,
        }
    }
}

pub(crate) fn script(actions: &[Action]) -> Transcript {
    let mut policy = Policy::default();
    let mut session = false;
    let mut history = false;
    let launches = actions
        .iter()
        .flat_map(|action| match action {
            Action::Turn { number, ending, .. } => {
                let opening = Opening::of(session, history);
                let launches = launches(*number, *ending, opening, policy);
                session = ending.leaves_session(session);
                history = true;
                launches
            }
            Action::Toggle(toggle) => {
                policy.toggle(*toggle);
                Vec::new()
            }
            Action::Draft { .. }
            | Action::Restart(_)
            | Action::StepClock { .. }
            | Action::DropEvents { .. } => Vec::new(),
        })
        .collect();
    Transcript::new(Source::Synthetic, launches)
}

fn launches(number: usize, ending: Ending, opening: Opening, policy: Policy) -> Vec<Vec<Step>> {
    let turn = turn_launches(number, ending, opening, policy);
    match opening {
        Opening::HandedOff => [vec![compaction()], turn].concat(),
        Opening::Resumed | Opening::Fresh => turn,
    }
}

fn compaction() -> Vec<Step> {
    vec![
        Step::In {
            text: json!({
                "type": "control_request",
                "request_id": "artisan-side-init",
                "request": {
                    "subtype": "initialize",
                    "hooks": { "PreToolUse": [{ "matcher": "*", "hookCallbackIds": ["artisan-side-refuse-tools"] }] },
                },
            })
            .to_string(),
        },
        Step::In {
            text: json!({
                "type": "user",
                "message": {"role": "user", "content": [{"type": "text", "text": ANY}]},
                "parent_tool_use_id": null,
                "session_id": "",
            })
            .to_string(),
        },
        out(&json!({
            "type": "result",
            "subtype": "success",
            "is_error": false,
            "result": "## Objective\n- Work through the numbered steps.",
            "usage": {},
        })),
        Step::Closed,
    ]
}

fn turn_launches(
    number: usize,
    ending: Ending,
    opening: Opening,
    policy: Policy,
) -> Vec<Vec<Step>> {
    let text = message(number);
    match ending {
        Ending::Replies => vec![
            [
                started(&text, opening),
                vec![
                    assistant(&reply(number)),
                    result(&reply(number)),
                    Step::Closed,
                ],
            ]
            .concat(),
        ],
        Ending::EngineDies => vec![
            [
                started(&text, opening),
                vec![assistant(&progress(number)), Step::Exit { code: KILLED }],
            ]
            .concat(),
        ],
        Ending::ForgeDies(stop) => {
            let interrupted = [
                started(&text, opening),
                vec![assistant(&progress(number))],
                stopped(stop),
            ]
            .concat();
            if policy.carries_on() {
                vec![interrupted, carried_on(number)]
            } else {
                vec![interrupted]
            }
        }
        Ending::ForgeDiesBeforeSession(stop) => {
            let interrupted = [accepted(&text, opening), stopped_without_session(stop)].concat();
            if opening == Opening::Resumed && policy.carries_on() {
                vec![interrupted, carried_on(number)]
            } else {
                vec![interrupted]
            }
        }
    }
}

fn carried_on(number: usize) -> Vec<Step> {
    [
        started(CONTINUE, Opening::Resumed),
        vec![
            assistant(&reply(number)),
            result(&reply(number)),
            Step::Closed,
        ],
    ]
    .concat()
}

fn accepted(text: &str, opening: Opening) -> Vec<Step> {
    let replayed = out(&json!({
        "type": "user",
        "isReplay": true,
        "message": {"role": "user", "content": [{"type": "text", "text": text}]},
    }));
    [asked(text, opening), vec![replayed]].concat()
}

fn stopped_without_session(stop: Stop) -> Vec<Step> {
    match stop {
        Stop::Killed => vec![Step::Closed],
        Stop::Graceful => vec![
            interrupt(),
            interrupted(),
            out(&json!({
                "type": "result",
                "subtype": "error_during_execution",
                "is_error": true,
                "terminal_reason": "aborted_streaming",
                "errors": [],
                "usage": {},
            })),
            Step::Closed,
        ],
    }
}

fn interrupt() -> Step {
    Step::In {
        text: json!({
            "type": "control_request",
            "request_id": "artisan-interrupt",
            "request": {"subtype": "interrupt"},
        })
        .to_string(),
    }
}

fn interrupted() -> Step {
    out(&json!({
        "type": "control_response",
        "response": {"subtype": "success", "request_id": "artisan-interrupt", "response": {}},
    }))
}

fn stopped(stop: Stop) -> Vec<Step> {
    match stop {
        Stop::Killed => vec![Step::Closed],
        Stop::Graceful => vec![
            interrupt(),
            interrupted(),
            out(&json!({
                "type": "result",
                "subtype": "error_during_execution",
                "is_error": true,
                "terminal_reason": "aborted_streaming",
                "errors": [],
                "session_id": SESSION,
                "usage": {},
            })),
            Step::Closed,
        ],
    }
}

fn asked(text: &str, opening: Opening) -> Vec<Step> {
    let sent = opening.sent(text);
    let user = Step::In {
        text: json!({
            "type": "user",
            "message": {"role": "user", "content": [{"type": "text", "text": sent}]},
            "parent_tool_use_id": null,
            "session_id": "",
        })
        .to_string(),
    };
    if opening.titled() {
        vec![
            user,
            Step::In {
                text: json!({
                    "type": "control_request",
                    "request_id": "artisan-title",
                    "request": {"subtype": "generate_session_title", "description": sent, "persist": true},
                })
                .to_string(),
            },
        ]
    } else {
        vec![user]
    }
}

fn started(text: &str, opening: Opening) -> Vec<Step> {
    let init = out(&json!({
        "type": "system",
        "subtype": "init",
        "cwd": "/work",
        "session_id": SESSION,
        "model": "claude-haiku-5-5",
        "permissionMode": "default",
        "tools": [],
        "mcp_servers": [],
    }));
    let replayed = out(&json!({
        "type": "user",
        "isReplay": true,
        "message": {"role": "user", "content": [{"type": "text", "text": text}]},
        "session_id": SESSION,
    }));
    let titled = opening.titled().then(|| {
        out(&json!({
            "type": "control_response",
            "response": {
                "subtype": "success",
                "request_id": "artisan-title",
                "response": {"title": "Simulating"},
            },
        }))
    });
    [
        asked(text, opening),
        vec![init, replayed],
        titled.into_iter().collect(),
    ]
    .concat()
}

fn assistant(text: &str) -> Step {
    out(&json!({
        "type": "assistant",
        "message": {
            "id": format!("msg_{}", text.replace([' ', '.'], "_")),
            "role": "assistant",
            "model": "claude-haiku-5-5",
            "stop_reason": "end_turn",
            "content": [{"type": "text", "text": text}],
        },
        "parent_tool_use_id": null,
        "session_id": SESSION,
    }))
}

fn result(text: &str) -> Step {
    out(&json!({
        "type": "result",
        "subtype": "success",
        "is_error": false,
        "result": text,
        "session_id": SESSION,
        "usage": {},
    }))
}

fn out(frame: &serde_json::Value) -> Step {
    Step::Out {
        text: frame.to_string(),
    }
}
