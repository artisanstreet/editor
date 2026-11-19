use super::*;
use aqua_core::SemanticNode;
use aqua_text::{SharedFonts, SharedRuns};
use artisan_api::{Item, ItemsPage, RunId, Status};
use test_support::editor::core::StubbedHost;
use test_support::editor::forge::{HeldRequest, empty_state};
use test_support::wait::{LIVENESS, wait_until};

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
        announcement(&Turn::Finished {
            text: String::new()
        }),
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

    let nodes = semantics(&Turn::Finished {
        text: "First point.\n\n".to_owned(),
    });
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

#[test]
fn the_last_paragraph_is_read_out_when_the_reply_finishes() {
    let reply = "The bug is in the parser.\n\nThe fix keeps the last line.";
    assert_eq!(
        announcement(&replying(reply)).text,
        "The bug is in the parser."
    );
    assert_eq!(
        announcement(&Turn::Finished {
            text: reply.to_owned()
        })
        .text,
        "The fix keeps the last line.\n\nReply finished",
        "the paragraph streaming never completed is read with the end of the reply"
    );
    assert_eq!(
        announcement(&Turn::Finished {
            text: "A single paragraph".to_owned()
        })
        .text,
        "A single paragraph\n\nReply finished"
    );
    assert_eq!(
        announcement(&Turn::Finished {
            text: "Already read.\n\n".to_owned()
        })
        .text,
        "Reply finished",
        "a paragraph already read while streaming is not read again"
    );
}

const LAST_PARAGRAPH: &str = "So the fix keeps the last line of every file.";

fn long_stored_reply() -> String {
    let filler = "Le café d\u{2019}à côté sert un thé glacé à l\u{2019}orée du jour.\n\n";
    format!("{}{LAST_PARAGRAPH}", filler.repeat(600))
}

fn finished_reply_page(thread: &ThreadId, source: ContentRef) -> ItemsPage {
    let run = RunId("long-run".into());
    let now = artisan_api::now();
    ItemsPage {
        items: vec![Item {
            id: ItemId("long-reply".into()),
            thread_id: thread.clone(),
            position: 1,
            seq: 1,
            run_id: Some(run.clone()),
            parent_id: None,
            status: Status::Done,
            created_at: now,
            updated_at: now,
            kind: ItemKind::AssistantMessage {
                text: TextContent::Stored(source),
                phase: artisan_api::Phase::Final,
            },
        }],
        runs: vec![Run {
            config: None,
            id: run,
            thread_id: thread.clone(),
            engine: artisan_api::EngineId::Claude,
            model: None,
            input_cutoff: 0,
            state: RunState::Finished {
                outcome: RunOutcome::Done,
                error: None,
                ended_at: now,
            },
            usage: artisan_api::Usage::default(),
            session_id: None,
            seq: 2,
            started_at: now,
            continuation: None,
        }],
        more: false,
        work: Vec::new(),
    }
}

fn query_number(request: &HeldRequest, name: &str) -> usize {
    request
        .query()
        .and_then(|query| {
            query
                .split('&')
                .find_map(|pair| pair.strip_prefix(name)?.strip_prefix('='))
        })
        .unwrap_or_else(|| panic!("the content request names its {name}: {request:?}"))
        .parse()
        .unwrap_or_else(|error| panic!("the content request's {name} is a number: {error}"))
}

fn answer_like_forge(request: HeldRequest, text: &str) {
    let offset = query_number(&request, "offset");
    let limit = query_number(&request, "limit");
    let start = text.floor_char_boundary(offset);
    let end = text.floor_char_boundary((start + limit).min(text.len()));
    request.respond(ContentRange {
        id: ContentRef::from_text(text).id,
        start: start as u64,
        end: end as u64,
        total: text.len() as u64,
        text: text
            .get(start..end)
            .expect("both ends are floored to character boundaries")
            .to_owned(),
    });
}

struct FinishedReply {
    stub: StubbedHost,
    conversation: Conversation,
    reply: String,
    source: ContentRef,
}

impl FinishedReply {
    fn open(thread: &str) -> Self {
        let reply = long_stored_reply();
        let source = ContentRef::from_text(&reply);
        assert!(
            source.bytes > u64::from(REPLY_TAIL_BYTES),
            "the reply is too long to keep inline or fetch whole"
        );
        let mut stub = StubbedHost::start(empty_state(), axum::Router::new());
        let thread = ThreadId::from(thread);
        let mut conversation = Conversation::new(&stub.core);
        conversation.open_thread(&mut stub.core, stub.host.clone(), thread.clone());
        stub.take("GET", &format!("/threads/{thread}/transcript"))
            .respond(finished_reply_page(&thread, source.clone()));
        wait_until("the finished reply to land", LIVENESS, || {
            conversation.drain(&mut stub.core);
            conversation.reply_tail.is_some()
        });
        Self {
            stub,
            conversation,
            reply,
            source,
        }
    }

    fn content_request(&mut self) -> HeldRequest {
        self.stub
            .take("GET", &format!("/content/{}", self.source.id))
    }

    fn announcement_once_settled(&mut self) -> Announcement {
        wait_until("the end of the reply to settle", LIVENESS, || {
            self.conversation.drain(&mut self.stub.core);
            !current(&self.conversation).text.is_empty()
        });
        current(&self.conversation)
    }
}

#[test]
fn the_last_paragraph_of_a_stored_reply_is_read_out_once_when_it_finishes() {
    let mut finished = FinishedReply::open("long-thread");
    assert_eq!(
        current(&finished.conversation),
        Announcement {
            text: String::new(),
            busy: true,
        },
        "nothing is read out while the end of the reply is on its way"
    );
    let request = finished.content_request();
    answer_like_forge(request, &finished.reply);
    assert_eq!(
        finished.announcement_once_settled(),
        Announcement {
            text: format!("{LAST_PARAGRAPH}\n\nReply finished"),
            busy: false,
        }
    );
}

#[test]
fn a_stored_reply_whose_end_cannot_be_read_still_announces_that_it_finished() {
    let mut finished = FinishedReply::open("unreadable-thread");
    finished.content_request().fail(
        axum::http::StatusCode::NOT_FOUND,
        "content_not_found",
        "The content is gone.",
    );
    assert_eq!(
        finished.announcement_once_settled(),
        Announcement {
            text: "Reply finished".to_owned(),
            busy: false,
        }
    );
}
