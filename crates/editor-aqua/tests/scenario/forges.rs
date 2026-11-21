use artisan_api::EngineId;
use test_support::scenario::{Key, Modifiers, NamedKey, Opening, Role, Scenario};

#[test]
fn switching_forge_from_the_palette_shows_that_forges_threads() {
    let mut editor = Scenario::replaying(EngineId::Claude, "reply")
        .thread("thread-a", "Greeting")
        .forge("Studio")
        .thread("thread-b", "Studio work")
        .open(Opening::Thread("thread-a"));
    editor.shows("No messages yet.");
    editor.press(Key::Char('k'), Modifiers::CONTROL);
    editor.type_text("Studio");
    insta::assert_snapshot!(
        "offered",
        editor.settle_part(
            "the palette to offer the other Forge",
            Role::Dialog,
            Some("Search threads and commands"),
            |view| {
                view.find(Role::Button, "Switch to Studio").is_some()
                    && view.find(Role::Button, "Studio work").is_some()
                    && !view.shows("Searching threads")
            }
        )
    );
    editor.press(Key::Named(NamedKey::Enter), Modifiers::NONE);
    insta::assert_snapshot!(
        "switched",
        editor.settle_part(
            "the other Forge's threads to show",
            Role::List,
            None,
            |view| {
                view.find(Role::Dialog, "Search threads and commands")
                    .is_none()
                    && view.find(Role::Button, "Studio work").is_some()
                    && view.find(Role::Button, "Greeting").is_none()
            }
        )
    );
}
