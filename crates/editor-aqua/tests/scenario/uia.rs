use artisan_api::EngineId;
use test_support::scenario::{Opening, Role, Scenario};

fn line<'a>(tree: &'a str, name: &str) -> &'a str {
    let quoted = format!("{name:?}");
    tree.lines()
        .find(|line| line.contains(&quoted))
        .unwrap_or_else(|| panic!("UI Automation has no element named {quoted}:\n{tree}"))
}

#[test]
fn a_screen_reader_reads_the_draft_sends_it_and_hears_the_streamed_reply() {
    let mut editor = Scenario::replaying(EngineId::Claude, "reply")
        .thread("thread-a", "Greeting")
        .open(Opening::Thread("thread-a"));
    editor.click(Role::TextInput, "Ask about anything");
    editor.type_text("Say hello");
    let uia = editor.read_through_uia();
    editor.uia_settle(&uia, "the draft in the composer", |tree| {
        line(tree, "Ask about anything").contains("value=\"Say hello\"")
            && !line(tree, "Send message").contains("disabled")
    });
    editor.uia_invoke(&uia, "Send message");
    let replied = editor.uia_settle(&uia, "the streamed reply", |tree| {
        tree.contains("\"Hello from the replay.\"") && !tree.contains("\"Stop\"")
    });
    assert!(
        replied.contains("\"Say hello\"") && !replied.contains("value=\"Say hello\""),
        "the sent message moves from the composer into the thread:\n{replied}"
    );
    assert_eq!(editor.engine().launches(), 1, "one message runs one turn");
}

#[test]
fn a_screen_reader_opens_settings_from_the_account_menu_and_flips_a_switch() {
    let mut editor = Scenario::replaying(EngineId::Claude, "reply")
        .thread("thread-a", "Greeting")
        .open(Opening::Thread("thread-a"));
    let uia = editor.read_through_uia();
    let closed = editor.uia_settle(&uia, "the account button", |tree| tree.contains("\"Test\""));
    assert!(
        line(&closed, "Test").contains("collapsed"),
        "the account menu starts closed:\n{closed}"
    );
    editor.uia_expand(&uia, "Test");
    editor.uia_settle(&uia, "the account menu", |tree| {
        tree.contains("\"Sign out\"") && line(tree, "Test").contains("expanded")
    });
    editor.uia_invoke(&uia, "Settings");
    let before = editor.uia_settle(&uia, "the settings dialog", |tree| {
        tree.contains("\"switch show all models\"")
    });
    assert!(
        line(&before, "switch show all models").contains("unchecked"),
        "Show all models starts off:\n{before}"
    );
    editor.uia_toggle(&uia, "switch show all models");
    editor.uia_settle(&uia, "the switch to turn on", |tree| {
        line(tree, "switch show all models").contains("[checked")
    });
}
