//! One ephemeral, read-only Codex app-server thread that writes a handoff
//! summary.

use artisan_domain::RootPath;
use artisan_native_engine::VerifiedCodexLaunch;
use serde_json::{Value, json};
use tokio::io::BufReader;

use super::super::codex::{
    initialize_params, notification_line, read_codex_line_bounded, request_line, write_codex_line,
};
use super::SUMMARY_INSTRUCTIONS;

/// Asks Codex for a summary of the transcript in `prompt` and returns the
/// reply text.
///
/// The thread is ephemeral (nothing is saved to the account's history),
/// read-only, never asks for approval, and has its shell and web search
/// turned off, so it can neither act nor pause. The caller bounds the run's
/// duration; dropping the future kills the app-server.
///
/// # Errors
///
/// Returns a bounded description of the step that failed. It never contains
/// transcript text.
pub(crate) async fn summarize_with_codex(
    launch: &VerifiedCodexLaunch,
    project_root: &RootPath,
    model: Option<&str>,
    prompt: &str,
) -> Result<String, String> {
    let mut child = super::super::process::spawn_codex_engine(launch, project_root)
        .map_err(|error| format!("spawning the Codex summary run: {error}"))?;
    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| "the Codex summary run had no stdin".to_owned())?;
    let mut reader = BufReader::new(
        child
            .stdout
            .take()
            .ok_or_else(|| "the Codex summary run had no stdout".to_owned())?,
    );
    // Nothing reads stderr diagnostics here; drain it so a chatty server
    // never blocks on a full pipe.
    if let Some(mut stderr) = child.stderr.take() {
        tokio::spawn(async move {
            let _ = tokio::io::copy(&mut stderr, &mut tokio::io::sink()).await;
        });
    }
    let mut session = Session {
        stdin: &mut stdin,
        reader: &mut reader,
        line: String::new(),
    };

    session
        .request(
            1,
            "initialize",
            &initialize_params("artisan-editor", "0.3.0"),
        )
        .await?;
    session.notify("initialized").await?;
    let started = session
        .request(
            2,
            "thread/start",
            &json!({
                "model": model,
                "ephemeral": true,
                "approvalPolicy": "never",
                "sandbox": "read-only",
                "cwd": project_root.as_str(),
                "baseInstructions": SUMMARY_INSTRUCTIONS,
                "config": {
                    "model_reasoning_effort": "low",
                    "web_search": "disabled",
                    "features.shell_tool": false,
                },
            }),
        )
        .await?;
    let thread_id = started
        .pointer("/result/thread/id")
        .and_then(Value::as_str)
        .ok_or_else(|| "the Codex summary thread has no id".to_owned())?
        .to_owned();
    session
        .write(&request_line(
            3,
            "turn/start",
            &json!({
                "threadId": thread_id,
                "input": [{ "type": "text", "text": prompt }],
            }),
        ))
        .await?;
    let summary = session.final_message(3, &thread_id).await;
    // Closing stdin ends the app-server; an ephemeral thread leaves nothing
    // behind, so one that lingers is killed on drop instead of awaited.
    drop(stdin);
    let _ = tokio::time::timeout(EXIT_GRACE, child.wait()).await;
    summary
}

/// How long the app-server may take to exit once its answer is read.
const EXIT_GRACE: std::time::Duration = std::time::Duration::from_secs(2);

struct Session<'a> {
    stdin: &'a mut tokio::process::ChildStdin,
    reader: &'a mut BufReader<tokio::process::ChildStdout>,
    line: String,
}

impl Session<'_> {
    async fn write(&mut self, line: &str) -> Result<(), String> {
        write_codex_line(self.stdin, line)
            .await
            .map_err(|_| "writing to the Codex summary run failed".to_owned())
    }

    async fn notify(&mut self, method: &str) -> Result<(), String> {
        self.write(&notification_line(method)).await
    }

    /// Sends one request and returns its reply, skipping interleaved
    /// notifications.
    async fn request(&mut self, id: u64, method: &str, params: &Value) -> Result<Value, String> {
        self.write(&request_line(id, method, params)).await?;
        loop {
            let frame = self.next_frame().await?;
            if frame.get("id").and_then(Value::as_u64) != Some(id) {
                continue;
            }
            if frame.get("error").is_some() {
                return Err(format!("the Codex summary run refused `{method}`"));
            }
            return Ok(frame);
        }
    }

    /// Follows the summary turn to completion and returns its last answer.
    async fn final_message(
        &mut self,
        turn_request: u64,
        thread_id: &str,
    ) -> Result<String, String> {
        let mut answer: Option<String> = None;
        loop {
            let frame = self.next_frame().await?;
            if frame.get("id").and_then(Value::as_u64) == Some(turn_request)
                && frame.get("error").is_some()
            {
                return Err("the Codex summary run refused `turn/start`".to_owned());
            }
            let Some(method) = frame.get("method").and_then(Value::as_str) else {
                continue;
            };
            // A server request (approval or question) can only stall a
            // tool-less turn; give up rather than wait for the deadline.
            if frame.get("id").is_some() {
                return Err(format!(
                    "the Codex summary run asked for interaction (`{method}`)"
                ));
            }
            if frame.pointer("/params/threadId").and_then(Value::as_str) != Some(thread_id) {
                continue;
            }
            match method {
                "item/completed"
                    if frame.pointer("/params/item/type").and_then(Value::as_str)
                        == Some("agentMessage")
                        && frame.pointer("/params/item/phase").and_then(Value::as_str)
                            != Some("commentary") =>
                {
                    if let Some(text) = frame.pointer("/params/item/text").and_then(Value::as_str) {
                        answer = Some(text.to_owned());
                    }
                }
                "turn/completed" => {
                    let status = frame
                        .pointer("/params/turn/status")
                        .and_then(Value::as_str)
                        .unwrap_or("unknown");
                    if status != "completed" {
                        return Err(format!("the Codex summary turn ended {status}"));
                    }
                    return answer
                        .ok_or_else(|| "the Codex summary turn wrote no answer".to_owned());
                }
                _ => {}
            }
        }
    }

    async fn next_frame(&mut self) -> Result<Value, String> {
        self.line.clear();
        let read = read_codex_line_bounded(self.reader, &mut self.line)
            .await
            .map_err(|_| "reading from the Codex summary run failed".to_owned())?;
        if read == 0 {
            return Err("the Codex summary run closed its output".to_owned());
        }
        // Unparseable lines are noise, not frames.
        Ok(serde_json::from_str(&self.line).unwrap_or(Value::Null))
    }
}
