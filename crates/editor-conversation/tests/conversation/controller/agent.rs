use super::*;
use crate::conversation::viewport::State;
use artisan_api::{Status, TextContent};
use editor_core::{EditorFiles, ForgeClient, HostConfig, HostKind};

fn fixture() -> (tempfile::TempDir, std::net::TcpListener, Core, Conversation) {
    let directory = tempfile::tempdir().unwrap();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let mut core = Core::for_tests(EditorFiles {
        dir: directory.path().to_owned(),
    })
    .unwrap();
    let config = HostConfig {
        id: HostId("agent-host".into()),
        name: "Agent Forge".into(),
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

fn item(id: &str, position: Seq, parent: Option<&str>, kind: ItemKind) -> Item {
    Item {
        id: ItemId(id.into()),
        thread_id: ThreadId("thread".into()),
        position,
        seq: 1,
        run_id: None,
        parent_id: parent.map(|parent| ItemId(parent.into())),
        status: Status::Done,
        created_at: 0,
        updated_at: 0,
        kind,
    }
}

fn agent() -> Item {
    item(
        "agent",
        1,
        None,
        ItemKind::Agent {
            name: None,
            description: TextContent::Inline("Survey the renderer".into()),
            prompt: Some(TextContent::Inline("A long task. ".repeat(200))),
            agent_id: None,
            context_used: None,
            model: None,
            background: false,
            result: None,
            label: None,
            engine: None,
            thread: None,
        },
    )
}

fn step(position: Seq) -> Item {
    item(
        &format!("step-{position}"),
        position,
        Some("agent"),
        ItemKind::AssistantMessage {
            text: TextContent::Inline(format!("Step {position}")),
            phase: artisan_api::Phase::Final,
        },
    )
}

fn deliver(conversation: &mut Conversation, core: &Core, completion: Completion) {
    assert!(
        runtime()
            .block_on(conversation.delivery.sender().send(completion))
            .is_ok()
    );
    conversation.drain(core);
}

fn origin(conversation: &Conversation) -> Origin {
    Origin {
        host: conversation.open.as_ref().unwrap().0.clone(),
        instance: conversation.host_instance.unwrap(),
        generation: conversation.generation,
    }
}

fn page(items: Vec<Item>) -> ItemsPage {
    ItemsPage {
        items,
        runs: Vec::new(),
        more: false,
        work: Vec::new(),
    }
}

#[test]
fn scrolling_up_into_a_subagent_task_stops_following_the_tail() {
    let (_directory, _listener, core, mut conversation) = fixture();
    let thread = origin(&conversation);
    deliver(
        &mut conversation,
        &core,
        Completion::Page {
            origin: thread,
            fetch: Fetch::Tail,
            asked: Instant::now(),
            result: Ok(page(vec![agent(), step(2), step(3)])),
        },
    );

    conversation.open_agent(&core, Some(ItemId("agent".into())));
    let scoped = origin(&conversation);
    let effects = vec![Effect::Fetch(Fetch::Tail)];
    deliver(
        &mut conversation,
        &core,
        Completion::Agent {
            origin: scoped.clone(),
            context: HashMap::from([(ItemId("agent".into()), Arc::new(agent()))]),
            error: None,
            effects,
        },
    );
    deliver(
        &mut conversation,
        &core,
        Completion::Page {
            origin: scoped,
            fetch: Fetch::Tail,
            asked: Instant::now(),
            result: Ok(page(vec![step(2), step(3)])),
        },
    );
    assert_eq!(
        conversation.rows.first().and_then(|(row, _)| row.anchor()),
        Some(&ItemId("agent".into()))
    );
    assert!(
        conversation
            .view
            .as_ref()
            .unwrap()
            .viewport
            .state()
            .is_following()
    );

    let inputs = super::super::scroll::scrolled(
        &conversation,
        0,
        600.0,
        -100.0,
        4000.0,
        true,
        false,
        |index| index >= 1,
    );
    for input in inputs {
        conversation.viewport(&core, input);
    }

    assert_eq!(
        conversation.view.as_ref().unwrap().viewport.state(),
        &State::Reading {
            anchor: 2,
            through: 2
        }
    );
}
