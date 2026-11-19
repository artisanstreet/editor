use crate::{append, emit_json, read_line, touch};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::ffi::OsString;
use std::fs::{File, OpenOptions, TryLockError};
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

#[derive(Debug)]
struct Config {
    spawns: PathBuf,
    input: PathBuf,
    stall_release: bool,
    start_delay: Option<Duration>,
    interrupt_delay: Option<Duration>,
    slow: Duration,
}

impl Config {
    fn from_env() -> Self {
        let path = |name: &str| {
            PathBuf::from(
                std::env::var_os(name)
                    .unwrap_or_else(|| panic!("the fake Codex app-server needs {name}")),
            )
        };
        let millis = |name: &str| {
            std::env::var(name).ok().map(|value| {
                Duration::from_millis(
                    value
                        .parse()
                        .unwrap_or_else(|_invalid| panic!("{name} is a number of milliseconds")),
                )
            })
        };
        Self {
            spawns: path("ARTISAN_TEST_SPAWNS"),
            input: path("ARTISAN_TEST_INPUT"),
            stall_release: std::env::var_os("ARTISAN_TEST_STALL_RELEASE").is_some(),
            start_delay: millis("ARTISAN_TEST_START_DELAY_MS"),
            interrupt_delay: millis("ARTISAN_TEST_INTERRUPT_DELAY_MS"),
            slow: millis("ARTISAN_TEST_SLOW_MS").unwrap_or(Duration::from_millis(1500)),
        }
    }

    fn lock_path(&self, thread: &str) -> PathBuf {
        let mut path = OsString::from(self.spawns.as_os_str());
        path.push(format!("-{thread}.lock"));
        PathBuf::from(path)
    }
}

#[derive(Debug)]
struct Asked {
    thread: String,
    turn: String,
}

#[derive(Debug)]
struct AppServer {
    config: Config,
    writers: HashMap<String, File>,
    threads: u32,
    turns: u32,
    turn: String,
    asked: Option<Asked>,
    answered: bool,
    exit_on_resume: bool,
}

pub(crate) fn app_server() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.get(1).map(String::as_str) != Some("-c")
        || args.get(2).map(String::as_str) != Some("thread_unload_delay_secs=0")
    {
        return ExitCode::FAILURE;
    }
    let config = Config::from_env();
    append(&config.spawns, &format!("{}\n", std::process::id()));
    let mut server = AppServer {
        config,
        writers: HashMap::new(),
        threads: 0,
        turns: 0,
        turn: String::new(),
        asked: None,
        answered: false,
        exit_on_resume: false,
    };
    while let Some(line) = read_line() {
        server.handle(&line);
    }
    ExitCode::SUCCESS
}

impl AppServer {
    fn handle(&mut self, line: &str) {
        append(&self.config.input, &format!("{line}\n"));
        let message: Value = serde_json::from_str(line).unwrap_or(Value::Null);
        let id = &message["id"];
        let method = message["method"].as_str().unwrap_or_default();
        let params = &message["params"];
        let thread = params["threadId"].as_str().unwrap_or_default().to_owned();
        if method.is_empty() && *id == json!(900) && !self.answered {
            self.answer(line.contains("\"accept\""));
        }
        match method {
            "initialize" => respond(id, &json!({ "userAgent": "fake" })),
            "thread/start" => {
                self.threads += 1;
                let thread = format!("thread-{}-{}", std::process::id(), self.threads);
                self.load(id, &thread);
            }
            "thread/resume" if self.exit_on_resume => std::process::exit(0),
            "thread/resume" => self.load(id, &thread),
            "thread/unsubscribe" => self.release(id, &thread),
            "thread/name/set" => {
                std::fs::write("named", params.to_string()).expect("the fake records the name");
                respond(id, &json!({}));
            }
            "turn/steer" => refuse(id, -32600, "no active turn to steer"),
            "turn/interrupt" => self.interrupt(id, &thread),
            "turn/start" => self.start(id, thread, params),
            _ => {}
        }
    }

    fn answer(&mut self, accepted: bool) {
        self.answered = true;
        let word = if accepted { "approved" } else { "refused" };
        let (thread, turn) = self
            .asked
            .as_ref()
            .map_or(("", ""), |asked| (&asked.thread, &asked.turn));
        emit_json(&json!({
            "method": "item/completed",
            "params": {
                "item": { "type": "agentMessage", "id": "msg-ask", "text": format!("ask on {thread} was {word}"), "phase": "final_answer" },
                "threadId": thread,
                "turnId": turn,
            },
        }));
        completed(thread, turn, "completed");
    }

    fn load(&mut self, id: &Value, thread: &str) {
        if !self.writers.contains_key(thread) {
            let writer = OpenOptions::new()
                .create(true)
                .truncate(false)
                .write(true)
                .open(self.config.lock_path(thread))
                .expect("the fake opens the thread's writer lock");
            match writer.try_lock() {
                Ok(()) => {
                    self.writers.insert(thread.to_owned(), writer);
                }
                Err(TryLockError::WouldBlock) => {
                    refuse(id, -32000, "already has an active writer");
                    return;
                }
                Err(TryLockError::Error(error)) => {
                    panic!("the fake could not lock the writer of {thread}: {error}")
                }
            }
        }
        respond(
            id,
            &json!({ "thread": { "id": thread }, "model": "fake-model" }),
        );
    }

    fn release(&mut self, id: &Value, thread: &str) {
        if self.config.stall_release {
            return;
        }
        respond(id, &json!({ "status": "unsubscribed" }));
        std::thread::sleep(Duration::from_millis(100));
        self.writers.remove(thread);
        emit_json(&json!({ "method": "thread/closed", "params": { "threadId": thread } }));
    }

    fn interrupt(&self, id: &Value, thread: &str) {
        if let Some(delay) = self.config.interrupt_delay {
            std::thread::sleep(delay);
            respond(id, &json!({}));
            completed(thread, &self.turn, "interrupted");
        }
    }

    fn start(&mut self, id: &Value, thread: String, params: &Value) {
        if let Some(delay) = self.config.start_delay {
            let mut starting = OsString::from(self.config.input.as_os_str());
            starting.push(".starting");
            touch(PathBuf::from(starting));
            std::thread::sleep(delay);
        }
        self.turns += 1;
        self.turn = format!("turn-{}", self.turns);
        let turn = self.turn.clone();
        let text = params["input"]
            .as_array()
            .and_then(|items| items.iter().rev().find_map(|item| item["text"].as_str()))
            .unwrap_or_default()
            .to_owned();
        respond(id, &json!({ "turn": { "id": turn } }));
        self.exit_on_resume = text == "then exit on resume";
        match text.as_str() {
            "jam" => std::thread::sleep(Duration::from_secs(30)),
            "flood" => {
                let delta = "x".repeat(1024 * 1024);
                for _ in 0..48 {
                    emit_json(&json!({
                        "method": "item/agentMessage/delta",
                        "params": { "threadId": thread, "turnId": turn, "itemId": format!("msg-{turn}"), "delta": delta },
                    }));
                }
                completed(&thread, &turn, "completed");
            }
            "ask" => {
                self.asked = Some(Asked {
                    thread: thread.clone(),
                    turn: turn.clone(),
                });
                std::thread::spawn(move || {
                    std::thread::sleep(Duration::from_millis(500));
                    emit_json(&json!({
                        "id": 900,
                        "method": "item/commandExecution/requestApproval",
                        "params": { "threadId": thread, "turnId": turn, "itemId": "cmd-1", "command": "ls" },
                    }));
                });
            }
            _ => {
                let slow = self.config.slow;
                std::thread::spawn(move || reply(&thread, &turn, &text, slow));
            }
        }
    }
}

fn reply(thread: &str, turn: &str, text: &str, slow: Duration) {
    std::thread::sleep(Duration::from_millis(300));
    if text == "slow" {
        std::thread::sleep(slow);
    }
    let child = format!("child-{thread}");
    let child_message = || {
        emit_json(&json!({
            "method": "item/completed",
            "params": {
                "item": { "type": "agentMessage", "id": format!("child-msg-{thread}"), "text": format!("child of {thread}"), "phase": "final_answer" },
                "threadId": child,
                "turnId": "child-turn",
            },
        }));
    };
    if text == "spawn" {
        emit_json(&json!({
            "method": "thread/started",
            "params": { "thread": { "id": child, "parentThreadId": thread, "agentNickname": "Helper" } },
        }));
        child_message();
    }
    if text == "activity" || text == "activity-followup" {
        let kind = if text == "activity" {
            "started"
        } else {
            "interacted"
        };
        child_message();
        emit_json(&json!({
            "method": "item/completed",
            "params": {
                "item": { "type": "subAgentActivity", "id": format!("spawn-{thread}"), "kind": kind, "agentThreadId": child, "agentPath": "/root/worker", "model": "worker-model" },
                "threadId": thread,
                "turnId": turn,
            },
        }));
        emit_json(&json!({
            "method": "item/completed",
            "params": {
                "item": { "type": "subAgentActivity", "id": format!("done-{thread}"), "kind": "completed", "agentThreadId": child, "agentPath": "/root/worker" },
                "threadId": thread,
                "turnId": turn,
            },
        }));
    }
    emit_json(&json!({
        "method": "item/completed",
        "params": {
            "item": { "type": "agentMessage", "id": format!("msg-{turn}"), "text": format!("reply on {thread} to {text}"), "phase": "final_answer" },
            "threadId": thread,
            "turnId": turn,
        },
    }));
    completed(thread, turn, "completed");
}

fn completed(thread: &str, turn: &str, status: &str) {
    emit_json(&json!({
        "method": "turn/completed",
        "params": { "threadId": thread, "turn": { "id": turn, "items": [], "status": status, "error": null } },
    }));
}

fn respond(id: &Value, result: &Value) {
    emit_json(&json!({ "id": id, "result": result }));
}

fn refuse(id: &Value, code: i64, message: &str) {
    emit_json(&json!({ "id": id, "error": { "code": code, "message": message } }));
}
