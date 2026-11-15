use super::decoder::ClaudeDecoder;
use crate::EngineEvent;
use artisan_api::{ItemKind, Phase};
use serde_json::{Value, json};

const MEMORY_FILE: &str = "/home/u/.claude/projects/-home-u-app/memory/plan.md";

fn says(id: &str, text: &str, then: &[Value], stop_reason: &str) -> String {
    let mut content = vec![json!({"type": "text", "text": text})];
    content.extend(then.iter().cloned());
    json!({
        "type": "assistant",
        "message": {"id": id, "stop_reason": stop_reason, "content": content},
    })
    .to_string()
}

fn replies(id: &str, text: &str) -> String {
    says(id, text, &[], "end_turn")
}

fn runs(id: &str, command: &str) -> Value {
    json!({"type": "tool_use", "id": id, "name": "Bash", "input": {"command": command}})
}

fn writes(id: &str, path: &str) -> Value {
    json!({
        "type": "tool_use",
        "id": id,
        "name": "Write",
        "input": {"file_path": path, "content": "notes"},
    })
}

fn user_writes() -> String {
    json!({"type": "user", "isReplay": true, "message": {"content": "and this too"}}).to_string()
}

fn turn_ends() -> String {
    json!({"type": "result", "subtype": "success", "usage": {}}).to_string()
}

fn reply_shown(frames: &[String]) -> Vec<String> {
    let mut decoder = ClaudeDecoder::default();
    let mut said: Vec<(String, String, Phase)> = Vec::new();
    for frame in frames {
        for event in decoder.feed(frame).unwrap() {
            let EngineEvent::Item {
                key,
                kind: ItemKind::AssistantMessage { text, phase },
                ..
            } = event
            else {
                continue;
            };
            let text = text.inline().unwrap_or_default().to_owned();
            match said.iter_mut().find(|(known, ..)| *known == key) {
                Some(entry) => *entry = (key, text, phase),
                None => said.push((key, text, phase)),
            }
        }
    }
    said.into_iter()
        .filter(|(_, text, phase)| *phase == Phase::Final && !text.is_empty())
        .map(|(_, text, _)| text)
        .collect()
}

#[test]
fn a_waiting_note_folds_once_the_woken_turn_works_again() {
    let shown = reply_shown(&[
        says(
            "m1",
            "Starting the build.",
            &[runs("t1", "make &")],
            "tool_use",
        ),
        replies("m2", "The build is running; I'll report when it ends."),
        turn_ends(),
        says(
            "m3",
            "It ended. Checking the log.",
            &[runs("t2", "tail log")],
            "tool_use",
        ),
        replies("m4", "The build passed."),
        turn_ends(),
    ]);
    assert_eq!(shown, ["The build passed."]);
}

#[test]
fn an_answer_stays_when_only_memory_keeping_follows_it() {
    let shown = reply_shown(&[
        says("m1", "Looking.", &[runs("t1", "ls")], "tool_use"),
        says(
            "m2",
            "Here is the full answer.",
            &[writes("t2", MEMORY_FILE)],
            "tool_use",
        ),
        replies("m3", "Saved for next time."),
        turn_ends(),
    ]);
    assert_eq!(shown, ["Here is the full answer.", "Saved for next time."]);
}

#[test]
fn a_file_written_outside_memory_is_work() {
    let shown = reply_shown(&[
        says(
            "m1",
            "Writing the fix.",
            &[writes("t1", "/home/u/app/src/main.rs")],
            "tool_use",
        ),
        replies("m2", "Fixed."),
        turn_ends(),
    ]);
    assert_eq!(shown, ["Fixed."]);
}

#[test]
fn a_woken_turn_that_does_no_work_adds_to_the_reply() {
    let shown = reply_shown(&[
        says("m1", "Looking.", &[runs("t1", "ls")], "tool_use"),
        replies("m2", "Here is the report."),
        turn_ends(),
        replies("m3", "A leftover timer fired; nothing changed."),
        turn_ends(),
    ]);
    assert_eq!(
        shown,
        [
            "Here is the report.",
            "A leftover timer fired; nothing changed."
        ]
    );
}

#[test]
fn a_report_stays_when_the_woken_turn_works_again() {
    let report = "The fix is in PR 15.\n\nCI is still running on it.";
    let shown = reply_shown(&[
        says("m1", "Fixing.", &[runs("t1", "make")], "tool_use"),
        replies("m2", report),
        turn_ends(),
        says("m3", "CI ended.", &[runs("t2", "gh pr checks")], "tool_use"),
        replies("m4", "PR 15 is green."),
        turn_ends(),
    ]);
    assert_eq!(shown, [report, "PR 15 is green."]);
}

#[test]
fn a_note_the_user_answered_stays() {
    let shown = reply_shown(&[
        says("m1", "Starting.", &[runs("t1", "make &")], "tool_use"),
        replies("m2", "Which colour do you want?"),
        turn_ends(),
        user_writes(),
        says("m3", "Painting.", &[runs("t2", "paint")], "tool_use"),
        replies("m4", "Painted it blue."),
        turn_ends(),
    ]);
    assert_eq!(shown, ["Which colour do you want?", "Painted it blue."]);
}

#[test]
fn only_the_latest_note_of_idle_wake_ups_stays() {
    let shown = reply_shown(&[
        says("m1", "Looking.", &[runs("t1", "ls")], "tool_use"),
        replies("m2", "Here is the report."),
        turn_ends(),
        replies("m3", "A timer fired; nothing changed."),
        turn_ends(),
        replies("m4", "Another timer fired; still nothing."),
        turn_ends(),
    ]);
    assert_eq!(
        shown,
        ["Here is the report.", "Another timer fired; still nothing."]
    );
}
