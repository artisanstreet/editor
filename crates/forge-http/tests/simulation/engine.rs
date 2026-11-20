use crate::plan::{Action, Ending, Policy, Stop};
use serde_json::json;
use test_support::transcript::{Source, Step, Transcript};

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

pub(crate) fn script(actions: &[Action]) -> Transcript {
    let mut policy = Policy::default();
    let launches = actions
        .iter()
        .flat_map(|action| match action {
            Action::Turn { number, ending, .. } => launches(*number, *ending, *number == 1, policy),
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

fn launches(number: usize, ending: Ending, first: bool, policy: Policy) -> Vec<Vec<Step>> {
    let text = message(number);
    match ending {
        Ending::Replies => vec![
            [
                started(&text, first),
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
                started(&text, first),
                vec![assistant(&progress(number)), Step::Exit { code: KILLED }],
            ]
            .concat(),
        ],
        Ending::ForgeDies(stop) => {
            let interrupted = [
                started(&text, first),
                vec![assistant(&progress(number))],
                stopped(stop),
            ]
            .concat();
            if policy.carries_on() {
                vec![
                    interrupted,
                    [
                        started(CONTINUE, false),
                        vec![
                            assistant(&reply(number)),
                            result(&reply(number)),
                            Step::Closed,
                        ],
                    ]
                    .concat(),
                ]
            } else {
                vec![interrupted]
            }
        }
    }
}

fn stopped(stop: Stop) -> Vec<Step> {
    match stop {
        Stop::Killed => vec![Step::Closed],
        Stop::Graceful => vec![
            Step::In {
                text: json!({
                    "type": "control_request",
                    "request_id": "artisan-interrupt",
                    "request": {"subtype": "interrupt"},
                })
                .to_string(),
            },
            out(&json!({
                "type": "control_response",
                "response": {"subtype": "success", "request_id": "artisan-interrupt", "response": {}},
            })),
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

fn started(text: &str, first: bool) -> Vec<Step> {
    let user = Step::In {
        text: json!({
            "type": "user",
            "message": {"role": "user", "content": [{"type": "text", "text": text}]},
            "parent_tool_use_id": null,
            "session_id": "",
        })
        .to_string(),
    };
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
    if first {
        vec![
            user,
            Step::In {
                text: json!({
                    "type": "control_request",
                    "request_id": "artisan-title",
                    "request": {"subtype": "generate_session_title", "description": text, "persist": true},
                })
                .to_string(),
            },
            init,
            replayed,
            out(&json!({
                "type": "control_response",
                "response": {
                    "subtype": "success",
                    "request_id": "artisan-title",
                    "response": {"title": "Simulating"},
                },
            })),
        ]
    } else {
        vec![user, init, replayed]
    }
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
