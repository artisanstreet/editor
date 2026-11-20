use editor_aqua::core::{AccountPhase, CoreEvent, EntryGate, Screen};
use test_support::editor::account::email_of;
use test_support::editor::shell::{Motion, ThreadShell};
use test_support::wait::{LIVENESS, wait_until};

fn signed_in(shell: &ThreadShell) -> bool {
    matches!(shell.shell.core.entry_gate(), EntryGate::Ready { .. })
}

#[test]
fn signing_out_and_back_in_brings_back_a_draft_typed_while_forge_was_away() {
    let mut opened = ThreadShell::open("offline-host", "drafted-thread", Motion::Reduced);
    let account = opened
        .shell
        .core
        .account()
        .profile()
        .expect("the shell opens signed in")
        .account_id
        .clone();
    {
        let workspace = opened.workspace();
        let mut composer = workspace
            .composer()
            .as_ref()
            .unwrap()
            .controller
            .borrow_mut();
        composer
            .editor
            .borrow_mut()
            .set_text("typed while Forge was away");
        composer.edited();
    }

    opened.shell.core.account_mut().sign_out();
    wait_until("the account to sign out", LIVENESS, || {
        opened.shell.core.drain();
        !signed_in(&opened)
    });
    let ThreadShell {
        shell, resources, ..
    } = &mut opened;
    shell.entry.handle_event(
        &mut shell.core,
        &CoreEvent::AccessChanged,
        false,
        &mut resources.ui,
    );
    assert!(shell.entry.workspace().is_none());

    shell
        .core
        .account_mut()
        .request_link(&email_of(&account), "Test".into());
    wait_until("the magic link to be approved", LIVENESS, || {
        shell.core.drain();
        matches!(
            shell.core.account().phase(),
            AccountPhase::EmailApproved { .. }
        )
    });
    shell.core.account_mut().finish_sign_in(None);
    let host = opened.host.id.clone();
    wait_until("the account and its Forge to come back", LIVENESS, || {
        opened.shell.core.drain();
        signed_in(&opened) && opened.shell.core.host(&host).is_some()
    });
    let ThreadShell {
        shell,
        resources,
        thread,
        ..
    } = &mut opened;
    shell.core.navigate(Screen::Thread {
        host,
        thread: thread.clone(),
    });
    shell.entry.handle_event(
        &mut shell.core,
        &CoreEvent::AccessChanged,
        false,
        &mut resources.ui,
    );
    let workspace = shell
        .entry
        .workspace_mut()
        .expect("signing back in shows a workspace");
    workspace.sync_composer(&shell.core, &mut resources.ui);
    let mut composer = workspace
        .composer()
        .as_ref()
        .unwrap()
        .controller
        .borrow_mut();
    composer.sync(&shell.core);
    assert_eq!(
        composer.editor.borrow().text(),
        "typed while Forge was away"
    );
}
