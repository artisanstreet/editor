use super::*;
use aqua_core::SemanticNode;
use aqua_text::{SharedFonts, SharedRuns};

fn replying(text: &str) -> Turn {
    Turn::Replying {
        engine: "Claude",
        text: text.to_owned(),
    }
}

#[test]
fn a_reply_is_announced_when_it_starts() {
    assert_eq!(
        announcement(&replying("The fix")),
        Announcement {
            text: "Claude is replying".to_owned(),
            busy: true,
        }
    );
}

#[test]
fn streaming_text_is_announced_a_whole_paragraph_at_a_time() {
    let first = "The bug is in the parser. It drops the last line.";
    assert_eq!(
        announcement(&replying(&format!("{first}\n\nThe fix is"))).text,
        first
    );
    assert_eq!(
        announcement(&replying(&format!("{first}\n\nThe fix is to keep"))).text,
        first,
        "more tokens inside the next paragraph change nothing"
    );
    assert_eq!(
        announcement(&replying(&format!(
            "{first}\n\nThe fix is to keep it.\n\n\n\nDone"
        )))
        .text,
        "The fix is to keep it."
    );
}

#[test]
fn completion_and_stopping_are_announced_and_end_the_busy_state() {
    assert_eq!(
        announcement(&Turn::Finished),
        Announcement {
            text: "Reply finished".to_owned(),
            busy: false,
        }
    );
    assert_eq!(announcement(&Turn::Stopped).text, "Reply stopped");
    assert_eq!(announcement(&Turn::Idle).text, "");
}

#[test]
fn a_failure_announces_its_error() {
    assert_eq!(
        announcement(&Turn::Failed {
            error: Some("Rate limited".to_owned())
        }),
        Announcement {
            text: "Reply failed: Rate limited".to_owned(),
            busy: false,
        }
    );
    assert_eq!(
        announcement(&Turn::Failed { error: None }).text,
        "Reply failed"
    );
}

#[test]
fn work_before_the_reply_is_announced_once_for_the_whole_stretch() {
    let working = announcement(&Turn::Working { engine: "Codex" });
    assert_eq!(working.text, "Codex is working");
    assert!(working.busy);
    let waiting = announcement(&Turn::WaitingOnYou { engine: "Codex" });
    assert_eq!(waiting.text, "Codex is waiting for you");
    assert!(!waiting.busy);
}

fn semantics(turn: &Turn) -> Vec<SemanticNode> {
    let announcement = announcement(turn);
    let transcript = UiBox::new()
        .build()
        .with_key("conversation-transcript")
        .aria_label("Transcript");
    let view = UiBox::new()
        .child(announced(transcript, &announcement))
        .child(announcer(&announcement))
        .build();
    let mut app = aqua::UiApp::new(SharedFonts::new(), SharedRuns::new());
    app.mount(|_| view).unwrap();
    app.set_viewport(400.0, 300.0, 1.0);
    app.build_frame().unwrap();
    app.tree().emit_semantics()
}

#[test]
fn the_tree_carries_a_polite_announcement_and_a_busy_transcript_while_streaming() {
    let nodes = semantics(&replying("First point.\n\nSecond"));
    let live: Vec<_> = nodes
        .iter()
        .filter(|node| node.live == Some(LiveRegion::Polite))
        .map(|node| node.label.as_str())
        .collect();
    assert_eq!(live, vec!["First point."]);
    let transcript = nodes
        .iter()
        .find(|node| node.label == "Transcript")
        .unwrap();
    assert!(transcript.busy);

    let nodes = semantics(&Turn::Finished);
    let live: Vec<_> = nodes
        .iter()
        .filter(|node| node.live == Some(LiveRegion::Polite))
        .map(|node| node.label.as_str())
        .collect();
    assert_eq!(live, vec!["Reply finished"]);
    assert!(
        nodes
            .iter()
            .find(|node| node.label == "Transcript")
            .is_some_and(|node| !node.busy)
    );
}

#[test]
fn nothing_is_read_out_while_no_turn_is_running() {
    let nodes = semantics(&Turn::Idle);
    assert!(
        nodes
            .iter()
            .all(|node| node.live.is_none() && !(node.role == Role::Text && node.label.is_empty())),
        "{nodes:#?}"
    );
}
