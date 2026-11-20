use artisan_api::EngineId;
use test_support::scenario::{Key, Modifiers, NamedKey, Opening, Role, Scenario};

const COMPOSER: &str = "Message";

fn turn_snapshots() -> insta::Settings {
    let mut settings = insta::Settings::clone_current();
    if test_support::scenario::auto_runs_unsandboxed() {
        settings.set_snapshot_suffix("unsandboxed");
    }
    settings
}

#[track_caller]
fn assert_focus_returns_to_the_composer(editor: &test_support::scenario::Editor, after: &str) {
    let focused = editor.view(|view| view.focused());
    assert!(
        focused
            .as_ref()
            .is_some_and(|focused| focused.role == Role::TextInput && !focused.disabled),
        "focus returns to the composer after {after}, not {focused:?}"
    );
}

#[test]
fn a_revisited_thread_reads_out_its_history_and_offers_the_composer() {
    let mut editor = Scenario::replaying(EngineId::Claude, "reply")
        .thread("thread-a", "Greeting")
        .exchange(
            "thread-a",
            "What does the parser do?",
            "It turns text into tokens.",
        )
        .open(Opening::Thread("thread-a"));
    insta::assert_snapshot!(editor.shows("It turns text into tokens."));
}

#[test]
fn a_message_sent_from_the_composer_streams_its_reply_into_the_thread() {
    let mut editor = Scenario::replaying(EngineId::Claude, "reply")
        .thread("thread-a", "Greeting")
        .open(Opening::Thread("thread-a"));
    editor.click(Role::TextInput, COMPOSER);
    editor.type_text("Say hello");
    insta::assert_snapshot!(
        "drafted",
        editor.settle("the draft to enable Send", |view| {
            view.find(Role::Button, "Send message")
                .is_some_and(|send| !send.disabled)
        })
    );
    editor.click(Role::Button, "Send message");
    let _host = turn_snapshots().bind_to_scope();
    editor.settle("the turn to start", |view| view.shows("Say hello"));
    assert_focus_returns_to_the_composer(&editor, "Send");
    insta::assert_snapshot!(
        "replied",
        editor.settle_on(
            "the reply to finish",
            |view| {
                view.shows("Hello from the replay.") && view.find(Role::Button, "Stop").is_none()
            },
            |view| view.part_showing(Role::List, "Hello from the replay.")
        )
    );
    assert!(
        editor.view(|view| view
            .all(Role::TextInput)
            .iter()
            .all(|input| input.value.as_deref().is_none_or(str::is_empty))),
        "the sent draft leaves the composer"
    );
    assert_eq!(editor.engine().launches(), 1, "one message runs one turn");
}

#[test]
fn a_draft_set_by_assistive_tech_can_be_sent() {
    let mut editor = Scenario::replaying(EngineId::Claude, "reply")
        .thread("thread-a", "Greeting")
        .open(Opening::Thread("thread-a"));
    editor.click(Role::TextInput, COMPOSER);
    editor.set_value(Role::TextInput, COMPOSER, "Say hello");
    editor.settle("the set draft to enable Send", |view| {
        view.find(Role::Button, "Send message")
            .is_some_and(|send| !send.disabled)
    });
    editor.activate(Role::Button, "Send message");
    editor.settle("the reply to finish", |view| {
        view.shows("Hello from the replay.") && view.find(Role::Button, "Stop").is_none()
    });
    assert_eq!(editor.engine().launches(), 1, "the set draft runs one turn");
}

#[test]
fn stop_ends_the_turn_and_continue_picks_the_work_back_up() {
    let mut editor = Scenario::replaying(EngineId::Claude, "stop-before-acknowledgement")
        .thread("thread-a", "Parser")
        .open(Opening::Thread("thread-a"));
    let _host = turn_snapshots().bind_to_scope();
    editor.click(Role::TextInput, COMPOSER);
    editor.type_text("Refactor the parser");
    editor.press(Key::Named(NamedKey::Enter), Modifiers::NONE);
    insta::assert_snapshot!(
        "working",
        editor.settle("the turn to start", |view| {
            view.find(Role::Button, "Stop").is_some() && view.shows("Refactor the parser")
        })
    );
    editor.click(Role::Button, "Stop");
    insta::assert_snapshot!(
        "stopped",
        editor.settle("the turn to stop", |view| {
            view.find(Role::Button, "Continue where it stopped")
                .is_some()
                && view.shows("Stopped after")
        })
    );
    assert_focus_returns_to_the_composer(&editor, "Stop");
    editor.click(Role::Button, "Continue where it stopped");
    insta::assert_snapshot!(
        "continued",
        editor.settle("the continued turn to finish", |view| {
            view.shows("Picked the parser refactor back up.")
                && view.find(Role::Button, "Stop").is_none()
        })
    );
    assert_focus_returns_to_the_composer(&editor, "Continue");
    assert_eq!(
        editor.engine().launches(),
        2,
        "the stopped turn and its continuation each run the engine"
    );
}

#[test]
fn an_approval_answered_in_the_thread_lets_the_turn_finish() {
    let mut editor = Scenario::replaying(EngineId::OpenCode, "permission")
        .thread("thread-a", "Echo")
        .open(Opening::Thread("thread-a"));
    let _host = turn_snapshots().bind_to_scope();
    editor.click(Role::TextInput, COMPOSER);
    editor.type_text("Echo hello");
    editor.press(Key::Named(NamedKey::Enter), Modifiers::NONE);
    insta::assert_snapshot!(
        "asking",
        editor.settle("the command to wait for approval", |view| {
            view.find(Role::Button, "Approve").is_some()
                && view.shows("Waiting for you")
                && view.shows("OpenCode is waiting for you")
        })
    );
    editor.click(Role::Button, "Always allow");
    insta::assert_snapshot!(
        "answered",
        editor.settle("the approved turn to finish", |view| {
            view.shows("It printed hello.")
                && view.find(Role::Button, "Approve").is_none()
                && view.find(Role::Button, "Stop").is_none()
        })
    );
}

#[test]
fn a_screen_reader_scrolls_back_through_the_transcript_and_it_stays_there() {
    use test_support::scenario::{Action, ActionData, Found};
    let mut scenario = Scenario::replaying(EngineId::Claude, "reply").thread("thread-a", "Long");
    for turn in 0..24 {
        scenario = scenario.exchange(
            "thread-a",
            &format!("Question {turn}"),
            &format!("Answer {turn}, long enough to fill a line of the transcript on its own."),
        );
    }
    let mut editor = scenario.open(Opening::Thread("thread-a"));
    editor.shows("Answer 23");
    let transcript =
        |view: &test_support::scenario::View<'_>| -> Option<Found> { view.scrollable(Role::List) };
    let at_end = editor
        .view(transcript)
        .expect("the transcript is a scrollable list");
    let end = at_end
        .scroll
        .expect("a scrollable list has a scroll position")
        .y;
    editor.assist(&at_end, Action::ScrollUp, None);
    editor.settle(
        "the transcript to stay where the screen reader scrolled it",
        |view| {
            transcript(view)
                .and_then(|list| list.scroll)
                .is_some_and(|scroll| scroll.y < end - 1.0)
        },
    );
    let paged = editor
        .view(transcript)
        .expect("the transcript is still a list");
    editor.assist(
        &paged,
        Action::SetScrollOffset,
        Some(ActionData::SetScrollOffset(test_support::scenario::Point {
            x: 0.0,
            y: 0.0,
        })),
    );
    editor.settle("the transcript to stay at its start", |view| {
        transcript(view)
            .and_then(|list| list.scroll)
            .is_some_and(|scroll| scroll.y < 1.0)
    });
}
