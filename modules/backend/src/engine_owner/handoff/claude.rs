//! One tool-less, unpersisted `claude -p` run that writes a handoff summary.

use artisan_domain::RootPath;
use artisan_native_engine::VerifiedClaudeLaunch;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use super::{SUMMARY_INSTRUCTIONS, SUMMARY_OUTPUT_MAX_BYTES};

/// Asks Claude for a summary of the transcript in `prompt` and returns the
/// reply text.
///
/// The run is print-mode, saves no session, loads no user settings, and has
/// no tools, so it can neither act nor pause on an approval. The prompt
/// travels on stdin because a long transcript exceeds the argument size
/// limit. The caller bounds the run's duration; dropping the future kills
/// the child.
///
/// # Errors
///
/// Returns a bounded description of the step that failed. It never contains
/// transcript text.
pub(crate) async fn summarize_with_claude(
    launch: &VerifiedClaudeLaunch,
    project_root: &RootPath,
    model: Option<&str>,
    prompt: &str,
) -> Result<String, String> {
    let mut args = vec![
        "-p".to_owned(),
        "--output-format".to_owned(),
        "json".to_owned(),
        "--no-session-persistence".to_owned(),
        "--tools".to_owned(),
        String::new(),
        "--setting-sources".to_owned(),
        String::new(),
        "--effort".to_owned(),
        "low".to_owned(),
        "--system-prompt".to_owned(),
        SUMMARY_INSTRUCTIONS.to_owned(),
    ];
    if let Some(model) = model {
        args.push("--model".to_owned());
        args.push(model.to_owned());
    }
    let mut child = super::super::process::spawn_claude_engine(launch, project_root, &args, &[])
        .map_err(|error| format!("spawning the Claude summary run: {error}"))?;
    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| "the Claude summary run had no stdin".to_owned())?;
    let mut stdout = child
        .stdout
        .take()
        .ok_or_else(|| "the Claude summary run had no stdout".to_owned())?
        .take(SUMMARY_OUTPUT_MAX_BYTES);
    let stderr = child.stderr.take();
    let mut output = Vec::new();
    let (written, read, ()) = tokio::join!(
        async {
            stdin.write_all(prompt.as_bytes()).await?;
            stdin.shutdown().await
        },
        stdout.read_to_end(&mut output),
        async {
            if let Some(stderr) = stderr {
                let _ = tokio::io::copy(
                    &mut stderr.take(SUMMARY_OUTPUT_MAX_BYTES),
                    &mut tokio::io::sink(),
                )
                .await;
            }
        },
    );
    written.map_err(|error| format!("writing the Claude summary request: {error}"))?;
    read.map_err(|error| format!("reading the Claude summary run's output: {error}"))?;
    let status = child
        .wait()
        .await
        .map_err(|error| format!("waiting for the Claude summary run: {error}"))?;
    if !status.success() {
        return Err(format!("the Claude summary run exited with {status}"));
    }
    summary_from_result(&output)
}

/// Extracts the reply from print mode's single JSON result object.
fn summary_from_result(output: &[u8]) -> Result<String, String> {
    let result: serde_json::Value = serde_json::from_slice(output)
        .map_err(|error| format!("parsing the Claude summary run's result: {error}"))?;
    if result.get("is_error").and_then(serde_json::Value::as_bool) != Some(false) {
        let subtype = result
            .get("subtype")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("unknown");
        return Err(format!(
            "the Claude summary run reported an error ({subtype})"
        ));
    }
    result
        .get("result")
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| "the Claude summary run returned no text".to_owned())
}

#[cfg(test)]
mod tests {
    use super::summary_from_result;

    #[test]
    fn reads_the_reply_of_a_successful_result() {
        let output = br###"{"type":"result","subtype":"success","is_error":false,"result":"## Objective\n- ship it"}"###;
        assert_eq!(
            summary_from_result(output).as_deref(),
            Ok("## Objective\n- ship it")
        );
    }

    #[test]
    fn refuses_error_results_and_missing_text() {
        for output in [
            br#"{"type":"result","subtype":"error_max_turns","is_error":true}"#.as_slice(),
            br#"{"type":"result","subtype":"success","is_error":false}"#.as_slice(),
            br#"{"type":"result","subtype":"success","result":"text"}"#.as_slice(),
            b"not json".as_slice(),
        ] {
            assert!(summary_from_result(output).is_err());
        }
    }
}
