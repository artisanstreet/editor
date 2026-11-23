use super::{TurnAction, markdown_link, turn_problem};
use artisan_api::{ApiError, ErrorCode, Failure, RunFailure, ThreadFailure, ThreadId};
use editor_core::client::{CallError, CallFailure, ResponseError};

#[test]
fn file_links_escape_markup_in_the_name_and_keep_the_path_one_link() {
    assert_eq!(
        markdown_link("*notes*_[draft].md", "/work/*notes*_[draft].md"),
        r"[\*notes\*\_\[draft\].md](</work/*notes*_[draft].md>)"
    );
    assert_eq!(
        markdown_link("a>b\nc.rs", "/work/a>b\nc.rs"),
        r"[a\>b c.rs](</work/a\>b%0Ac.rs>)"
    );
    assert_eq!(
        markdown_link("main.rs", r"C:\src\main.rs"),
        r"[main.rs](<C:\src\main.rs>)"
    );
    assert_eq!(
        markdown_link("x.rs", r"C:\[x]\x.rs"),
        r"[x.rs](<C:\\[x]\x.rs>)"
    );
}

fn refused(error: ApiError) -> CallError {
    CallError {
        method: reqwest::Method::POST,
        path: "/threads/t1/continue".to_owned(),
        failure: Box::new(CallFailure::Answer(ResponseError::Refused {
            status: reqwest::StatusCode::CONFLICT,
            error,
        })),
    }
}

fn typed(failure: Failure) -> CallError {
    let message = failure.to_string();
    refused(ApiError::new(failure, message))
}

#[test]
fn continuing_a_turn_that_already_finished_or_resumed_is_not_an_error() {
    let thread = ThreadId("t1".into());
    for failure in [
        Failure::Run(RunFailure::NothingToContinue {
            thread: thread.clone(),
        }),
        Failure::Run(RunFailure::AlreadyWorking {
            thread: thread.clone(),
        }),
    ] {
        assert_eq!(
            turn_problem(TurnAction::Continue, &typed(failure.clone())),
            None,
            "{failure:?}"
        );
    }
    assert_eq!(
        turn_problem(
            TurnAction::ClearGoal,
            &typed(Failure::Run(RunFailure::NoGoal { thread }))
        ),
        None
    );
}

#[test]
fn a_refused_turn_action_says_what_failed_and_keeps_forges_words_as_the_detail() {
    let missing = typed(Failure::Thread(ThreadFailure::NotFound {
        thread: ThreadId("t1".into()),
    }));
    let shown = turn_problem(TurnAction::Stop, &missing).expect("a failed stop is shown");
    assert_eq!(
        shown.headline(),
        "Couldn't stop the run. Forge didn't recognise it."
    );
    assert!(
        shown
            .detail()
            .is_some_and(|detail| detail.contains("Thread t1 doesn't exist on this Forge.")),
        "{shown:?}"
    );
    let busy = typed(Failure::Run(RunFailure::AlreadyWorking {
        thread: ThreadId("t1".into()),
    }));
    let shown = turn_problem(TurnAction::ClearGoal, &busy).expect("a refused clear is shown");
    assert_eq!(
        shown.headline(),
        "Couldn't clear the goal. Forge ran into a problem."
    );
    assert!(
        shown
            .detail()
            .is_some_and(|detail| detail.contains("Thread t1 is already working.")),
        "{shown:?}"
    );
    let legacy = refused(ApiError::legacy(
        ErrorCode::Conflict,
        "The thread has no stopped turn to continue.".to_owned(),
    ));
    let shown = turn_problem(TurnAction::Continue, &legacy).expect("a legacy refusal is shown");
    assert!(
        !shown.headline().contains("POST /threads"),
        "the call path never reaches the headline: {shown:?}"
    );
    assert_eq!(
        shown.detail(),
        Some(
            "POST /threads/t1/continue: Forge refused the call: Forge answered 409 Conflict: The thread has no stopped turn to continue."
        ),
        "a legacy refusal keeps the whole cause chain as the detail"
    );
}

fn recovered(composer: &mut super::Composer, core: &editor_core::Core) {
    let session = composer.session.as_ref().unwrap();
    let mut changed = session.subscribe();
    editor_core::runtime::runtime().block_on(async {
        while session.read(crate::composer::draft_session::DraftSession::recovering) {
            changed.changed().await.unwrap();
        }
    });
    composer.sync(core);
}

fn bypassing() -> super::Submission {
    super::Submission {
        message: artisan_api::SendMessage {
            id: artisan_api::ItemId("bypassing".into()),
            text: "another message".into(),
            images: Vec::new(),
            config: None,
            draft: None,
        },
        create: None,
        thread: ThreadId("thread".into()),
    }
}

#[test]
fn a_restored_send_waiting_on_a_choice_asks_for_the_choice_and_refuses_another_send() {
    use super::{Composer, ComposerTarget, SendRefusal};
    use crate::composer::draft_body::DraftBody;
    use crate::composer::draft_session::{DraftSession, KeptDraft};
    use crate::composer::persistence::DraftSessions;
    use crate::composer::spool::tests::Direct as _;
    use crate::composer::spool::{DraftSpool, Holder, LocalDisk, Slot};
    use artisan_api::tls::{Identity, fingerprint};
    use editor_core::{ForgeClient, HostConfig, HostId, HostKind};

    let directory = tempfile::tempdir().unwrap();
    let mut core = test_support::editor::core::test_core(directory.path());
    let forge = test_support::net::SilentListener::bind();
    let identity = Identity::generate("choosing-draft").unwrap();
    let config = HostConfig {
        id: HostId("choosing".into()),
        name: "Forge".into(),
        address: forge.address().to_string(),
        fingerprint: fingerprint(identity.certificate_der().unwrap().as_ref()),
        kind: HostKind::Local,
    };
    let client = ForgeClient::new(&config.address, &config.fingerprint, &identity).unwrap();
    test_support::editor::core::connect_client(&mut core, &config, client);
    let target = ComposerTarget::NewThread {
        host: config.id.clone(),
        project: None,
    };
    let spool = DraftSpool::new(directory.path(), LocalDisk::shared());
    let kept = |text: &str| KeptDraft {
        body: DraftBody {
            text: text.to_owned(),
            ..DraftBody::default()
        },
        base: None,
        post: None,
        lost: Vec::new(),
        set_aside: Vec::new(),
    };
    spool
        .keep(
            &config.id,
            &target.scope(),
            serde_json::from_value(serde_json::json!({
                "body": {"text": "sent from the record"},
                "post": {
                    "submission": {
                        "message": {"id": "restored", "text": "sent from the record", "images": [], "config": null},
                        "create": null,
                        "thread": "thread"
                    },
                    "sent": {"text": "sent from the record"}
                }
            }))
            .unwrap(),
            &[],
            Holder::Owner(Slot::Record),
        )
        .unwrap();
    spool
        .keep(
            &config.id,
            &target.scope(),
            kept("kept beside the record"),
            &[],
            Holder::Newcomer,
        )
        .unwrap();
    let mut sessions = DraftSessions::with_spool(spool);
    let mut composer = Composer::new(&core, &mut sessions, target);
    recovered(&mut composer, &core);
    composer.config = Some(artisan_api::EngineConfig::new(artisan_api::EngineId::Codex));
    composer.project = Some(artisan_api::ProjectId::from("project"));
    composer.editor.borrow_mut().set_text("another message");
    let host = core.host(&config.id).unwrap();
    let sending = composer
        .session
        .as_ref()
        .unwrap()
        .read(DraftSession::sending);
    assert!(sending);
    assert_eq!(
        composer.blocked(host),
        Some("Choose which draft to keep first."),
        "the restored message waits on the choice, not on Forge"
    );
    assert_eq!(composer.send(host), Err(SendRefusal::ChoosingDraft));
    assert_eq!(
        composer
            .session
            .as_ref()
            .unwrap()
            .submit(bypassing(), None, None)
            .map(drop),
        Err(SendRefusal::ChoosingDraft),
        "the draft itself refuses a send while a choice is open"
    );
}

#[test]
fn sending_while_a_draft_kept_beside_waits_for_a_choice_is_refused_and_keeps_the_message() {
    use super::{Composer, ComposerTarget, SendRefusal};
    use crate::composer::draft_body::DraftBody;
    use crate::composer::draft_session::{DraftSession, KeptDraft};
    use crate::composer::persistence::DraftSessions;
    use crate::composer::spool::tests::Direct as _;
    use crate::composer::spool::{DraftSpool, Holder, LocalDisk, Slot};
    use artisan_api::tls::{Identity, fingerprint};
    use editor_core::{ForgeClient, HostConfig, HostId, HostKind};

    let directory = tempfile::tempdir().unwrap();
    let mut core = test_support::editor::core::test_core(directory.path());
    let forge = test_support::net::SilentListener::bind();
    let identity = Identity::generate("choosing-draft").unwrap();
    let config = HostConfig {
        id: HostId("choosing".into()),
        name: "Forge".into(),
        address: forge.address().to_string(),
        fingerprint: fingerprint(identity.certificate_der().unwrap().as_ref()),
        kind: HostKind::Local,
    };
    let client = ForgeClient::new(&config.address, &config.fingerprint, &identity).unwrap();
    test_support::editor::core::connect_client(&mut core, &config, client);
    let target = ComposerTarget::NewThread {
        host: config.id.clone(),
        project: None,
    };
    let spool = DraftSpool::new(directory.path(), LocalDisk::shared());
    let kept = |text: &str| KeptDraft {
        body: DraftBody {
            text: text.to_owned(),
            ..DraftBody::default()
        },
        base: None,
        post: None,
        lost: Vec::new(),
        set_aside: Vec::new(),
    };
    spool
        .keep(
            &config.id,
            &target.scope(),
            kept("kept in the record"),
            &[],
            Holder::Owner(Slot::Record),
        )
        .unwrap();
    spool
        .keep(
            &config.id,
            &target.scope(),
            kept("kept beside the record"),
            &[],
            Holder::Newcomer,
        )
        .unwrap();
    let mut sessions = DraftSessions::with_spool(spool);
    let mut composer = Composer::new(&core, &mut sessions, target);
    recovered(&mut composer, &core);
    composer.config = Some(artisan_api::EngineConfig::new(artisan_api::EngineId::Codex));
    composer.project = Some(artisan_api::ProjectId::from("project"));
    composer
        .editor
        .borrow_mut()
        .set_text("sent before choosing");
    let host = core.host(&config.id).unwrap();

    assert_eq!(composer.send(host), Err(SendRefusal::ChoosingDraft));
    assert_eq!(
        composer.editor.borrow().text(),
        "sent before choosing",
        "the refused message stays in the composer"
    );
    let session = composer.session.as_ref().unwrap();
    assert!(!session.read(DraftSession::sending));

    session.keep_local();
    composer.send(host).expect("the message is sent");
    assert!(
        composer
            .session
            .as_ref()
            .unwrap()
            .read(DraftSession::sending),
        "once the user has chosen, the message goes out"
    );
}

#[test]
fn a_draft_kept_on_this_device_shows_once_read_and_the_composer_waits_for_it() {
    use super::{Composer, ComposerTarget, SendRefusal};
    use crate::composer::draft_body::DraftBody;
    use crate::composer::draft_session::{DraftSession, KeptDraft};
    use crate::composer::persistence::DraftSessions;
    use crate::composer::spool::tests::Direct as _;
    use crate::composer::spool::{DraftSpool, Holder, LocalDisk, Slot};
    use artisan_api::tls::{Identity, fingerprint};
    use editor_core::{ForgeClient, HostConfig, HostId, HostKind};

    let directory = tempfile::tempdir().unwrap();
    let mut core = test_support::editor::core::test_core(directory.path());
    let forge = test_support::net::SilentListener::bind();
    let identity = Identity::generate("recovering-draft").unwrap();
    let config = HostConfig {
        id: HostId("recovering".into()),
        name: "Forge".into(),
        address: forge.address().to_string(),
        fingerprint: fingerprint(identity.certificate_der().unwrap().as_ref()),
        kind: HostKind::Local,
    };
    let client = ForgeClient::new(&config.address, &config.fingerprint, &identity).unwrap();
    test_support::editor::core::connect_client(&mut core, &config, client);
    let target = ComposerTarget::NewThread {
        host: config.id.clone(),
        project: None,
    };
    let scope = target.scope();
    let spool = DraftSpool::new(directory.path(), LocalDisk::shared());
    spool
        .keep(
            &config.id,
            &scope,
            KeptDraft {
                body: DraftBody {
                    text: "kept on this device".to_owned(),
                    ..DraftBody::default()
                },
                base: None,
                post: None,
                lost: Vec::new(),
                set_aside: Vec::new(),
            },
            &[],
            Holder::Owner(Slot::Record),
        )
        .unwrap();
    let busy = spool.turn(&config.id, &scope).wait();
    let mut sessions = DraftSessions::with_spool(spool.clone());

    let mut composer = Composer::new(&core, &mut sessions, target);
    composer.sync(&core);
    assert!(
        composer.editor.borrow().is_disabled(),
        "the composer takes no edits while the kept draft is read"
    );
    composer.config = Some(artisan_api::EngineConfig::new(artisan_api::EngineId::Codex));
    composer.project = Some(artisan_api::ProjectId::from("project"));
    composer.editor.borrow_mut().set_text("typed too early");
    let host = core.host(&config.id).unwrap();
    assert_eq!(composer.send(host), Err(SendRefusal::DraftLoading));
    assert!(
        composer
            .session
            .as_ref()
            .unwrap()
            .read(DraftSession::recovering)
    );
    composer.editor.borrow_mut().set_text("");

    drop(busy);
    drop(spool.turn(&config.id, &scope).wait());
    composer.sync(&core);
    assert_eq!(composer.editor.borrow().text(), "kept on this device");
    assert!(!composer.editor.borrow().is_disabled());
}

struct ThreadComposer {
    forge: test_support::editor::core::StubbedHost,
    composer: super::Composer,
    _sessions: crate::composer::persistence::DraftSessions,
    _directory: tempfile::TempDir,
}

fn chat_thread() -> artisan_api::Thread {
    serde_json::from_value(serde_json::json!({
        "id": "chat",
        "project_id": "project",
        "title": "chat",
        "config": {"engine": "claude", "model": "opus"},
        "activity_seq": 3,
        "read_seq": 3,
        "seq": 3,
        "created_at": 0,
        "updated_at": 0,
        "archived": false,
    }))
    .unwrap()
}

fn thread_composer() -> ThreadComposer {
    use super::{Composer, ComposerTarget};
    use crate::composer::draft_session::DraftSession;
    use crate::composer::persistence::DraftSessions;
    use crate::composer::spool::{DraftSpool, LocalDisk};
    use artisan_api::{DraftSnapshot, Event};
    let mut forge = test_support::editor::core::StubbedHost::start(
        test_support::editor::forge::empty_state(),
        axum::Router::new(),
    );
    forge.forge.publish(Event::Thread {
        thread: chat_thread(),
    });
    forge.apply_published();
    let directory = tempfile::tempdir().unwrap();
    let mut sessions =
        DraftSessions::with_spool(DraftSpool::new(directory.path(), LocalDisk::shared()));
    let mut composer = Composer::new(
        &forge.core,
        &mut sessions,
        ComposerTarget::Thread {
            host: forge.host.clone(),
            thread: ThreadId("chat".into()),
        },
    );
    recovered(&mut composer, &forge.core);
    forge
        .take("GET", "/drafts/thread:chat")
        .respond(DraftSnapshot {
            revision: 0,
            body: serde_json::Value::Null,
        });
    let session = composer.session.as_ref().unwrap();
    let mut changed = session.subscribe();
    editor_core::runtime::runtime().block_on(async {
        tokio::time::timeout(std::time::Duration::from_secs(10), async {
            while !session.read(DraftSession::sendable) {
                changed.changed().await.unwrap();
            }
        })
        .await
        .expect("the draft loads");
    });
    composer.sync(&forge.core);
    ThreadComposer {
        forge,
        composer,
        _sessions: sessions,
        _directory: directory,
    }
}

fn choose_permissions(setup: &mut ThreadComposer, permissions: artisan_api::Permissions) {
    let mut config = chat_thread().config;
    config.permissions = permissions;
    setup.composer.config = Some(config);
    setup
        .composer
        .apply_permissions(setup.forge.host_mut(), permissions);
}

fn answer_patch(setup: &mut ThreadComposer, seq: u64, permissions: artisan_api::Permissions) {
    let mut reply = chat_thread();
    reply.seq = seq;
    reply.config.permissions = permissions;
    setup.forge.take("PATCH", "/threads/chat").respond(reply);
}

fn drain_draft_writes(forge: &mut test_support::editor::core::StubbedHost) {
    use artisan_api::{DraftSnapshot, DraftWriteResult};
    forge.core.drain_with(|_, _| {});
    if let Some(write) = forge.forge.try_take("PUT", "/drafts/thread:chat") {
        write.respond(DraftWriteResult {
            saved: true,
            current: DraftSnapshot {
                revision: 1,
                body: serde_json::Value::Null,
            },
        });
    }
}

#[test]
fn a_message_sent_while_a_permission_change_is_queued_goes_out_after_it_in_the_chosen_mode() {
    use artisan_api::{Permissions, SendMessage};
    let mut setup = thread_composer();
    choose_permissions(&mut setup, Permissions::Full);
    let full = setup.forge.take("PATCH", "/threads/chat");
    choose_permissions(&mut setup, Permissions::Auto);
    setup.composer.editor.borrow_mut().set_text("hello");
    setup
        .composer
        .send(setup.forge.host())
        .expect("the message is sent");
    for _ in 0..50 {
        drain_draft_writes(&mut setup.forge);
        assert!(
            !setup.forge.forge.asked("POST", "/threads/chat/messages"),
            "the message waits while the wider mode can still land on its run"
        );
        setup.forge.apply_published();
    }
    let mut reply = chat_thread();
    reply.seq = 4;
    reply.config.permissions = Permissions::Full;
    full.respond(reply);
    answer_patch(&mut setup, 5, Permissions::Auto);
    let forge = &mut setup.forge;
    let sent = test_support::wait::wait_for(
        "the message goes out",
        std::time::Duration::from_secs(10),
        || {
            drain_draft_writes(forge);
            forge
                .forge
                .try_take("POST", "/threads/chat/messages")
                .map(|post| post.json::<SendMessage>())
        },
    );
    assert_eq!(
        sent.config.map(|config| config.permissions),
        Some(Permissions::Auto),
        "the run starts in the chosen mode once no older mode can follow it"
    );
}

#[test]
fn a_refused_permission_change_keeps_a_newer_choice_of_the_same_mode() {
    use artisan_api::Permissions;
    use axum::http::StatusCode;
    let mut setup = thread_composer();
    choose_permissions(&mut setup, Permissions::Full);
    let first = setup.forge.take("PATCH", "/threads/chat");
    choose_permissions(&mut setup, Permissions::Auto);
    choose_permissions(&mut setup, Permissions::Full);
    first.fail(StatusCode::INTERNAL_SERVER_ERROR, "internal", "disk full");
    let ThreadComposer {
        forge, composer, ..
    } = &mut setup;
    test_support::wait::wait_until(
        "the refusal reaches the composer",
        std::time::Duration::from_secs(10),
        || {
            forge.core.drain_with(|_, _| {});
            composer.drain_turn_actions(&forge.core);
            composer.notice.is_some()
        },
    );
    assert_eq!(
        composer.config.as_ref().map(|config| config.permissions),
        Some(Permissions::Full),
        "the later Full choice still stands after the first Full is refused"
    );
}

#[test]
fn an_image_pasted_while_the_last_message_is_sending_joins_the_next_one() {
    use crate::composer::attachment::Source;
    use crate::composer::draft_session::DraftSession;
    let mut setup = thread_composer();
    setup.composer.config = Some(chat_thread().config);
    setup.composer.editor.borrow_mut().set_text("hello");
    setup
        .composer
        .send(setup.forge.host())
        .expect("the message is sent");
    let ThreadComposer {
        forge, composer, ..
    } = &mut setup;
    let post = test_support::wait::wait_for(
        "the message is posted",
        std::time::Duration::from_secs(10),
        || {
            drain_draft_writes(forge);
            forge.forge.try_take("POST", "/threads/chat/messages")
        },
    );
    assert!(
        composer
            .session
            .as_ref()
            .unwrap()
            .read(DraftSession::sending)
    );
    composer.add_image(
        Source::Bytes(test_support::editor::forge::png(7)),
        "Pasted image".into(),
    );
    post.respond(test_support::items::item(
        "sent",
        "chat",
        4,
        test_support::items::user_message("hello"),
    ));
    test_support::wait::wait_until(
        "the send is accepted",
        std::time::Duration::from_secs(10),
        || {
            drain_draft_writes(forge);
            composer.sync(&forge.core);
            !composer
                .session
                .as_ref()
                .unwrap()
                .read(DraftSession::sending)
        },
    );
    composer.sync(&forge.core);
    assert_eq!(
        composer
            .attachments
            .as_ref()
            .map(|attachments| attachments.items.len()),
        Some(1),
        "the image pasted during the send is in the composer for the next message"
    );
}
