use artisan_api::EngineId;
use test_support::scenario::{Key, Modifiers, NamedKey, Opening, Role, Scenario};

#[test]
fn settings_move_between_pages_and_close_back_to_the_thread() {
    let mut editor = Scenario::replaying(EngineId::Claude, "reply")
        .thread("thread-a", "Greeting")
        .open(Opening::Thread("thread-a"));
    editor.shows("No messages yet.");
    editor.press(Key::Char(','), Modifiers::CONTROL);
    insta::assert_snapshot!(
        "general",
        editor.settle_part(
            "settings to open on General",
            Role::Dialog,
            Some("Settings"),
            |view| view.shows("How this Editor behaves on this device.")
        )
    );
    editor.click(Role::Button, "Forges");
    insta::assert_snapshot!(
        "forges",
        editor.settle_part(
            "the Forges page",
            Role::Dialog,
            Some("Settings"),
            |view| view.shows("The account's Forges this Editor has open.")
                && view.shows("Connected")
        )
    );
    editor.click(Role::Button, "Claude Code");
    insta::assert_snapshot!(
        "engine",
        editor.settle_part(
            "the Claude Code page",
            Role::Dialog,
            Some("Settings"),
            |view| view.shows("claude.ai session")
        )
    );
    editor.press(Key::Named(NamedKey::Escape), Modifiers::NONE);
    insta::assert_snapshot!(
        "closed",
        editor.settle("settings to close", |view| {
            view.find(Role::Dialog, "Settings").is_none()
        })
    );
}
