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

#[test]
fn a_restored_send_waiting_on_a_choice_asks_for_the_choice_and_refuses_another_send() {
    use super::{Composer, ComposerTarget, SendRefusal, Submission};
    use crate::composer::draft_body::DraftBody;
    use crate::composer::draft_session::{DraftSession, KeptDraft};
    use crate::composer::persistence::DraftSessions;
    use crate::composer::spool::tests::Direct as _;
    use crate::composer::spool::{DraftSpool, Holder, Slot};
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
    let spool = DraftSpool::new(directory.path());
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
    composer.config = Some(artisan_api::EngineConfig::new(artisan_api::EngineId::Codex));
    composer.project = Some(artisan_api::ProjectId::from("project"));
    composer.editor.borrow_mut().set_text("another message");
    let host = core.host(&config.id).unwrap();
    assert!(
        composer
            .session
            .as_ref()
            .unwrap()
            .read(DraftSession::sending)
    );

    assert_eq!(
        composer.blocked(host),
        Some("Choose which draft to keep first."),
        "the restored message waits on the choice, not on Forge"
    );
    assert_eq!(composer.send(host), Err(SendRefusal::ChoosingDraft));
    assert_eq!(
        composer.session.as_ref().unwrap().submit(Submission {
            message: artisan_api::SendMessage {
                id: artisan_api::ItemId("bypassing".into()),
                text: "another message".into(),
                images: Vec::new(),
                config: None,
                draft: None,
            },
            create: None,
            thread: ThreadId("thread".into()),
        }),
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
    use crate::composer::spool::{DraftSpool, Holder, Slot};
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
    let spool = DraftSpool::new(directory.path());
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
    assert_eq!(composer.send(host), Ok(()));
    assert!(
        composer
            .session
            .as_ref()
            .unwrap()
            .read(DraftSession::sending),
        "once the user has chosen, the message goes out"
    );
}
