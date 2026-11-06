use super::*;
use crate::core::{EditorFiles, ForgeClient, HostConfig, HostKind};
use artisan_api::Status;

fn fixture() -> (tempfile::TempDir, std::net::TcpListener, Core, Conversation) {
    let directory = tempfile::tempdir().unwrap();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let mut core = Core::for_tests(EditorFiles {
        dir: directory.path().to_owned(),
    })
    .unwrap();
    let config = HostConfig {
        id: HostId("reset-host".into()),
        name: "Reset Forge".into(),
        address: listener.local_addr().unwrap().to_string(),
        fingerprint: "0".repeat(64),
        kind: HostKind::Local,
    };
    let client = ForgeClient::new(
        &config.address,
        &config.fingerprint,
        core.identity.as_ref().unwrap(),
    )
    .unwrap();
    core.add_test_host(&config, client);
    let mut conversation = Conversation::new(&core);
    conversation.open_thread(&mut core, config.id, ThreadId("thread".into()));
    (directory, listener, core, conversation)
}

fn origin(conversation: &Conversation) -> Origin {
    Origin {
        host: conversation.open.as_ref().unwrap().0.clone(),
        instance: conversation.host_instance.unwrap(),
        generation: conversation.generation,
    }
}

fn page(id: &str) -> ItemsPage {
    ItemsPage {
        items: vec![Item {
            id: ItemId(id.into()),
            thread_id: ThreadId("thread".into()),
            position: 1,
            seq: 1,
            run_id: None,
            parent_id: None,
            status: Status::Done,
            created_at: 0,
            updated_at: 0,
            kind: ItemKind::UserMessage {
                text: id.into(),
                images: Vec::new(),
                steer: false,
                from_agent: None,
                sender: None,
            },
        }],
        runs: Vec::new(),
        more: false,
        work: Vec::new(),
    }
}

fn enqueue(conversation: &Conversation, completion: Completion) {
    assert!(
        runtime()
            .block_on(conversation.delivery.sender().send(completion))
            .is_ok()
    );
}

fn finish_tail(conversation: &mut Conversation, core: &Core) {
    enqueue(
        conversation,
        Completion::Page {
            origin: origin(conversation),
            fetch: Fetch::Tail,
            result: Ok(page("current")),
        },
    );
    assert!(conversation.drain(core));
    assert!(conversation.page_task.is_none());
    let view = conversation.view.as_ref().unwrap();
    assert!(!view.viewport.in_flight);
    assert!(view.viewport.loaded());
    assert!(view.item(&ItemId("stale".into())).is_none());
    assert!(view.item(&ItemId("current".into())).is_some());
    assert!(
        conversation
            .rows
            .iter()
            .any(|(row, _)| row.anchor() == Some(&ItemId("current".into())))
    );
}

#[test]
fn reset_rejects_queued_pages_and_preserves_the_new_tail_until_it_loads() {
    for fetch in [Fetch::Tail, Fetch::Older { before: 10 }] {
        let (_directory, _listener, core, mut conversation) = fixture();
        let previous = origin(&conversation);
        enqueue(
            &conversation,
            Completion::Page {
                origin: previous.clone(),
                fetch,
                result: Ok(page("stale")),
            },
        );

        conversation.host_event(&core, &HostEvent::Reset, false);

        assert_ne!(conversation.generation, previous.generation);
        assert!(!conversation.drain(&core));
        assert!(conversation.page_task.is_some());
        assert!(conversation.rows.is_empty());
        assert_eq!(
            conversation.view.as_ref().unwrap().viewport.requested,
            Some(Fetch::Tail)
        );
        assert!(conversation.view.as_ref().unwrap().viewport.in_flight);
        finish_tail(&mut conversation, &core);
    }
}

#[test]
fn mismatched_fetch_in_current_generation_does_not_cancel_the_active_tail() {
    let (_directory, _listener, core, mut conversation) = fixture();
    enqueue(
        &conversation,
        Completion::Page {
            origin: origin(&conversation),
            fetch: Fetch::Older { before: 10 },
            result: Ok(page("stale")),
        },
    );

    assert!(!conversation.drain(&core));
    assert!(conversation.page_task.is_some());
    assert!(conversation.view.as_ref().unwrap().viewport.in_flight);
    finish_tail(&mut conversation, &core);
}

#[test]
fn reset_rejects_queued_work_even_when_the_new_span_reuses_its_position() {
    let (_directory, _listener, core, mut conversation) = fixture();
    let previous = origin(&conversation);
    enqueue(
        &conversation,
        Completion::Work {
            origin: previous,
            first: 10,
            result: Err(anyhow::anyhow!("stale work failed")),
        },
    );
    conversation.host_event(&core, &HostEvent::Reset, false);
    conversation.work_reading = Some(10);
    conversation.work_task = Some(AbortOnDropHandle::new(
        runtime().spawn(std::future::pending()),
    ));

    assert!(!conversation.drain(&core));
    assert_eq!(conversation.work_reading, Some(10));
    assert!(conversation.work_task.is_some());
    assert!(conversation.load_error.is_none());
    enqueue(
        &conversation,
        Completion::Work {
            origin: origin(&conversation),
            first: 10,
            result: Ok(ItemsPage {
                items: Vec::new(),
                runs: Vec::new(),
                more: false,
                work: Vec::new(),
            }),
        },
    );
    assert!(conversation.drain(&core));
    assert!(conversation.work_task.is_none());
    assert!(conversation.work_reading.is_none());
    finish_tail(&mut conversation, &core);
}

#[test]
fn reset_restarts_pending_agent_context_without_accepting_its_old_completion() {
    let (_directory, _listener, core, mut conversation) = fixture();
    conversation.agent = Some(ItemId("agent".into()));
    conversation.agent_task = Some(AbortOnDropHandle::new(
        runtime().spawn(std::future::pending()),
    ));
    let previous = origin(&conversation);
    enqueue(
        &conversation,
        Completion::Agent {
            origin: previous.clone(),
            context: HashMap::new(),
            error: Some("stale ancestry failure".into()),
            effects: Vec::new(),
        },
    );

    conversation.host_event(&core, &HostEvent::Reset, false);

    assert_ne!(conversation.generation, previous.generation);
    assert_eq!(conversation.agent, Some(ItemId("agent".into())));
    assert!(conversation.agent_task.is_some());
    assert!(!conversation.drain(&core));
    assert!(conversation.agent_task.is_some());
    assert!(conversation.load_error.is_none());
}
