use artisan_api::EngineId;
use test_support::scenario::{Opening, Role, Scenario};

fn named<'a>(tree: &'a str, name: &str) -> Option<&'a str> {
    let quoted = format!("{name:?}");
    tree.lines().find(|line| line.contains(&quoted))
}

fn line<'a>(tree: &'a str, name: &str) -> &'a str {
    named(tree, name)
        .unwrap_or_else(|| panic!("UI Automation has no element named {name:?}:\n{tree}"))
}

fn switch<'a>(tree: &'a str, name: &str) -> Option<&'a str> {
    let quoted = format!("{name:?}");
    tree.lines()
        .find(|line| line.contains(&quoted) && line.contains("checked"))
}

fn toggle<'a>(tree: &'a str, name: &str) -> &'a str {
    switch(tree, name)
        .unwrap_or_else(|| panic!("UI Automation has no toggle named {name:?}:\n{tree}"))
}

#[test]
fn a_screen_reader_reads_the_draft_sends_it_and_hears_the_streamed_reply() {
    let mut editor = Scenario::replaying(EngineId::Claude, "reply")
        .thread("thread-a", "Greeting")
        .open(Opening::Thread("thread-a"));
    editor.click(Role::TextInput, "Message");
    editor.type_text("Say hello");
    let uia = editor.read_through_uia();
    editor.uia_settle(&uia, "the draft in the composer", |tree| {
        named(tree, "Message").is_some_and(|line| line.contains("value=\"Say hello\""))
            && named(tree, "Send message").is_some_and(|line| !line.contains("disabled"))
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
        tree.contains("\"Sign out\"")
            && named(tree, "Test").is_some_and(|line| line.contains("expanded"))
    });
    editor.uia_invoke(&uia, "Settings");
    let before = editor.uia_settle(&uia, "the settings dialog", |tree| {
        switch(tree, "Show all models").is_some()
    });
    assert!(
        toggle(&before, "Show all models").contains("unchecked"),
        "Show all models starts off:\n{before}"
    );
    editor.uia_toggle(&uia, "Show all models");
    editor.uia_settle(&uia, "the switch to turn on", |tree| {
        switch(tree, "Show all models").is_some_and(|line| line.contains("[checked"))
    });
}
