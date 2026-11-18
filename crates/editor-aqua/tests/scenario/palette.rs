use artisan_api::EngineId;
use test_support::scenario::{Key, Modifiers, NamedKey, Opening, Role, Scenario};

#[test]
fn the_command_palette_narrows_to_a_typed_thread_and_opens_it() {
    let mut editor = Scenario::replaying(EngineId::Claude, "reply")
        .thread("thread-a", "Greeting")
        .thread("thread-b", "Parser rewrite")
        .exchange("thread-b", "Where is the lexer?", "In the parser module.")
        .open(Opening::Thread("thread-a"));
    editor.press(Key::Char('k'), Modifiers::CONTROL);
    insta::assert_snapshot!(
        "opened",
        editor.settle_part(
            "the palette to open with its search focused",
            Role::Dialog,
            Some("Search threads and commands"),
            |view| {
                view.focused()
                    .is_some_and(|focused| focused.role == Role::TextInput)
            }
        )
    );
    editor.type_text("Parser");
    insta::assert_snapshot!(
        "narrowed",
        editor.settle_part(
            "the palette to narrow to the typed thread",
            Role::Dialog,
            Some("Search threads and commands"),
            |view| {
                view.part(Role::Dialog, Some("Search threads and commands"))
                    .is_some_and(|palette| {
                        palette.contains("Parser rewrite")
                            && !palette.contains("Greeting")
                            && !palette.contains("Searching threads")
                    })
            }
        )
    );
    editor.press(Key::Named(NamedKey::Enter), Modifiers::NONE);
    insta::assert_snapshot!(
        "opened_thread",
        editor.settle("the chosen thread to open", |view| {
            view.find(Role::Dialog, "Search threads and commands")
                .is_none()
                && view.shows("In the parser module.")
        })
    );
}
