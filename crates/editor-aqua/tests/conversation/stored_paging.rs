use editor_aqua::conversation::stored::{ROW_WINDOW, retired_leaves, row_window};

#[cfg(feature = "qualification")]
#[test]
fn paint_serial_advances_at_callback_and_survives_observation_clear() {
    use aqua_text::document::SelectionDocumentId;
    use editor_aqua::conversation::fold::Row;
    use editor_aqua::conversation::presentation::MarkdownOwners;
    use editor_aqua::conversation::view::transcript::{RowSnapshot, TranscriptObservations};
    use std::cell::{Cell, RefCell};
    use std::rc::Rc;

    let observations = TranscriptObservations::default();
    let snapshot = RowSnapshot {
        row: Row::Gap,
        version: 1,
        item_seq: None,
        source: None,
        active: Rc::new(Cell::new(true)),
        revealed: true,
        reveal_ready: None,
        live: None,
        stored: None,
    };
    let (_, paint) = observations.callbacks(
        snapshot,
        Rc::new(RefCell::new(MarkdownOwners::new(SelectionDocumentId(1)))),
    );
    let bounds = aqua_core::Rect {
        x: 0.0,
        y: 0.0,
        w: 10.0,
        h: 10.0,
    };
    let baseline = observations.qualification_paint_serial();
    paint(bounds);
    assert_eq!(observations.drain().painted[0].serial, baseline + 1);
    observations.clear();
    assert_eq!(observations.qualification_paint_serial(), baseline + 1);
    paint(bounds);
    assert_eq!(observations.drain().painted[0].serial, baseline + 2);
}

#[test]
fn leaves_outside_the_moved_window_retire_unless_mounted() {
    let mounted = |ordinal: usize| ordinal == 3;
    assert_eq!(
        retired_leaves(&(10..20), [3, 9, 10, 19, 20, 3, 30, 9], mounted),
        vec![9, 20, 30]
    );
    assert!(retired_leaves(&(0..64), [0, 31, 63], |_| false).is_empty());
    assert!(retired_leaves(&(10..20), std::iter::empty(), |_| false).is_empty());
}

#[test]
fn the_row_window_opens_at_the_tail_and_shifts_once_per_edge_reached() {
    let count = 5_000;
    let tail = row_window(None, count, None);
    assert_eq!(tail, count - ROW_WINDOW..count);
    assert_eq!(row_window(Some(&tail), count, Some(count - 1)), tail);
    assert_eq!(row_window(Some(&tail), count, Some(tail.start + 600)), tail);
    let earlier = row_window(Some(&tail), count, Some(tail.start + 10));
    assert!(earlier.start < tail.start && earlier.contains(&(tail.start + 10)));
    assert_eq!(earlier.len(), ROW_WINDOW);
    for reached in tail.start..tail.start + 200 {
        assert_eq!(row_window(Some(&earlier), count, Some(reached)), earlier);
    }
    let later = row_window(Some(&earlier), count, Some(earlier.end - 1));
    assert!(later.start > earlier.start && later.contains(&(earlier.end - 1)));
    let mut window = tail;
    while window.start > 0 {
        window = row_window(Some(&window), count, Some(window.start));
        assert!(window.contains(&window.start));
    }
    assert_eq!(window, 0..ROW_WINDOW);
    assert_eq!(row_window(None, 130, None), 0..130);
    assert_eq!(row_window(Some(&(0..130)), 130, Some(0)), 0..130);
}

#[cfg(feature = "qualification")]
#[test]
#[expect(
    clippy::too_many_lines,
    reason = "one failing-page scenario from the error to the retried load"
)]
fn a_stored_page_that_fails_to_load_says_why_and_loads_on_try_again() {
    use aqua_text::document::SelectionDocumentId;
    use artisan_api::{
        CreateProject, CreateThread, EngineConfig, EngineId, ItemId, ItemKind, Phase, ProjectId,
        Status, TextContent, ThreadId,
    };
    use editor_aqua::conversation::adapter::{ScopedAction, row_model};
    use editor_aqua::conversation::controller::Conversation;
    use editor_aqua::conversation::fold::{Row, Slot};
    use editor_aqua::conversation::presentation::{BodyKey, MarkdownOwners, PresentationScope};
    use editor_aqua::conversation::stored::StoredConversation;
    use editor_aqua::conversation::view::factory::RealizedRow;
    use editor_aqua::core::{ForgeClient, HostKind, Screen, hosts, runtime::runtime};
    use editor_aqua::qualification::fixture::ForgeServer;
    use editor_aqua::ui::markdown::stored::{LeafPreparation, StoredPreparation};
    use std::cell::RefCell;
    use std::collections::VecDeque;
    use std::rc::Rc;
    use std::sync::{Arc, mpsc};
    use std::time::{Duration, Instant};
    use test_support::editor::rows::row_factory_with;
    use test_support::editor::ui::{click, mounted, painted_texts};

    let directory = tempfile::tempdir().unwrap();
    let forge = directory.path().join("forge");
    let server = runtime()
        .block_on(ForgeServer::start(
            forge.clone(),
            "127.0.0.1:0".parse().unwrap(),
        ))
        .unwrap();
    let address = server.address();
    let filler = "words ".repeat(16);
    let text = (0..200)
        .map(|index| format!("Paragraph {index:03} of a long reply, {filler}.\n\n"))
        .collect::<Vec<_>>()
        .concat();
    let item = runtime().block_on(async {
        let project = server
            .store()
            .create_project(CreateProject {
                id: ProjectId::new(),
                path: "/stored-paging".into(),
                name: None,
            })
            .await
            .unwrap();
        let thread = server
            .store()
            .create_thread(CreateThread {
                id: ThreadId::new(),
                project_id: project.id,
                config: EngineConfig::new(EngineId::Codex),
                title: None,
                parent_thread_id: None,
                spawned_by: None,
            })
            .await
            .unwrap();
        server
            .store()
            .upsert_item(forge_store::NewItem {
                id: ItemId::new(),
                thread_id: thread.id,
                run_id: None,
                parent_id: None,
                native_id: None,
                status: Status::Done,
                kind: ItemKind::AssistantMessage {
                    text: text.clone().into(),
                    phase: Phase::Final,
                },
            })
            .await
            .unwrap()
    });
    let ItemKind::AssistantMessage {
        text: TextContent::Stored(reference),
        ..
    } = &item.kind
    else {
        panic!("a reply over the inline limit is stored")
    };
    let mut core = test_support::editor::core::test_core(&directory.path().join("editor"));
    let pairing = runtime().block_on(server.pairing_input()).unwrap();
    let config = runtime()
        .block_on(hosts::pair(
            &pairing.address,
            &pairing.code,
            &pairing.fingerprint,
            core.identity().unwrap(),
            "stored-paging",
            "Forge".to_owned(),
            HostKind::Local,
        ))
        .unwrap();
    let client = ForgeClient::new(
        &config.address,
        &config.fingerprint,
        core.identity().unwrap(),
    )
    .unwrap();
    test_support::editor::core::connect_client(&mut core, &config, client);
    let deadline = Instant::now() + Duration::from_secs(60);
    while core.pending_work() > 0 {
        assert!(Instant::now() < deadline, "Core startup did not settle");
        core.drain_with(|_, _| {});
        std::thread::yield_now();
    }
    core.navigate(Screen::Thread {
        host: config.id.clone(),
        thread: item.thread_id.clone(),
    });
    let mut conversation = Conversation::new(&core);
    conversation.open_thread(&mut core, config.id.clone(), item.thread_id.clone());
    while conversation
        .view
        .as_ref()
        .and_then(|view| view.item(&item.id))
        .is_none()
    {
        core.drain_with(|core, event| {
            conversation.core_event(core, &event, false);
        });
        conversation.drain(&mut core);
        assert!(
            Instant::now() < deadline,
            "the stored reply reaches the conversation"
        );
        std::thread::yield_now();
    }
    let scope = PresentationScope::capture(&conversation, &core).unwrap();
    let provider = core.source_provider(&scope.host, scope.instance).unwrap();
    let stored = Rc::new(RefCell::new(StoredConversation::default()));
    stored.borrow_mut().rebind(Some(scope.clone()));
    let (wake, woken) = mpsc::channel();
    stored
        .borrow()
        .wake_mailbox()
        .unwrap()
        .set_waker(Arc::new(move || {
            wake.send(()).expect("the test keeps the wake receiver");
        }));
    let wait = |woken: &mpsc::Receiver<()>, step: &str| {
        let remaining = deadline
            .checked_duration_since(Instant::now())
            .unwrap_or_else(|| panic!("{step} within the deadline"));
        woken
            .recv_timeout(remaining)
            .unwrap_or_else(|_| panic!("{step} wakes the conversation"));
    };
    let key = BodyKey {
        item: item.id.clone(),
        slot: Slot::Prose,
    };
    let version = conversation.body_version(&key).unwrap();
    stored
        .borrow_mut()
        .ensure(&scope, &provider, key.clone(), reference.clone(), version)
        .unwrap();
    while stored.borrow().block_count(&key).is_none() {
        stored.borrow_mut().drain_publish(&mut conversation, &core);
        if stored.borrow().block_count(&key).is_none() {
            wait(&woken, "the stored index loads");
        }
    }
    stored.borrow_mut().drain_publish(&mut conversation, &core);
    let blocks = stored.borrow().block_count(&key).unwrap();
    assert!(blocks > 2 * artisan_api::SOURCE_INDEX_PAGE_LEAVES as usize);
    let first = conversation
        .rows
        .iter()
        .position(|(row, _)| {
            matches!(row, Row::Text { id, slot: Slot::Prose, block: 0, .. } if *id == item.id)
        })
        .expect("every block of the stored reply has a row, not only the last page");
    runtime().block_on(server.shutdown()).unwrap();

    let actions: Rc<RefCell<VecDeque<ScopedAction>>> = Rc::default();
    let realize = |conversation: &Conversation| {
        let model = row_model(conversation, first, None).unwrap();
        let row_key = model.row.key();
        let queue = Rc::clone(&actions);
        let scoped = editor_aqua::conversation::view::scoped_dispatch(
            conversation,
            Rc::new(move |action| queue.borrow_mut().push_back(action)),
        )
        .unwrap();
        let (_, allocate) = test_support::editor::rows::source_ids(1000);
        let factory = row_factory_with(
            vec![RealizedRow {
                model,
                disclosure: editor_aqua::conversation::disclosure::RowDisclosure::default(),
                held: false,
            }],
            scope.clone(),
            Rc::new(RefCell::new(MarkdownOwners::new(SelectionDocumentId(77)))),
            true,
            allocate,
            Instant::now(),
            scoped,
            Rc::default(),
        )
        .with_sources(provider.clone(), SelectionDocumentId(77), Rc::new(|_| {}))
        .unwrap()
        .with_stored(Rc::clone(&stored), provider.clone())
        .unwrap();
        factory.realize(first, &row_key).unwrap().element
    };
    let retry_key = format!("retry-stored-page-{}-{}-0", item.id, Slot::Prose);
    let mut app = loop {
        stored.borrow_mut().drain_publish(&mut conversation, &core);
        let app = mounted(600.0, 400.0, realize(&conversation));
        if painted_texts(&app)
            .iter()
            .any(|text| text.starts_with("Couldn't load this part of the message: "))
        {
            break app;
        }
        wait(&woken, "the unreachable page fails");
    };
    let restarted = runtime()
        .block_on(ForgeServer::start(forge, address))
        .unwrap();
    click(&mut app, &retry_key);
    let retry = actions
        .borrow_mut()
        .pop_front()
        .expect("Try again dispatches a row action");
    assert!(retry.dispatch(
        &mut conversation,
        &mut stored.borrow_mut(),
        &core,
        true,
        Instant::now()
    ));
    let StoredPreparation::Ready(source) = stored
        .borrow_mut()
        .ensure(&scope, &provider, key.clone(), reference.clone(), version)
        .unwrap()
    else {
        panic!("the stored index stays ready while a page fails")
    };
    loop {
        stored.borrow_mut().drain_publish(&mut conversation, &core);
        let app = mounted(600.0, 400.0, realize(&conversation));
        let texts = painted_texts(&app);
        assert!(
            !texts.iter().any(|text| text.starts_with("Couldn't load")),
            "Try again starts the page over instead of repeating the failure"
        );
        if !texts.iter().any(|text| text.starts_with("Loading")) {
            break;
        }
        wait(&woken, "the retried page loads");
    }
    let LeafPreparation::Ready(leaf) = stored.borrow_mut().leaf(&key, &source, 0) else {
        panic!("the first block of the reply is prepared once its page loads")
    };
    assert!(leaf.source().starts_with("Paragraph 000"));
    runtime().block_on(restarted.shutdown()).unwrap();
}
