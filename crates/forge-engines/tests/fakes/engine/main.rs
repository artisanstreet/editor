mod claude;
mod codex;
mod opencode;

use std::fs::{File, OpenOptions};
use std::io::{BufRead as _, Write as _};
use std::path::Path;
use std::process::ExitCode;
use std::time::Duration;

fn main() -> ExitCode {
    let scenario = std::env::var("ARTISAN_FAKE").unwrap_or_default();
    match scenario.as_str() {
        "codex-app-server" => codex::app_server(),
        "claude-interrupt" => claude::interrupt(),
        "claude-closes-output" => claude::closes_output(),
        "claude-monitor-steer" => claude::monitor_steer(),
        "claude-protocol-error" => claude::protocol_error(),
        "claude-background-agent" => claude::background_agent(),
        "claude-long-turn" => claude::long_turn(),
        "claude-side-fork" => claude::side_fork(),
        "opencode-title" => opencode::serve(opencode::Mode::Title),
        "opencode-clear-goal" => opencode::serve(opencode::Mode::ClearGoal),
        "opencode-compaction" => opencode::serve(opencode::Mode::compaction()),
        "opencode-cancellation" => opencode::serve(opencode::Mode::cancellation()),
        "opencode-oversized-output" => opencode::serve(opencode::Mode::OversizedOutput),
        "hold" => hold(),
        unknown => {
            writeln!(
                std::io::stderr(),
                "the fake engine has no scenario {unknown:?}"
            )
            .expect("the fake engine writes to stderr");
            ExitCode::from(2)
        }
    }
}

fn hold() -> ExitCode {
    let name = std::env::var("ARTISAN_TEST_HOLD").expect("a held process names its marker");
    let _alive = alive(&name);
    let mut stderr = std::io::stderr();
    writeln!(stderr, "held")
        .and_then(|()| stderr.flush())
        .expect("the parent reads that the marker is held");
    std::thread::sleep(Duration::from_secs(30));
    ExitCode::SUCCESS
}

fn emit(line: &str) {
    let mut stdout = std::io::stdout().lock();
    stdout
        .write_all(line.as_bytes())
        .and_then(|()| stdout.write_all(b"\n"))
        .and_then(|()| stdout.flush())
        .expect("the harness reads the fake engine's output");
}

fn emit_json(value: &serde_json::Value) {
    emit(&value.to_string());
}

fn read_line() -> Option<String> {
    let mut line = String::new();
    let read = std::io::stdin()
        .lock()
        .read_line(&mut line)
        .expect("the harness writes UTF-8 lines");
    (read > 0).then(|| line.trim_end_matches(['\r', '\n']).to_owned())
}

fn drain_input() {
    while read_line().is_some() {}
}

fn touch(path: impl AsRef<Path>) {
    std::fs::write(path.as_ref(), b"").expect("the fake engine writes its markers");
}

fn append(path: &Path, text: &str) {
    OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .and_then(|mut file| file.write_all(text.as_bytes()))
        .expect("the fake engine appends to its logs");
}

fn alive(name: &str) -> File {
    std::fs::write(format!("{name}.pid"), std::process::id().to_string())
        .expect("the fake engine writes its pid");
    let marker = OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(format!("{name}.alive"))
        .expect("the fake engine opens its liveness marker");
    marker
        .lock()
        .expect("the fake engine holds its liveness marker");
    marker
}
