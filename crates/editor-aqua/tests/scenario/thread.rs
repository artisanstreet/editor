use artisan_api::EngineId;
use test_support::scenario::{Key, Modifiers, NamedKey, Opening, Role, Scenario};

const COMPOSER: &str = "Message";

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
    editor.click(Role::TextInput, COMPOSER);
    editor.type_text("Echo hello");
    editor.press(Key::Named(NamedKey::Enter), Modifiers::NONE);
    insta::assert_snapshot!(
        "asking",
        editor.settle("the command to wait for approval", |view| {
            view.find(Role::Button, "Approve").is_some()
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
