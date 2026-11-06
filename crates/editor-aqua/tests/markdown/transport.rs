use aqua_text::document::{SelectionDocumentId, SelectionSourceId};
use artisan_api::{
    CreateProject, CreateThread, EngineConfig, EngineId, ItemId, ItemKind, Phase, ProjectId,
    SourceIndexFormat, Status, TextContent, ThreadId,
};
use editor_aqua::core::{Core, EditorFiles, ForgeClient, HostKind, hosts, runtime::runtime};
use editor_aqua::qualification::fixture::ForgeServer;
use editor_aqua::ui::markdown::{
    Block, IndexedLeafContent,
    presenter::MarkdownPresenter,
    stored::{LeafPreparation, StoredOwner, StoredPreparation, StoredSource},
    visual::BodyTone,
};
use editor_aqua::ui::theme::{ArtisanTheme, ThemeMode};
use std::{
    rc::Rc,
    sync::{Arc, mpsc},
    time::{Duration, Instant},
};

fn wait_wake(receiver: &mpsc::Receiver<()>, deadline: Instant) {
    let remaining = deadline
        .checked_duration_since(Instant::now())
        .expect("stored transport deadline");
    receiver
        .recv_timeout(remaining)
        .expect("stored transport wake");
}

fn wait_source(
    owner: &mut StoredOwner,
    receiver: &mpsc::Receiver<()>,
    deadline: Instant,
    start: Option<usize>,
) -> StoredSource {
    loop {
        owner.drain();
        match owner.current("row") {
            StoredPreparation::Ready(source)
                if start.is_none_or(|start| source.loaded_range().start == start) =>
            {
                return source;
            }
            StoredPreparation::Failed(error) => panic!("source index failed: {error}"),
            _ => wait_wake(receiver, deadline),
        }
    }
}

#[test]
fn real_forge_stored_markdown_moves_bounded_windows_and_revokes_leaf_proof() {
    let directory = tempfile::tempdir().unwrap();
    let server = runtime()
        .block_on(ForgeServer::start(
            directory.path().join("forge"),
            "127.0.0.1:0".parse().unwrap(),
        ))
        .unwrap();
    let text: String = (0..130)
        .map(|index| {
            format!(
                "Paragraph {index:03} with **bold** and {}.\n\n",
                "payload ".repeat(22)
            )
        })
        .collect();
    assert!(text.len() > artisan_api::INLINE_TEXT_BYTES);
    let (reference, item) = runtime().block_on(async {
        let project = server
            .store
            .create_project(CreateProject {
                id: ProjectId::new(),
                path: "/stored-test".into(),
                name: None,
            })
            .await
            .unwrap();
        let thread = server
            .store
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
        let item = server
            .store
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
            .unwrap();
        let ItemKind::AssistantMessage {
            text: TextContent::Stored(reference),
            ..
        } = &item.kind
        else {
            panic!("expected stored source")
        };
        (reference.clone(), item)
    });
    let files = EditorFiles {
        dir: directory.path().join("editor"),
    };
    let mut core = Core::for_tests(files).unwrap();
    let pairing = runtime().block_on(server.pairing_input()).unwrap();
    let config = runtime()
        .block_on(hosts::pair(
            &pairing.address,
            &pairing.code,
            &pairing.fingerprint,
            core.identity.as_ref().unwrap(),
            "stored-test",
            "Forge".to_owned(),
            HostKind::Local,
        ))
        .unwrap();
    let client = ForgeClient::new(
        &config.address,
        &config.fingerprint,
        core.identity.as_ref().unwrap(),
    )
    .unwrap();
    core.add_test_host(&config, client.clone());
    core.host_mut(&config.id).unwrap().test_stop_connection();
    let deadline = Instant::now() + Duration::from_secs(30);
    while core.test_startup_pending() {
        assert!(Instant::now() < deadline, "Core startup did not settle");
        core.drain_with(|_, _| {});
        std::thread::yield_now();
    }
    let instance = core.host(&config.id).unwrap().instance();
    let provider = core.source_provider(&config.id, instance).unwrap();
    let mut owner = StoredOwner::default();
    let (wake, received) = mpsc::channel();
    owner.wake_mailbox().unwrap().set_waker(Arc::new(move || {
        let _ = wake.send(());
    }));
    assert!(matches!(
        owner.ensure(
            "row",
            provider.clone(),
            reference.clone(),
            SourceIndexFormat::Markdown
        ),
        StoredPreparation::Pending
    ));
    let tail = wait_source(&mut owner, &received, deadline, None);
    assert!(tail.block_count() > 64);
    assert_eq!(tail.loaded_range().end, tail.block_count());
    assert!(tail.loaded_range().len() <= 64);
    assert!(tail.loaded_range().start > 0);
    assert!(matches!(
        owner.move_window("row", &tail, 0),
        StoredPreparation::Pending
    ));
    let first = wait_source(&mut owner, &received, deadline, Some(0));
    assert!(first.loaded_range().len() <= 64);
    let StoredPreparation::Ready(again) = owner.ensure(
        "row",
        provider.clone(),
        reference.clone(),
        SourceIndexFormat::Markdown,
    ) else {
        panic!("current window was not retained")
    };
    assert_eq!(again.loaded_range(), first.loaded_range());
    let descriptor = first.descriptor(0).unwrap().clone();
    assert!(first.descriptor(first.loaded_range().end).is_none());
    let leaf = loop {
        owner.drain();
        match owner.leaf("leaf-0", &first, 0) {
            LeafPreparation::Ready(leaf) => break leaf,
            LeafPreparation::Failed(error) => panic!("stored leaf failed: {error}"),
            LeafPreparation::Pending => wait_wake(&received, deadline),
        }
    };
    let start = usize::try_from(descriptor.source_start).unwrap();
    let end = usize::try_from(descriptor.source_end).unwrap();
    assert_eq!(leaf.source.as_ref(), &text[start..end]);
    assert!(matches!(
        &leaf.content,
        IndexedLeafContent::Block(Block::Paragraph { .. })
    ));
    assert!(owner.leaf_is_current(leaf.proof()));
    let mut presenter = MarkdownPresenter::default();
    let theme = ArtisanTheme::for_mode(ThemeMode::Dark);
    presenter
        .prepare_stored_leaf(
            &owner,
            "row",
            0,
            leaf.clone(),
            &theme,
            SelectionDocumentId(9),
            &[7],
            |part| SelectionSourceId(100 + part as u64),
        )
        .unwrap();
    let render = |presenter: &MarkdownPresenter, owner: &StoredOwner| {
        presenter.render_stored_leaf(
            owner,
            "row",
            0,
            0,
            true,
            &theme,
            BodyTone::Foreground,
            Rc::new(|_, _, _| aqua_ui::element::ui::Box::new().build()),
            Rc::new(|| {}),
            None,
            None,
            None,
            None,
            None,
        )
    };
    let projected = render(&presenter, &owner).unwrap();
    assert!(!projected.selection_batch.sources().is_empty());
    assert!(
        projected
            .geometry
            .text
            .iter()
            .any(|run| run.text.contains("Paragraph 000"))
    );
    assert!(owner.leaf_is_current(&projected.proof));
    stored_conversation_transaction(&mut core, &config, &client, item, deadline);
    let _ = owner.move_window("row", &first, 64);
    core.add_test_host(&config, client);
    assert!(!provider.is_available());
    owner.drain();
    assert!(!owner.leaf_is_current(leaf.proof()));
    assert!(render(&presenter, &owner).is_err());
    assert!(matches!(
        owner.ensure("row", provider, reference, SourceIndexFormat::Markdown),
        StoredPreparation::Failed(_)
    ));
    runtime().block_on(server.shutdown()).unwrap();
}

fn stored_conversation_transaction(
    core: &mut Core,
    config: &editor_aqua::core::HostConfig,
    client: &ForgeClient,
    item: artisan_api::Item,
    deadline: Instant,
) {
    use aqua_text::{SharedFonts, SharedRuns};
    use artisan_api::Event;
    use editor_aqua::conversation::{adapter::row_model, controller::Conversation, fold::{Row, Slot}, presentation::{BodyKey, PresentationScope}, stored::StoredConversation, view::{factory::{RealizedRow, RowFactory}, status::StatusShimmers, transcript::TranscriptObservations}};
    use editor_aqua::core::Screen;
    use std::cell::{Cell, RefCell};

    core.navigate(Screen::Thread { host: config.id.clone(), thread: item.thread_id.clone() });
    let mut conversation = Conversation::new(core);
    conversation.open_thread(core, config.id.clone(), item.thread_id.clone());
    let sender = core.host(&config.id).unwrap().test_transition_sender();
    runtime().block_on(sender.event(Event::Item { item: item.clone() }));
    while conversation.view.as_ref().and_then(|view| view.item(&item.id)).is_none() {
        core.drain_with(|core, event| { conversation.core_event(core, &event, false); });
        assert!(Instant::now() < deadline, "stored item delivery deadline");
        std::thread::yield_now();
    }
    let scope = PresentationScope::capture(&conversation, core).unwrap();
    let provider = core.source_provider(&scope.host, scope.instance).unwrap();
    let stored = Rc::new(RefCell::new(StoredConversation::default()));
    stored.borrow_mut().rebind(Some(scope.clone()));
    let (wake, received) = mpsc::channel();
    stored.borrow().wake_mailbox().unwrap().set_waker(Arc::new(move || { let _ = wake.send(()); }));
    let key = BodyKey { item: item.id.clone(), slot: Slot::Prose };
    let version = conversation.body_version(&key).unwrap();
    let ItemKind::AssistantMessage { text: TextContent::Stored(reference), .. } = &item.kind else { panic!("expected stored message") };
    assert!(matches!(stored.borrow_mut().ensure(&scope, &provider, key.clone(), reference.clone(), version).unwrap(), StoredPreparation::Pending));
    loop {
        stored.borrow_mut().drain_publish(&mut conversation, core);
        if let Some(range) = stored.borrow().current_range(&key) && range.end > range.start { break; }
        wait_wake(&received, deadline);
    }
    let range = stored.borrow().current_range(&key).unwrap();
    let ordinal = range.end - 1;
    let source = loop {
        let state = stored.borrow_mut().ensure(&scope, &provider, key.clone(), reference.clone(), version).unwrap();
        match state {
            StoredPreparation::Ready(source) => break source,
            StoredPreparation::Failed(error) => panic!("stored index failed: {error}"),
            StoredPreparation::Pending => {
                if !stored.borrow_mut().drain_publish(&mut conversation, core) {
                    wait_wake(&received, deadline);
                }
            }
        }
    };
    loop {
        let state = stored.borrow_mut().leaf(&key, &source, ordinal);
        match state {
            LeafPreparation::Ready(_) => break,
            LeafPreparation::Failed(error) => panic!("stored leaf failed: {error}"),
            LeafPreparation::Pending => {
                if !stored.borrow_mut().drain_publish(&mut conversation, core) {
                    wait_wake(&received, deadline);
                }
            }
        }
    }
    let index = conversation.rows.iter().position(|(row, _)| matches!(row, Row::Text { id, slot: Slot::Prose, block, .. } if *id == item.id && *block == ordinal)).unwrap();
    let model = row_model(&conversation, index, None).unwrap();
    let row_key = model.row.key();
    let document = SelectionDocumentId(555);
    let markdown = Rc::new(RefCell::new(editor_aqua::conversation::presentation::MarkdownOwners::new(document)));
    let observations = TranscriptObservations::default();
    let ids = Rc::new(Cell::new(1000));
    let allocated = ids.clone();
    let factory = Rc::new(RowFactory::new(vec![RealizedRow { model, disclosure: Default::default(), held: false }], scope.clone(), markdown.clone(), Rc::new(RefCell::new(StatusShimmers::default())), *ArtisanTheme::for_mode(ThemeMode::Dark), true, Rc::new(move || { let id = allocated.get() + 1; allocated.set(id); SelectionSourceId(id) }), Rc::new(|| {}), Rc::new(|_| {}), Rc::new(|_| {}), Rc::new(|_| {}))
        .with_sources(provider.clone(), document, Rc::new(|_| {})).unwrap()
        .with_stored(stored.clone(), provider).unwrap()
        .with_observations(observations.clone()));
    let realized = factory.realize(0, &row_key).unwrap();
    let source_id = realized.batches[0].sources()[0].id;
    let callback = factory.callback();
    let list_key = row_key.clone();
    let list = aqua_ui::collection::List::new().items(vec![0_usize]).key(move |_: &usize| format!("{list_key:?}")).try_batched_row(move |index: &usize| callback(*index, &row_key)).virtualize().estimated_row_height(aqua_ui::style::Length::Px(30.0)).style(aqua_ui::style::Style::new().width(aqua_ui::style::Length::Px(600.0)).height(aqua_ui::style::Length::Px(300.0))).build();
    let mut app = aqua::UiApp::new(SharedFonts::new(), SharedRuns::new());
    app.set_viewport(800.0, 600.0, 1.0);
    app.mount(|_| list).unwrap();
    app.layout().unwrap();
    app.build_frame().unwrap();
    let clip = aqua_core::Rect { x: 0.0, y: 0.0, w: 800.0, h: 600.0 };
    let painted = observations.drain().painted.into_iter().find(|paint| paint.snapshot.stored.is_some()).unwrap();
    let candidate = painted.clone().capture(&conversation, core, &markdown.borrow(), &scope, clip, true).unwrap();
    let stale_host = painted.capture(&conversation, core, &markdown.borrow(), &scope, clip, true).unwrap();
    assert!(app.unregister_selection_source_in(document, source_id));
    let mut changed = item.clone();
    changed.seq += 1;
    if let ItemKind::AssistantMessage { text, .. } = &mut changed.kind { *text = TextContent::Stored(artisan_api::ContentRef::from_text("different source")); }
    runtime().block_on(sender.event(Event::Item { item: changed.clone() }));
    while conversation.view.as_ref().and_then(|view| view.item(&item.id)).is_none_or(|current| current.seq != changed.seq) {
        core.drain_with(|core, event| { conversation.core_event(core, &event, false); });
        assert!(Instant::now() < deadline, "stored item update deadline");
        std::thread::yield_now();
    }
    assert!(!candidate.acknowledge(&conversation, core, &markdown.borrow()));
    core.add_test_host(config, client.clone());
    assert!(!stale_host.acknowledge(&conversation, core, &markdown.borrow()));
}
