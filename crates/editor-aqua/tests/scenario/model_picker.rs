use artisan_api::EngineId;
use test_support::scenario::{Key, Modifiers, NamedKey, Opening, Role, Scenario};

#[test]
fn the_model_picker_says_why_no_engine_can_run_and_offers_the_fix() {
    let mut editor = Scenario::replaying(EngineId::Claude, "reply")
        .thread("thread-a", "Greeting")
        .open(Opening::Thread("thread-a"));
    editor.click(Role::ComboBox, "No model");
    insta::assert_snapshot!(
        "claude",
        editor.settle_part("the picker to open", Role::Menu, None, |view| {
            view.find(Role::Button, "Sign in to Claude Code").is_some()
        })
    );
    editor.click(Role::Button, "Codex");
    insta::assert_snapshot!(
        "codex",
        editor.settle_part("the Codex tab", Role::Menu, None, |view| {
            view.focused().is_some_and(|focused| {
                focused.role == Role::Button && focused.label.as_deref() == Some("Codex")
            })
        })
    );
    editor.press(Key::Named(NamedKey::Escape), Modifiers::NONE);
    editor.settle("the picker to close", |view| {
        view.all(Role::Menu).is_empty() && view.find(Role::ComboBox, "No model").is_some()
    });
}

#[test]
fn a_forge_without_threads_opens_on_an_empty_new_thread_page() {
    let mut editor = Scenario::replaying(EngineId::Claude, "reply").open(Opening::NewThread);
    insta::assert_snapshot!(editor.settle("the new thread page", |view| {
        view.shows("No threads yet.") && view.find(Role::ComboBox, "Project").is_some()
    }));
}
