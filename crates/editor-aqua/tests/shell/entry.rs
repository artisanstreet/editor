use editor_aqua::core::{AccountPhase, CoreEvent, EntryGate, Screen};
use test_support::editor::account::email_of;
use test_support::editor::shell::{Motion, ThreadShell};
use test_support::wait::{LIVENESS, wait_until};

fn signed_in(shell: &ThreadShell) -> bool {
    matches!(shell.shell.core.entry_gate(), EntryGate::Ready { .. })
}

fn type_into_composer(opened: &mut ThreadShell, text: &str) {
    let workspace = opened.workspace();
    let mut composer = workspace
        .composer()
        .as_ref()
        .unwrap()
        .controller
        .borrow_mut();
    crate::recovered(composer.session.as_ref().unwrap());
    composer.editor.borrow_mut().set_text(text);
    composer.edited();
}

fn sign_out_and_back_in_on_the_thread(opened: &mut ThreadShell) -> String {
    let account = opened
        .shell
        .core
        .account()
        .profile()
        .expect("the shell opens signed in")
        .account_id
        .clone();
    opened.shell.core.account_mut().sign_out();
    wait_until("the account to sign out", LIVENESS, || {
        opened.shell.core.drain();
        !signed_in(opened)
    });
    let ThreadShell {
        shell, resources, ..
    } = &mut *opened;
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
        signed_in(opened) && opened.shell.core.host(&host).is_some()
    });
    let ThreadShell {
        shell,
        resources,
        thread,
        ..
    } = &mut *opened;
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
    crate::recovered(composer.session.as_ref().unwrap());
    composer.sync(&shell.core);
    composer.editor.borrow().text().to_owned()
}

#[test]
fn signing_out_and_back_in_brings_back_a_draft_typed_while_forge_was_away() {
    let mut opened = ThreadShell::open("offline-host", "drafted-thread", Motion::Reduced);
    type_into_composer(&mut opened, "typed while Forge was away");

    assert_eq!(
        sign_out_and_back_in_on_the_thread(&mut opened),
        "typed while Forge was away"
    );
}

#[test]
fn a_draft_this_device_could_not_keep_at_sign_out_comes_back_and_is_kept_once_at_shutdown() {
    let mut opened = ThreadShell::open("offline-host", "held-thread", Motion::Reduced);
    let drafts = opened.shell.core.files().dir.join("drafts");
    std::fs::write(&drafts, b"not a folder").unwrap();
    type_into_composer(&mut opened, "only copy of this text");

    assert_eq!(
        sign_out_and_back_in_on_the_thread(&mut opened),
        "only copy of this text",
        "the draft held in memory at sign-out is back"
    );

    std::fs::remove_file(&drafts).unwrap();
    let ThreadShell {
        shell, directory, ..
    } = opened;
    drop(shell);
    let outcome = editor_aqua::core::runtime::runtime().block_on(
        editor_aqua::composer::persistence::drain_pending(
            editor_aqua::composer::persistence::ShutdownLimits {
                forge: std::time::Duration::from_secs(5),
                device: std::time::Duration::from_secs(5),
            },
        ),
    );
    assert!(
        matches!(
            outcome,
            editor_aqua::composer::persistence::ShutdownOutcome::Incomplete {
                kept_locally: 1..,
                ..
            }
        ),
        "the draft Forge never saw is kept on this device at shutdown: {outcome:?}"
    );
    let listed = |folder: &std::path::Path| -> Vec<_> {
        std::fs::read_dir(folder)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .collect()
    };
    let scopes = listed(&directory.path().join("drafts"));
    let [scope] = &scopes[..] else {
        panic!("one thread's draft is kept: {scopes:?}");
    };
    let kept = listed(scope);
    let [record] = &kept[..] else {
        panic!("one draft is kept, with no copy beside it: {kept:?}");
    };
    let record = std::fs::read_to_string(record.join("record.json")).unwrap();
    assert!(record.contains("only copy of this text"), "{record}");
}
