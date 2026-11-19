use artisan_api::EngineId;
use test_support::scenario::{Key, Modifiers, NamedKey, Opening, Role, Scenario};

#[test]
fn a_thread_forge_does_not_have_says_so_without_an_empty_state_or_a_retry() {
    let mut editor = Scenario::replaying(EngineId::Claude, "reply")
        .thread("thread-a", "Greeting")
        .open(Opening::Thread("missing"));
    let settled = editor.settle("the missing thread", |view| {
        view.shows("This thread isn't on this Forge.")
    });
    assert!(
        !settled.contains("No messages yet."),
        "a missing thread isn't shown as an empty one:\n{settled}"
    );
    assert!(
        !settled.contains("Try again"),
        "nothing can bring a missing thread back:\n{settled}"
    );
    insta::assert_snapshot!(settled);
}

#[test]
fn a_message_sent_while_forge_is_down_stays_in_the_composer_with_a_retry() {
    let mut editor = Scenario::replaying(EngineId::Claude, "reply")
        .thread("thread-a", "Greeting")
        .open(Opening::Thread("thread-a"));
    editor.shows("No messages yet.");
    editor.stop_forge();
    editor.click(Role::TextInput, "Message");
    editor.type_text("Say hello");
    editor.press(Key::Named(NamedKey::Enter), Modifiers::NONE);
    let settled = editor.settle("the failure to show", |view| {
        view.shows("Couldn't") && view.find(Role::Button, "Try again").is_some()
    });
    insta::assert_snapshot!(with_os_cause_elided(&settled));
    assert_eq!(editor.engine().launches(), 0, "nothing reached the engine");
}

fn with_os_cause_elided(view: &str) -> String {
    const STEP: &str = ": reach Forge: ";
    view.lines()
        .map(|line| match line.split_once(STEP) {
            Some((before, _)) => format!("{before}{STEP}[connection refused]\""),
            None => line.to_owned(),
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn a_thread_tells_assistive_tech_when_forge_goes_offline() {
    let mut editor = Scenario::replaying(EngineId::Claude, "reply")
        .thread("thread-a", "Greeting")
        .open(Opening::Thread("thread-a"));
    editor.shows("No messages yet.");
    assert!(
        editor.view(|view| !view.shows("Lost the connection to Forge")),
        "a live Forge is not reported as lost"
    );
    editor.stop_forge();
    insta::assert_snapshot!(editor.settle("the offline status", |view| {
        view.shows("Lost the connection to Forge")
    }));
}
