use artisan_api::EngineId;
use test_support::scenario::{Key, Modifiers, NamedKey, Opening, Role, Scenario};

#[test]
fn settings_move_between_pages_and_close_back_to_the_thread() {
    let mut editor = Scenario::replaying(EngineId::Claude, "reply")
        .thread("thread-a", "Greeting")
        .open(Opening::Thread("thread-a"));
    editor.shows("No messages yet.");
    editor.press(Key::Char(','), Modifiers::CONTROL);
    drop(editor.settle_part(
        "settings to open on General",
        Role::Dialog,
        Some("Settings"),
        |view| view.shows("Interface & Models"),
    ));
    editor.click(Role::TextInput, "Search settings…");
    editor.type_text("curated models");
    drop(editor.settle_part(
        "settings search to find the matching control",
        Role::Dialog,
        Some("Settings"),
        |view| {
            view.find(Role::Switch, "Curated models").is_some() && !view.shows("Higher contrast")
        },
    ));
    editor.click(Role::Button, "General");
    drop(editor.settle_part(
        "the General button to clear search without changing pages",
        Role::Dialog,
        Some("Settings"),
        |view| view.find(Role::Switch, "Higher contrast").is_some(),
    ));
    editor.click(Role::Button, "Computers");
    drop(editor.settle_part(
        "the Computers page",
        Role::Dialog,
        Some("Settings"),
        |view| view.shows("Enroll Computer"),
    ));
    editor.click(Role::Button, "Claude Code");
    drop(editor.settle_part(
        "the Claude Code page",
        Role::Dialog,
        Some("Settings"),
        |view| view.shows("Accounts"),
    ));
    editor.press(Key::Named(NamedKey::Escape), Modifiers::NONE);
    insta::assert_snapshot!(
        "closed",
        editor.settle("settings to close", |view| {
            view.find(Role::Dialog, "Settings").is_none()
        })
    );
}
