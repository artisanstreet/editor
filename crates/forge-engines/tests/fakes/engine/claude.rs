use crate::{alive, drain_input, emit, read_line, touch};
use std::io::{BufRead as _, BufReader, Read as _, Write as _};
use std::path::PathBuf;
use std::process::{ExitCode, Stdio};
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::time::Duration;

const REPLAYED: &str = r#"{"type":"user","isReplay":true,"message":{"content":"go"}}"#;
const INIT: &str = r#"{"type":"system","subtype":"init","session_id":"s1","model":"m"}"#;
const AGENT_STARTED: &str = r#"{"type":"system","subtype":"background_tasks_changed","tasks":[{"task_id":"a1","task_type":"local_agent"}]}"#;
const NO_BACKGROUND: &str = r#"{"type":"system","subtype":"background_tasks_changed","tasks":[]}"#;

pub(crate) fn interrupt() -> ExitCode {
    let _alive = alive("cli");
    read_line();
    touch("ready");
    let ignore = std::env::var("IGNORE_INTERRUPT").is_ok_and(|value| value == "true");
    while let Some(line) = read_line() {
        if line.contains(r#""subtype":"interrupt""#) {
            touch("interrupted");
            if !ignore {
                std::thread::sleep(Duration::from_millis(30));
                return ExitCode::SUCCESS;
            }
            std::thread::sleep(Duration::from_secs(30));
        }
    }
    ExitCode::SUCCESS
}

pub(crate) fn closes_output() -> ExitCode {
    read_line();
    close_stdout();
    touch("closed");
    std::thread::sleep(Duration::from_secs(60));
    ExitCode::SUCCESS
}

#[expect(
    unsafe_code,
    clippy::undocumented_unsafe_blocks,
    reason = "closing the inherited stdout while the process lives on is the scenario; nothing writes to it afterwards"
)]
fn close_stdout() {
    #[cfg(unix)]
    {
        use std::os::fd::{AsRawFd as _, FromRawFd as _, OwnedFd};
        drop(unsafe { OwnedFd::from_raw_fd(std::io::stdout().as_raw_fd()) });
    }
    #[cfg(windows)]
    {
        use std::os::windows::io::{AsRawHandle as _, FromRawHandle as _, OwnedHandle};
        drop(unsafe { OwnedHandle::from_raw_handle(std::io::stdout().as_raw_handle()) });
    }
}

pub(crate) fn monitor_steer() -> ExitCode {
    read_line();
    emit(REPLAYED);
    emit(INIT);
    emit(
        r#"{"type":"system","subtype":"background_tasks_changed","tasks":[{"task_id":"m1","task_type":"local_bash","description":"Tail a log"}]}"#,
    );
    emit(r#"{"type":"result","subtype":"success","result":"All done.","usage":{}}"#);
    touch("replied");
    if let Some(steer) = read_line() {
        std::fs::write("steer", steer).expect("the fake records the steer");
        emit(r#"{"type":"user","isReplay":true,"message":{"content":"steer"}}"#);
        emit(r#"{"type":"result","subtype":"success","result":"Changed course.","usage":{}}"#);
        emit(NO_BACKGROUND);
        drain_input();
    } else {
        touch("input_closed_under_the_monitor");
        std::thread::sleep(Duration::from_secs(30));
    }
    ExitCode::SUCCESS
}

#[derive(Debug)]
enum BrokenOutput {
    Malformed,
    Oversized { bytes: u64 },
}

pub(crate) fn protocol_error() -> ExitCode {
    let _alive = alive("parent");
    let after_result =
        std::env::var("ARTISAN_TEST_AFTER_RESULT").is_ok_and(|value| value == "true");
    let output =
        std::env::var("ARTISAN_TEST_LINE_BYTES").map_or(BrokenOutput::Malformed, |bytes| {
            BrokenOutput::Oversized {
                bytes: bytes
                    .parse()
                    .expect("ARTISAN_TEST_LINE_BYTES is a byte count"),
            }
        });
    let mut descendant = std::process::Command::new(
        std::env::current_exe().expect("the fake engine knows its own path"),
    )
    .env("ARTISAN_FAKE", "hold")
    .env("ARTISAN_TEST_HOLD", "child")
    .stdin(Stdio::null())
    .stdout(Stdio::inherit())
    .stderr(Stdio::piped())
    .spawn()
    .expect("the fake engine starts a descendant");
    let mut held = String::new();
    BufReader::new(
        descendant
            .stderr
            .take()
            .expect("the descendant's stderr is piped"),
    )
    .read_line(&mut held)
    .expect("the descendant reports that it holds its marker");
    read_line();
    if after_result {
        emit(r#"{"type":"result","subtype":"success","result":"done","usage":{}}"#);
    }
    match output {
        BrokenOutput::Malformed => emit("{broken-json"),
        BrokenOutput::Oversized { bytes } => {
            let mut stdout = std::io::stdout().lock();
            std::io::copy(&mut std::io::repeat(0).take(bytes), &mut stdout)
                .and_then(|_written| stdout.write_all(b"\n"))
                .and_then(|()| stdout.flush())
                .expect("the harness reads the oversized line");
        }
    }
    descendant
        .wait()
        .expect("the fake engine waits for its descendant");
    ExitCode::SUCCESS
}

pub(crate) fn background_agent() -> ExitCode {
    read_line();
    emit(REPLAYED);
    emit(INIT);
    emit(AGENT_STARTED);
    emit(r#"{"type":"result","subtype":"success","result":"started","usage":{}}"#);
    let input = follow_input();
    std::thread::sleep(Duration::from_secs(1));
    if closed(&input) {
        emit(
            r#"{"type":"result","subtype":"error_during_execution","errors":["input closed while the agent worked"],"usage":{}}"#,
        );
        return ExitCode::SUCCESS;
    }
    emit(NO_BACKGROUND);
    emit(r#"{"type":"result","subtype":"success","result":"agent reported","usage":{}}"#);
    while input.recv().is_ok() {}
    ExitCode::SUCCESS
}

pub(crate) fn long_turn() -> ExitCode {
    read_line();
    emit(REPLAYED);
    emit(INIT);
    emit(AGENT_STARTED);
    emit(r#"{"type":"result","subtype":"success","result":"started","usage":{}}"#);
    emit(NO_BACKGROUND);
    emit(
        r#"{"type":"assistant","message":{"id":"m2","role":"assistant","content":[{"type":"text","text":"Reading the report."}]}}"#,
    );
    let input = follow_input();
    std::thread::sleep(Duration::from_secs(1));
    if closed(&input) {
        emit(
            r#"{"type":"result","subtype":"error_during_execution","errors":["input closed mid-turn"],"usage":{}}"#,
        );
        return ExitCode::SUCCESS;
    }
    emit(
        r#"{"type":"system","subtype":"background_tasks_changed","tasks":[{"task_id":"a2","task_type":"local_agent"}]}"#,
    );
    emit(r#"{"type":"result","subtype":"success","result":"launched the next agent","usage":{}}"#);
    emit(NO_BACKGROUND);
    emit(r#"{"type":"result","subtype":"success","result":"agent reported","usage":{}}"#);
    while input.recv().is_ok() {}
    ExitCode::SUCCESS
}

fn follow_input() -> Receiver<String> {
    let (lines, input) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        while let Some(line) = read_line() {
            if lines.send(line).is_err() {
                break;
            }
        }
    });
    input
}

fn closed(input: &Receiver<String>) -> bool {
    matches!(
        input.recv_timeout(Duration::from_millis(50)),
        Err(RecvTimeoutError::Disconnected)
    )
}

pub(crate) fn side_fork() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let passed = |expected: &[&str]| {
        args.windows(expected.len()).any(|window| {
            window
                .iter()
                .map(String::as_str)
                .eq(expected.iter().copied())
        })
    };
    if !passed(&["--resume", "s1"]) || !passed(&["--fork-session"]) {
        return ExitCode::from(3);
    }
    if !passed(&["--dangerously-skip-permissions"]) {
        return ExitCode::from(4);
    }
    let cwd = std::env::current_dir().expect("the fake engine has a working directory");
    let project: String = cwd
        .to_string_lossy()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    let projects = PathBuf::from(
        std::env::var_os("CLAUDE_CONFIG_DIR").expect("the harness sets CLAUDE_CONFIG_DIR"),
    )
    .join("projects")
    .join(project);
    let init = read_line().unwrap_or_default();
    let hooked = init
        .split_once(r#""PreToolUse""#)
        .is_some_and(|(_, hook)| hook.contains(r#""hookCallbackIds""#));
    if !hooked {
        return ExitCode::from(6);
    }
    read_line();
    emit(r#"{"type":"system","subtype":"init","session_id":"f1","model":"m"}"#);
    if let Err(error) = std::fs::write(projects.join("f1.jsonl"), "{}\n") {
        writeln!(
            std::io::stderr(),
            "the fork's transcript cannot be written in {}: {error}",
            projects.display()
        )
        .expect("the fake engine writes to stderr");
        return ExitCode::from(7);
    }
    emit(
        r#"{"type":"control_request","request_id":"r1","request":{"subtype":"hook_callback","callback_id":"artisan-side-refuse-tools","input":{"hook_event_name":"PreToolUse","tool_name":"Bash","tool_input":{"command":"rm -rf /"}}}}"#,
    );
    let answer = read_line().unwrap_or_default();
    if !answer.contains(r#""permissionDecision":"deny""#) {
        return ExitCode::from(5);
    }
    emit(
        r#"{"type":"result","subtype":"success","is_error":false,"session_id":"f1","result":" Ship it. ","usage":{"input_tokens":10,"cache_read_input_tokens":9000,"cache_creation_input_tokens":0,"output_tokens":4}}"#,
    );
    drain_input();
    ExitCode::SUCCESS
}
