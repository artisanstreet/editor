use aqua_text::document::{SelectionDocumentId, SelectionSource, SelectionSourceId};
use aqua_text::{SharedFonts, SharedRuns};
use aqua_ui::element::Text;
use artisan_api::{ItemId, ThreadId};
use editor_aqua::conversation::controller::Conversation;
use editor_aqua::conversation::fold::Slot;
use editor_aqua::conversation::presentation::{
    BodyKey, MarkdownOwners, MarkdownRender, PreparedElement, PresentationScope, SelectionOwners,
};
use editor_aqua::core::Core;
use std::rc::Rc;
use std::sync::Arc;
use test_support::editor::core::{OpenThread, connect, remote_host, test_core};
use test_support::editor::rows::{
    fixed_source, listed, realized, row_factory, row_factory_with, row_list, row_model, source_ids,
};
use test_support::editor::ui::{click, mount, mounted, node_with_key, painted_texts, ui_app};
use test_support::items::{self, assistant_reply, read_tool, user_message};
use test_support::wait::wait_until;

fn source(id: u64, text: &str) -> SelectionSource {
    SelectionSource {
        id: SelectionSourceId(id),
        order: vec![id],
        text: Arc::from(text),
        separator: Arc::from("\n\n"),
        copies: Vec::new(),
    }
}

#[test]
fn measured_heights_ignore_stale_rows_and_invalid_values() {
    use editor_aqua::conversation::fold::{Row, RowKey};
    use editor_aqua::conversation::view::transcript::{HeightObservation, apply_heights};
    let active = Rc::new(std::cell::Cell::new(true));
    let height = |version, height| HeightObservation {
        key: RowKey::Gap,
        version,
        height,
        active: Rc::clone(&active),
    };
    let dir = tempfile::tempdir().unwrap();
    let core = test_core(dir.path());
    let mut conversation = Conversation::new(&core);
    conversation.rows = vec![(Row::Gap, 7)];
    assert!(!apply_heights(
        &mut conversation,
        [height(6, 40.0), height(7, f32::NAN)]
    ));
    assert!(conversation.row_heights.is_empty());
    assert!(apply_heights(&mut conversation, [height(7, 48.0)]));
    assert!(!apply_heights(&mut conversation, [height(7, 48.0)]));
    active.set(false);
    assert!(!apply_heights(&mut conversation, [height(7, 55.0)]));
    assert_eq!(conversation.row_heights.get(&RowKey::Gap), Some(&48.0));
}

#[test]
#[expect(
    clippy::too_many_lines,
    reason = "one admission-order scenario walked across rebuilds"
)]
fn mounted_fresh_work_rows_enter_in_admission_order_across_factory_rebuilds() {
    use artisan_api::{Item, RunId, Status};
    use editor_aqua::conversation::adapter::RowModel;
    use editor_aqua::conversation::fold::{Row, RowKey, RunSummary};
    use editor_aqua::conversation::phase::Phase;
    use editor_aqua::conversation::thread::ItemUi;
    use editor_aqua::conversation::view::factory::RealizedRow;
    use editor_aqua::conversation::view::motion::RowMotion;
    use std::cell::RefCell;
    use std::collections::HashMap;
    use std::time::{Duration, Instant};

    let opened = OpenThread::new("work-host", "work-thread");
    let scope = opened.scope();
    let OpenThread {
        directory: _directory,
        mut core,
        host: config,
        thread,
        mut conversation,
    } = opened;
    let mut app = ui_app(800.0, 600.0);
    let markdown = Rc::new(RefCell::new(MarkdownOwners::new(SelectionDocumentId(803))));
    markdown.borrow_mut().rebind(&mut app, Some(scope.clone()));
    let motion = Rc::new(RefCell::new(RowMotion::default()));
    let row_cache = Rc::new(RefCell::new(
        editor_aqua::conversation::view::row_cache::RowCache::default(),
    ));
    let now = Instant::now();
    let ids = [ItemId("work-first".into()), ItemId("work-second".into())];
    let keys = [RowKey::Item(ids[0].clone()), RowKey::Item(ids[1].clone())];
    let rows = || {
        ids.iter()
            .enumerate()
            .map(|(index, id)| {
                let item = Arc::new(Item {
                    run_id: Some(RunId("run".into())),
                    status: Status::Running,
                    ..items::item(
                        id.clone(),
                        thread.clone(),
                        u64::try_from(index).unwrap() + 1,
                        read_tool("src/main.rs"),
                    )
                });
                let row = Row::WorkEntry {
                    id: id.clone(),
                    run: Some(RunId("run".into())),
                };
                realized(RowModel {
                    ui: HashMap::from([(
                        id.clone(),
                        ItemUi {
                            arrived: Some(now),
                            ..Default::default()
                        },
                    )]),
                    run: Some(RunSummary {
                        phase: Phase::Working,
                        started_at: None,
                        ended_at: None,
                        open: true,
                        worked: true,
                        doing: None,
                    }),
                    gap: 8.0,
                    ..row_model(row, [item])
                })
            })
            .collect::<Vec<RealizedRow>>()
    };
    let factory = |at| {
        Rc::new(
            row_factory_with(
                rows(),
                scope.clone(),
                Rc::clone(&markdown),
                false,
                fixed_source(9000),
                at,
                Rc::new(|_| {}),
                Rc::clone(&motion),
            )
            .with_admission(Rc::clone(&conversation.reveal))
            .with_row_cache(Rc::clone(&row_cache)),
        )
    };
    let first = factory(now);
    mount(
        &mut app,
        row_list(&first, listed(keys.clone()), 30.0, 300.0),
    );
    app.build_frame().unwrap();
    assert_eq!(
        conversation.reveal.borrow().frontier_key(now),
        Some(keys[0].clone())
    );
    assert!(first.sync_after_layout(&mut app));

    let next = factory(now + Duration::from_millis(250));
    next.realize(0, &keys[0]).unwrap();
    next.realize(1, &keys[1]).unwrap();
    assert_eq!(
        conversation
            .reveal
            .borrow()
            .frontier_key(now + Duration::from_millis(250)),
        Some(keys[1].clone())
    );
    core.remove_host(&config.id).unwrap();
    connect(&mut core, &config);
    conversation.open_thread(&mut core, config.id.clone(), thread.clone());
    let replacement_scope = PresentationScope::capture(&conversation, &core).unwrap();
    let _replacement_factory = row_factory_with(
        Vec::new(),
        replacement_scope,
        Rc::clone(&markdown),
        false,
        fixed_source(9001),
        Instant::now(),
        Rc::new(|_| {}),
        Rc::clone(&motion),
    )
    .with_row_cache(Rc::clone(&row_cache));
    assert!(next.realize(0, &keys[0]).is_err());
    first.dispose();
    assert!(first.realize(0, &keys[0]).is_err());
}

#[test]
fn mounted_live_reveal_requires_current_complete_coverage_and_host_lifetime() {
    exercise_mounted_live_reveal(false);
}

#[test]
fn mounted_live_reveal_uses_live_rope_when_saved_source_is_stored() {
    exercise_mounted_live_reveal(true);
}

#[expect(
    clippy::too_many_lines,
    reason = "one mounted reveal scenario walked step by step"
)]
fn exercise_mounted_live_reveal(stored_saved: bool) {
    use artisan_api::{Event, Item, LiveCoverage, LiveTarget, Status, TextContent};
    use editor_aqua::conversation::adapter::row_model;
    use editor_aqua::conversation::fold::{Row, RowKey};
    use editor_aqua::conversation::live::LiveBinding;
    use editor_aqua::conversation::view::reveal::MountedReveals;
    use editor_aqua::conversation::view::transcript::TranscriptObservations;
    use editor_aqua::core::runtime::runtime;
    use editor_aqua::core::{CoreEvent, HostEvent, Screen};
    use std::cell::RefCell;
    use std::time::Duration;

    let directory = tempfile::tempdir().unwrap();
    let mut core = test_core(directory.path());
    let config = remote_host("live-host");
    connect(&mut core, &config);
    core.host_mut(&config.id).unwrap().test_stop_connection();
    let thread = ThreadId("live-thread".into());
    let id = ItemId("live-reply".into());
    core.navigate(Screen::Thread {
        host: config.id.clone(),
        thread: thread.clone(),
    });
    let mut conversation = Conversation::new(&core);
    conversation.open_thread(&mut core, config.id.clone(), thread.clone());
    let sender = core.host(&config.id).unwrap().test_transition_sender();
    let target = LiveTarget::Text {
        item_id: id.clone(),
    };
    let drain_live = |core: &mut Core, conversation: &mut Conversation| {
        wait_until("the matching live event", Duration::from_secs(15), || {
            let mut delivered = false;
            core.drain_with(|core, event| {
                delivered |= matches!(&event, CoreEvent::Host { host, event: HostEvent::Live { thread_id, target: actual } }
                    if host == &config.id && thread_id == &thread && actual == &target);
                conversation.core_event(core, &event, false);
            });
            delivered
        });
    };
    runtime().block_on(async {
        sender
            .event(Event::Item {
                item: Item {
                    status: Status::Running,
                    ..items::item(
                        id.clone(),
                        thread.clone(),
                        1,
                        artisan_api::ItemKind::AssistantMessage {
                            text: if stored_saved {
                                TextContent::Stored(artisan_api::ContentRef::from_text(
                                    "saved reply",
                                ))
                            } else {
                                TextContent::Inline(String::new())
                            },
                            phase: artisan_api::Phase::Final,
                        },
                    )
                },
            })
            .await;
        sender
            .event(Event::Live {
                thread_id: thread.clone(),
                target: target.clone(),
                text: "visible words ".into(),
                append: false,
                coverage: LiveCoverage {
                    start: 10,
                    total: 24,
                },
            })
            .await;
    });
    drain_live(&mut core, &mut conversation);
    let first = LiveBinding::capture(&conversation, &core, &id).unwrap();
    assert!(first.current(&conversation, &core));
    assert!(!first.complete(&conversation, &core));
    assert!(first.preview_notice().unwrap().contains("14 of 24 bytes"));
    let index = conversation.rows.iter().position(|(row, _)| matches!(row, Row::Text { id: row_id, slot: Slot::Prose, .. } if row_id == &id)).unwrap();
    let model = row_model(&conversation, index, None).unwrap();
    let frozen_rows =
        editor_aqua::conversation::adapter::RowSnapshots::capture(&conversation, None).unwrap();
    let scope = PresentationScope::capture(&conversation, &core).unwrap();
    let mut app = ui_app(800.0, 600.0);
    let markdown = Rc::new(RefCell::new(MarkdownOwners::new(SelectionDocumentId(801))));
    markdown.borrow_mut().rebind(&mut app, Some(scope.clone()));
    let owners = Rc::new(RefCell::new(MountedReveals::default()));
    let row_cache = Rc::new(RefCell::new(
        editor_aqua::conversation::view::row_cache::RowCache::default(),
    ));
    let observations = TranscriptObservations::default();
    let factory = Rc::new(
        row_factory(
            vec![realized(model)],
            scope.clone(),
            Rc::clone(&markdown),
            true,
            source_ids(900).1,
        )
        .with_observations(observations.clone())
        .with_row_cache(Rc::clone(&row_cache))
        .with_reveals(&conversation, &core, Rc::clone(&owners), false)
        .unwrap(),
    );
    let row_key = RowKey::Item(id.clone());
    mount(
        &mut app,
        row_list(&factory, listed([row_key.clone()]), 30.0, 300.0),
    );
    app.build_frame().unwrap();
    let sync = factory.sync_reveals(&mut app, &mut conversation, &core);
    assert!(!sync.pending);
    let settled = factory.sync_reveals(&mut app, &mut conversation, &core);
    assert!(
        !settled.frontier_changed,
        "an unchanged admission frontier must not request a rebuild"
    );
    assert_eq!(owners.borrow().ready(&row_key), Some(false));
    let original = owners.borrow().retained_for_tests(&row_key).unwrap();
    let replacement = row_factory(
        vec![realized(row_model(&conversation, index, None).unwrap())],
        scope.clone(),
        Rc::clone(&markdown),
        true,
        fixed_source(9999),
    )
    .with_row_cache(Rc::clone(&row_cache))
    .with_reveals(&conversation, &core, Rc::clone(&owners), false)
    .unwrap();
    replacement.realize(0, &row_key).unwrap();
    assert!(Rc::ptr_eq(
        &original,
        &owners.borrow().retained_for_tests(&row_key).unwrap()
    ));
    let painted = observations.drain().painted;
    let clip = aqua_core::Rect {
        x: 0.0,
        y: 0.0,
        w: 800.0,
        h: 600.0,
    };
    assert!(painted.into_iter().all(|paint| {
        paint
            .capture(&conversation, &core, &markdown.borrow(), &scope, clip, true)
            .is_none()
    }));
    runtime().block_on(sender.event(Event::Live {
        thread_id: thread.clone(),
        target: target.clone(),
        text: "visible words ".into(),
        append: false,
        coverage: LiveCoverage {
            start: 0,
            total: 14,
        },
    }));
    drain_live(&mut core, &mut conversation);
    assert!(!first.current(&conversation, &core));
    let old_model = frozen_rows.model(index).unwrap();
    assert!(Arc::ptr_eq(
        old_model.live.get(&target).unwrap(),
        &first.text
    ));
    let complete = LiveBinding::capture(&conversation, &core, &id).unwrap();
    assert!(complete.complete(&conversation, &core));
    assert!(
        factory
            .sync_reveals(&mut app, &mut conversation, &core)
            .pending
    );
    let complete_index = conversation.rows.iter().position(|(row, _)| matches!(row, Row::Text { id: row_id, slot: Slot::Prose, .. } if row_id == &id)).unwrap();
    let complete_observations = TranscriptObservations::default();
    let complete_factory = Rc::new(
        row_factory(
            vec![],
            scope.clone(),
            Rc::clone(&markdown),
            true,
            fixed_source(9999),
        )
        .with_lazy_rows(&conversation, &core, None)
        .unwrap()
        .with_observations(complete_observations.clone())
        .with_row_cache(Rc::clone(&row_cache))
        .with_reveals(&conversation, &core, Rc::clone(&owners), false)
        .unwrap(),
    );
    let mut complete_app = mounted(
        800.0,
        600.0,
        row_list(
            &complete_factory,
            vec![(complete_index, row_key.clone())],
            30.0,
            300.0,
        ),
    );
    complete_app.build_frame().unwrap();
    let sync = complete_factory.sync_reveals(&mut complete_app, &mut conversation, &core);
    assert!(!sync.pending);
    assert_eq!(owners.borrow().ready(&row_key), Some(true));
    assert!(
        complete_observations
            .drain()
            .painted
            .into_iter()
            .any(|paint| paint
                .capture(&conversation, &core, &markdown.borrow(), &scope, clip, true)
                .is_some())
    );
    assert!(Rc::ptr_eq(
        &original,
        &owners.borrow().retained_for_tests(&row_key).unwrap()
    ));
    let mounted_owner = row_cache.borrow().get(&row_key).unwrap();
    let mounted_weak = Rc::downgrade(&mounted_owner);
    drop(mounted_owner);
    for index in 0..128 {
        let key = RowKey::Item(ItemId(format!("cache-eviction-{index}")));
        let owner = row_cache.borrow().candidate(&key);
        row_cache.borrow_mut().commit(key, owner);
    }
    assert!(row_cache.borrow().get(&row_key).is_none());
    assert!(mounted_weak.upgrade().is_some());
    let node = node_with_key(&complete_app, &format!("{row_key:?}"));
    assert!(complete_app.tree_mut().remove(node));
    complete_factory.sync_reveals(&mut complete_app, &mut conversation, &core);
    assert_eq!(owners.borrow().ready(&row_key), Some(false));
    complete_factory.realize(complete_index, &row_key).unwrap();
    let remounted = owners.borrow().retained_for_tests(&row_key).unwrap();
    assert!(!Rc::ptr_eq(&original, &remounted));
    assert_eq!(complete_factory.revision(complete_index, &row_key), None);
    core.remove_host(&config.id).unwrap();
    assert!(!complete.current(&conversation, &core));
    assert!(
        factory
            .sync_reveals(&mut app, &mut conversation, &core)
            .pending
    );
    connect(&mut core, &config);
    assert!(!complete.current(&conversation, &core));
    assert!(
        factory
            .sync_reveals(&mut app, &mut conversation, &core)
            .pending
    );
    factory.dispose();
    assert!(factory.realize(0, &row_key).is_err());
}

#[test]
#[expect(
    clippy::too_many_lines,
    reason = "one factory lifetime scenario walked step by step"
)]
fn realized_factory_rows_share_registered_sources_and_stale_factory_is_rejected() {
    use artisan_api::{Item, ItemKind, TextContent};
    use editor_aqua::conversation::adapter::RowModel;
    use editor_aqua::conversation::fold::{Row, RowKey};
    use std::cell::{Cell, RefCell};

    let opened = OpenThread::new("factory-host", "thread");
    let scope = opened.scope();
    let OpenThread {
        directory: _directory,
        core,
        thread,
        ..
    } = opened;
    let mut app = ui_app(800.0, 600.0);
    let document = SelectionDocumentId(28);
    let mut markdown = MarkdownOwners::new(document);
    markdown.rebind(&mut app, Some(scope.clone()));
    let id = ItemId("reply".into());
    let text = "First paragraph.\n\nSecond paragraph.";
    let key = BodyKey {
        item: id.clone(),
        slot: Slot::Prose,
    };
    let item = Arc::new(items::item(id.clone(), thread, 1, assistant_reply(text)));
    let make_row = |block| {
        realized(row_model(
            Row::Text {
                id: id.clone(),
                slot: Slot::Prose,
                block,
                run: None,
            },
            [Arc::clone(&item)],
        ))
    };
    let markdown = Rc::new(RefCell::new(markdown));
    let (next, allocate) = source_ids(40);
    let factory = Rc::new(row_factory(
        vec![make_row(0), make_row(1)],
        scope.clone(),
        Rc::clone(&markdown),
        true,
        allocate,
    ));
    let keys = [RowKey::Item(id.clone()), RowKey::Text(id, Slot::Prose, 1)];
    mount(
        &mut app,
        row_list(&factory, listed(keys.clone()), 30.0, 300.0),
    );
    assert_eq!(next.get(), 42);
    let range = markdown.borrow().projected_range(&key, 0).unwrap();
    assert!(
        markdown
            .borrow()
            .projected_current(&scope, &key, text, 0, &range)
    );
    assert!(
        !markdown
            .borrow()
            .projected_current(&scope, &key, "changed", 0, &range)
    );
    assert!(app.unregister_selection_source_in(document, SelectionSourceId(41)));
    assert!(app.unregister_selection_source_in(document, SelectionSourceId(42)));
    let proof = markdown.borrow().capture_projected(&key, 1).unwrap();
    assert!(markdown.borrow().accepts_projected(&scope, &proof, text));
    let engine = editor_aqua::ui::markdown::MarkdownEngine::new().unwrap();
    markdown
        .borrow_mut()
        .prepare_on_realization(&scope, key, "reply", &engine, "replacement", 2)
        .unwrap();
    assert!(!markdown.borrow().accepts_projected(&scope, &proof, text));
    factory.dispose();
    assert!(factory.realize(0, &keys[0]).is_err());

    let activations = Rc::new(Cell::new(0));
    let activated = Rc::clone(&activations);
    let gap_factory = row_factory_with(
        vec![realized(RowModel {
            version: 3,
            ..row_model(Row::Gap, [])
        })],
        scope.clone(),
        Rc::clone(&markdown),
        true,
        fixed_source(999),
        std::time::Instant::now(),
        Rc::new(move |_| activated.set(activated.get() + 1)),
        Rc::default(),
    )
    .with_observations(
        editor_aqua::conversation::view::transcript::TranscriptObservations::default(),
    );
    let gap = gap_factory.realize(0, &RowKey::Gap).unwrap().element;
    let mut gap_app = mounted(800.0, 600.0, gap);
    gap_app.build_frame().unwrap();
    click(&mut gap_app, "load-newer");
    assert_eq!(activations.get(), 1);

    let user_id = ItemId("user-message".into());
    let user_text = "First user paragraph.\n\nSecond user paragraph.";
    let user_item = Arc::new(Item {
        kind: ItemKind::UserMessage {
            text: TextContent::Inline(user_text.into()),
            images: vec![artisan_api::BlobHash("user-image".into())],
            steer: false,
            from_agent: None,
            sender: Some(artisan_api::Sender {
                thread: ThreadId("sender-thread".into()),
                engine: artisan_api::EngineId::Claude,
                title: None,
            }),
        },
        ..items::item(user_id.clone(), item.thread_id.clone(), 2, user_message(""))
    });
    let user_slot = Slot::Text(editor_aqua::conversation::fold::TextSection::User);
    let user_row = |block| {
        realized(RowModel {
            version: 2,
            ..row_model(
                Row::Text {
                    id: user_id.clone(),
                    slot: user_slot,
                    block,
                    run: None,
                },
                [Arc::clone(&user_item)],
            )
        })
    };
    let (user_ids, allocate) = source_ids(100);
    let blob_events = Rc::new(RefCell::new(Vec::new()));
    let emitted = Rc::clone(&blob_events);
    let images = core.host(&scope.host).unwrap().blob_images();
    let user_factory = Rc::new(
        row_factory(
            vec![user_row(0), user_row(1)],
            scope.clone(),
            Rc::clone(&markdown),
            true,
            allocate,
        )
        .with_blob_images(
            images,
            Rc::new(move |item, seq, action| {
                emitted.borrow_mut().push((item, seq, action));
            }),
        ),
    );
    let user_keys = [
        RowKey::Item(user_id.clone()),
        RowKey::Text(user_id.clone(), user_slot, 1),
    ];
    let mut user_app = mounted(
        800.0,
        600.0,
        row_list(&user_factory, listed(user_keys), 30.0, 300.0),
    );
    assert!(blob_events.borrow().is_empty());
    user_app.build_frame().unwrap();
    assert!(blob_events.borrow().iter().any(|(id, seq, action)| {
        id == &user_id && *seq == 2 && matches!(action, editor_aqua::conversation::view::factory::BlobAction::Load { hash, retry: false } if hash.0 == "user-image")
    }));
    assert_eq!(user_ids.get(), 102);
    let key = BodyKey {
        item: user_id.clone(),
        slot: user_slot,
    };
    let proof = markdown.borrow().capture_projected(&key, 1).unwrap();
    assert!(
        markdown
            .borrow()
            .accepts_projected(&scope, &proof, user_text)
    );
    assert!(user_app.unregister_selection_source_in(document, SelectionSourceId(101)));
    assert!(user_app.unregister_selection_source_in(document, SelectionSourceId(102)));

    let mut empty_item = (*user_item).clone();
    if let ItemKind::UserMessage { text, .. } = &mut empty_item.kind {
        *text = TextContent::Inline(String::new());
    }
    let mut empty_row = user_row(0);
    empty_row
        .model
        .items
        .insert(user_id.clone(), Arc::new(empty_item));
    let empty_events = Rc::new(RefCell::new(Vec::new()));
    let emitted = Rc::clone(&empty_events);
    let empty_factory = Rc::new(
        row_factory(
            vec![empty_row],
            scope.clone(),
            Rc::clone(&markdown),
            true,
            fixed_source(999),
        )
        .with_blob_images(
            core.host(&scope.host).unwrap().blob_images(),
            Rc::new(move |id, seq, action| {
                emitted.borrow_mut().push((id, seq, action));
            }),
        ),
    );
    let mut empty_app = mounted(
        800.0,
        600.0,
        row_list(
            &empty_factory,
            listed([RowKey::Item(user_id.clone())]),
            120.0,
            300.0,
        ),
    );
    assert!(empty_events.borrow().is_empty());
    empty_app.build_frame().unwrap();
    assert!(empty_events.borrow().iter().any(|(id, seq, action)| {
        id == &user_id && *seq == 2 && matches!(action, editor_aqua::conversation::view::factory::BlobAction::Load { hash, retry: false } if hash.0 == "user-image")
    }));
    assert!(
        markdown
            .borrow()
            .accepts_projected(&scope, &proof, user_text)
    );
}

#[test]
fn agent_tasks_and_reports_read_as_markdown() {
    use artisan_api::{EngineId, Item, ItemKind, Status};
    use editor_aqua::conversation::adapter::RowModel;
    use editor_aqua::conversation::fold::{Row, RowKey, TextSection};
    use std::cell::RefCell;

    let opened = OpenThread::new("task-host", "thread");
    let scope = opened.scope();
    let mut app = ui_app(800.0, 600.0);
    let mut markdown = MarkdownOwners::new(SelectionDocumentId(31));
    markdown.rebind(&mut app, Some(scope.clone()));
    let id = ItemId("agent".into());
    let slot = Slot::Text(TextSection::AgentTask);
    let item = Arc::new(Item {
        status: Status::Running,
        kind: ItemKind::Agent {
            name: Some("general-purpose".into()),
            description: "Review the decoder".into(),
            prompt: Some("## Scope\n\nRun `cargo test` before reporting.".into()),
            agent_id: None,
            context_used: None,
            model: None,
            background: true,
            result: Some("### Findings\n\nThe decoder holds.".into()),
            label: None,
            engine: None,
            thread: None,
        },
        ..items::item(id.clone(), opened.thread, 1, user_message(""))
    });
    let report = Slot::Text(TextSection::Report);
    let make_row = |slot, block| {
        realized(RowModel {
            author: Some(EngineId::Claude),
            ..row_model(
                Row::Text {
                    id: id.clone(),
                    slot,
                    block,
                    run: None,
                },
                [Arc::clone(&item)],
            )
        })
    };
    let factory = Rc::new(row_factory(
        vec![make_row(slot, 0), make_row(slot, 1), make_row(report, 0)],
        scope,
        Rc::new(RefCell::new(markdown)),
        true,
        source_ids(60).1,
    ));
    let keys = [
        RowKey::AgentTask(id.clone()),
        RowKey::Text(id.clone(), slot, 1),
        RowKey::Text(id, report, 0),
    ];
    mount(&mut app, row_list(&factory, listed(keys), 30.0, 300.0));
    let painted = painted_texts(&app);
    assert!(
        painted.iter().any(|text| text == "Scope"),
        "the heading is typeset without its marker: {painted:?}"
    );
    assert!(
        painted
            .iter()
            .any(|text| text.contains("cargo test") && !text.contains('`')),
        "inline code is typeset without its backticks: {painted:?}"
    );
    assert!(
        painted.iter().any(|text| text == "Findings"),
        "an opened report is typeset too: {painted:?}"
    );
}

#[test]
fn resource_batch_reuses_identity_until_source_changes() {
    use editor_aqua::conversation::view::resources::{ResourceBatches, ResourceKey};
    let opened = OpenThread::new("batch-host", "thread");
    let scope = opened.scope();
    let mut app = aqua::UiApp::new(SharedFonts::new(), SharedRuns::new());
    let mut batches = ResourceBatches::default();
    batches.rebind(&mut app, Some(scope.clone()));
    let key = ResourceKey::Questions(ItemId("item".into()));
    let view = |text| editor_aqua::sources::ContentView {
        element: Text::new(text).build(),
        selection: Some(source(8, text)),
    };
    let first = batches
        .row(&scope, key.clone(), SelectionDocumentId(3), view("one"))
        .unwrap();
    let second = batches
        .row(&scope, key.clone(), SelectionDocumentId(3), view("one"))
        .unwrap();
    assert!(Arc::ptr_eq(&first.batches[0], &second.batches[0]));
    let third = batches
        .row(&scope, key, SelectionDocumentId(3), view("two"))
        .unwrap();
    assert_ne!(first.batches[0].identity(), third.batches[0].identity());
    assert_eq!(first.batches[0].owner(), third.batches[0].owner());
    assert!(third.batches[0].revision() > first.batches[0].revision());
    batches.clear(&mut app);
}

#[test]
fn projection_expands_only_current_versions_and_preserves_leaf_ordinals() {
    use editor_aqua::conversation::fold::Row;
    use editor_aqua::conversation::projection::{BodyProjection, Projections};
    let id = ItemId("body".into());
    let key = BodyKey {
        item: id.clone(),
        slot: Slot::Prose,
    };
    let base = Row::Text {
        id: id.clone(),
        slot: Slot::Prose,
        block: 0,
        run: None,
    };
    let mut projections = Projections::default();
    assert!(projections.publish(
        key.clone(),
        BodyProjection {
            version: 8,
            revision: 3,
            leaves: 4..7
        }
    ));
    let rows = projections.expand(vec![(base.clone(), 8)], |_, _| None, |_, _| None);
    assert_eq!(rows.len(), 3);
    for (row, block) in rows.iter().map(|entry| &entry.0).zip(4..7) {
        assert_eq!(
            row,
            &Row::Text {
                id: id.clone(),
                slot: Slot::Prose,
                block,
                run: None
            }
        );
    }
    assert!(rows.iter().all(|entry| entry.1 == rows[0].1));
    assert_eq!(
        projections.expand(vec![(base.clone(), 9)], |_, _| None, |_, _| None),
        vec![(base, 9)]
    );
    assert!(!projections.publish(
        key,
        BodyProjection {
            version: 8,
            revision: 3,
            leaves: 4..7
        }
    ));
}

#[test]
fn markdown_edits_keep_equal_funnel_state_and_changed_models_reset_it() {
    use aqua_core::{InputEvent, Key, KeyEvent, Modifiers, NamedKey};
    use editor_aqua::ui::markdown::MarkdownEngine;
    use editor_aqua::ui::markdown::presenter::HeatmapServices;
    use editor_aqua::ui::markdown::visual::BodyTone;
    use editor_aqua::ui::theme::{ArtisanTheme, ThemeMode};

    let opened = OpenThread::new("host", "thread");
    let scope = opened.scope();
    let mut app = ui_app(800.0, 600.0);
    let mut owners = MarkdownOwners::new(SelectionDocumentId(18));
    owners.rebind(&mut app, Some(scope.clone()));
    let engine = MarkdownEngine::new().unwrap();
    let theme = ArtisanTheme::for_mode(ThemeMode::Dark);
    let services = HeatmapServices::new(*theme);
    let key = BodyKey {
        item: ItemId("item".into()),
        slot: Slot::Prose,
    };
    let render = |owners: &mut MarkdownOwners, app: &mut aqua::UiApp, source: &str| {
        let mut next = 100;
        owners
            .render(
                app,
                &scope,
                &engine,
                MarkdownRender {
                    key: key.clone(),
                    id: "item",
                    source,
                    theme,
                    tone: BodyTone::Foreground,
                    order: &[0],
                    glyph: Rc::new(|_, _, _| aqua_ui::element::ui::Box::new().build()),
                    invalidate: Rc::new(|| {}),
                    on_link: Rc::new(|_| {}),
                    on_color: Rc::new(|_| {}),
                    reference_scope: None,
                    heatmap: &services,
                },
                || {
                    next += 1;
                    SelectionSourceId(next)
                },
            )
            .unwrap()
    };
    let original = "Before\n\n```artisan-funnel\n{\"steps\":[{\"label\":\"A\",\"value\":10},{\"label\":\"B\",\"value\":5}]}\n```";
    let updated = original.replace("Before", "Edited paragraph");
    let changed = updated.replace("\"value\":5", "\"value\":3");
    let element = render(&mut owners, &mut app, original);
    mount(&mut app, element);
    assert!(app.focus_key("item-rich-0-funnel-navigation"));
    app.handle_input(&InputEvent::KeyDown(KeyEvent {
        key: Key::Named(NamedKey::End),
        modifiers: Modifiers::NONE,
        repeat: false,
    }));
    let reveal = owners.presenters().next().unwrap().2.funnel_reveals("item");
    assert_eq!(reveal.len(), 1);
    drop(render(&mut owners, &mut app, &updated));
    assert_eq!(
        owners.presenters().next().unwrap().2.funnel_reveals("item"),
        reveal
    );
    drop(render(&mut owners, &mut app, &changed));
    assert!(
        owners
            .presenters()
            .next()
            .unwrap()
            .2
            .funnel_reveals("item")
            .is_empty()
    );
}

#[test]
fn replacing_a_body_releases_only_its_obsolete_selection_sources() {
    let mut opened = OpenThread::new("host", "thread");
    let scope = opened.scope();
    let document = SelectionDocumentId(18);
    let mut app = aqua::UiApp::new(SharedFonts::new(), SharedRuns::new());
    let mut owners = SelectionOwners::new(document);
    owners.rebind(&mut app, Some(scope.clone()));
    let first = BodyKey {
        item: ItemId("first".into()),
        slot: Slot::Prose,
    };
    let second = BodyKey {
        item: ItemId("second".into()),
        slot: Slot::Prose,
    };
    let prepared = |source| PreparedElement {
        element: Text::new("content").build(),
        sources: vec![source],
    };
    owners
        .mount(
            &mut app,
            &scope,
            first.clone(),
            prepared(source(1, "first")),
        )
        .unwrap();
    owners
        .mount(
            &mut app,
            &scope,
            second.clone(),
            prepared(source(2, "second")),
        )
        .unwrap();
    owners
        .mount(&mut app, &scope, first, prepared(source(3, "replacement")))
        .unwrap();
    assert!(!app.unregister_selection_source_in(document, SelectionSourceId(1)));
    assert!(app.unregister_selection_source_in(document, SelectionSourceId(3)));
    owners.retain(&mut app, |key| *key == second);
    assert!(app.unregister_selection_source_in(document, SelectionSourceId(2)));
    connect(&mut opened.core, &opened.host);
    assert!(!scope.current(&opened.conversation, &opened.core));
    owners.rebind(&mut app, None);
}

#[test]
fn running_inline_reply_without_live_stream_realizes_from_its_text() {
    use artisan_api::{Event, Item, Status};
    use editor_aqua::conversation::adapter::row_model;
    use editor_aqua::conversation::fold::{Row, RowKey};
    use editor_aqua::conversation::view::reveal::MountedReveals;
    use editor_aqua::core::runtime::runtime;
    use editor_aqua::core::{CoreEvent, HostEvent, Screen};
    use std::cell::RefCell;
    use std::time::Duration;

    let directory = tempfile::tempdir().unwrap();
    let mut core = test_core(directory.path());
    let config = remote_host("inline-host");
    connect(&mut core, &config);
    core.host_mut(&config.id).unwrap().test_stop_connection();
    let thread = ThreadId("inline-thread".into());
    let id = ItemId("inline-reply".into());
    core.navigate(Screen::Thread {
        host: config.id.clone(),
        thread: thread.clone(),
    });
    let mut conversation = Conversation::new(&core);
    conversation.open_thread(&mut core, config.id.clone(), thread.clone());
    let sender = core.host(&config.id).unwrap().test_transition_sender();
    runtime().block_on(sender.event(Event::Item {
        item: Item {
            status: Status::Running,
            ..items::item(
                id.clone(),
                thread,
                1,
                assistant_reply("streamed words so far"),
            )
        },
    }));
    wait_until("the item event", Duration::from_secs(15), || {
        let mut delivered = false;
        core.drain_with(|core, event| {
            delivered |= matches!(&event, CoreEvent::Host { event: HostEvent::Item(item), .. } if item.id == id);
            conversation.core_event(core, &event, false);
        });
        delivered
    });
    let index = conversation
        .rows
        .iter()
        .position(|(row, _)| matches!(row, Row::Text { id: row_id, slot: Slot::Prose, .. } if row_id == &id))
        .unwrap();
    let model = row_model(&conversation, index, None).unwrap();
    let scope = PresentationScope::capture(&conversation, &core).unwrap();
    let mut app = ui_app(800.0, 600.0);
    let markdown = Rc::new(RefCell::new(MarkdownOwners::new(SelectionDocumentId(805))));
    markdown.borrow_mut().rebind(&mut app, Some(scope.clone()));
    let owners = Rc::new(RefCell::new(MountedReveals::default()));
    let row_cache = Rc::new(RefCell::new(
        editor_aqua::conversation::view::row_cache::RowCache::default(),
    ));
    let factory = row_factory(
        vec![realized(model)],
        scope,
        markdown,
        true,
        source_ids(1000).1,
    )
    .with_row_cache(row_cache)
    .with_reveals(&conversation, &core, owners, false)
    .unwrap();
    factory.realize(0, &RowKey::Item(id)).unwrap();
}

#[test]
fn stored_reply_that_is_still_loading_does_not_request_reveal_frames() {
    use artisan_api::{ContentRef, Event, Item, ItemKind, Phase, Status, TextContent};
    use editor_aqua::conversation::adapter::row_model;
    use editor_aqua::conversation::fold::{Row, RowKey};
    use editor_aqua::conversation::stored::StoredConversation;
    use editor_aqua::conversation::view::reveal::MountedReveals;
    use editor_aqua::core::runtime::runtime;
    use editor_aqua::core::{CoreEvent, HostEvent, Screen};
    use std::cell::RefCell;
    use std::time::Duration;

    let directory = tempfile::tempdir().unwrap();
    let mut core = test_core(directory.path());
    let config = remote_host("stored-host");
    connect(&mut core, &config);
    core.host_mut(&config.id).unwrap().test_stop_connection();
    let thread = ThreadId("stored-thread".into());
    let id = ItemId("stored-reply".into());
    core.navigate(Screen::Thread {
        host: config.id.clone(),
        thread: thread.clone(),
    });
    let mut conversation = Conversation::new(&core);
    conversation.open_thread(&mut core, config.id.clone(), thread.clone());
    let sender = core.host(&config.id).unwrap().test_transition_sender();
    runtime().block_on(sender.event(Event::Item {
        item: Item {
            status: Status::Done,
            ..items::item(
                id.clone(),
                thread,
                1,
                ItemKind::AssistantMessage {
                    text: TextContent::Stored(ContentRef::from_text("a stored reply")),
                    phase: Phase::Final,
                },
            )
        },
    }));
    wait_until("the item event", Duration::from_secs(15), || {
        let mut delivered = false;
        core.drain_with(|core, event| {
            delivered |= matches!(&event, CoreEvent::Host { event: HostEvent::Item(item), .. } if item.id == id);
            conversation.core_event(core, &event, false);
        });
        delivered
    });
    let index = conversation
        .rows
        .iter()
        .position(|(row, _)| matches!(row, Row::Text { id: row_id, slot: Slot::Prose, .. } if row_id == &id))
        .unwrap();
    let model = row_model(&conversation, index, None).unwrap();
    let scope = PresentationScope::capture(&conversation, &core).unwrap();
    let provider = core.source_provider(&scope.host, scope.instance).unwrap();
    let mut app = ui_app(800.0, 600.0);
    let markdown = Rc::new(RefCell::new(MarkdownOwners::new(SelectionDocumentId(806))));
    markdown.borrow_mut().rebind(&mut app, Some(scope.clone()));
    let owners = Rc::new(RefCell::new(MountedReveals::default()));
    let factory = Rc::new(
        row_factory(
            vec![realized(model)],
            scope,
            markdown,
            false,
            source_ids(1100).1,
        )
        .with_stored(
            Rc::new(RefCell::new(StoredConversation::default())),
            provider,
        )
        .unwrap()
        .with_reveals(&conversation, &core, Rc::clone(&owners), false)
        .unwrap(),
    );
    let row_key = RowKey::Item(id);
    mount(
        &mut app,
        row_list(&factory, listed([row_key.clone()]), 30.0, 300.0),
    );
    app.build_frame().unwrap();
    assert!(
        painted_texts(&app)
            .iter()
            .any(|text| text.contains("Loading formatted content"))
    );
    let sync = factory.sync_reveals(&mut app, &mut conversation, &core);
    assert!(
        !sync.animating,
        "a reply waiting for its stored body has nothing to reveal and must not keep requesting frames"
    );
    assert_eq!(owners.borrow().ready(&row_key), Some(false));
}
