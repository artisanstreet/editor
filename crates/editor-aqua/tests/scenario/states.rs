use artisan_api::EngineId;
use test_support::scenario::{Key, Modifiers, NamedKey, Opening, Role, Scenario};

#[test]
fn a_thread_forge_does_not_have_says_it_could_not_load_and_offers_a_retry() {
    let mut editor = Scenario::replaying(EngineId::Claude, "reply")
        .thread("thread-a", "Greeting")
        .open(Opening::Thread("missing"));
    insta::assert_snapshot!(editor.settle("the load failure", |view| {
        view.shows("Couldn't load this thread") && view.find(Role::Button, "Try again").is_some()
    }));
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
    insta::assert_snapshot!(editor.settle("the failure to show", |view| {
        view.shows("Couldn't") && view.find(Role::Button, "Try again").is_some()
    }));
    assert_eq!(editor.engine().launches(), 0, "nothing reached the engine");
}
