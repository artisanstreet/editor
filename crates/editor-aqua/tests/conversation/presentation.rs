use aqua_text::document::{SelectionDocumentId, SelectionSource, SelectionSourceId};
use aqua_text::{SharedFonts, SharedRuns};
use aqua_ui::element::Text;
use artisan_api::{ItemId, ThreadId};
use editor_aqua::conversation::controller::Conversation;
use editor_aqua::conversation::fold::Slot;
use editor_aqua::conversation::presentation::{
    BodyKey, MarkdownOwners, MarkdownRender, PreparedElement, PresentationScope, SelectionOwners,
};
use editor_aqua::core::{Core, EditorFiles, ForgeClient, HostConfig, HostId, HostKind};
use std::rc::Rc;
use std::sync::Arc;

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
        active: active.clone(),
    };
    let dir = tempfile::tempdir().unwrap();
    let core = Core::for_tests(EditorFiles {
        dir: dir.path().to_owned(),
    })
    .unwrap();
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
fn mounted_fresh_work_rows_enter_in_admission_order_across_factory_rebuilds() {
    use artisan_api::{Item, ItemKind, RunId, Status, ToolKind};
    use editor_aqua::conversation::adapter::RowModel;
    use editor_aqua::conversation::fold::{Row, RowKey, RunSummary};
    use editor_aqua::conversation::phase::Phase;
    use editor_aqua::conversation::thread::ItemUi;
    use editor_aqua::conversation::view::factory::{RealizedRow, RowFactory};
    use editor_aqua::conversation::view::motion::RowMotion;
    use editor_aqua::conversation::view::status::StatusShimmers;
    use editor_aqua::ui::theme::{ArtisanTheme, ThemeMode};
    use std::cell::RefCell;
    use std::collections::HashMap;
    use std::time::{Duration, Instant};

    let dir = tempfile::tempdir().unwrap();
    let mut core = Core::for_tests(EditorFiles {
        dir: dir.path().to_owned(),
    })
    .unwrap();
    let config = HostConfig {
        id: HostId("work-host".into()),
        name: "Forge".into(),
        address: "127.0.0.1:1".into(),
        fingerprint: "0".repeat(64),
        kind: HostKind::Remote,
    };
    let client = ForgeClient::new(
        &config.address,
        &config.fingerprint,
        core.identity.as_ref().unwrap(),
    )
    .unwrap();
    core.add_test_host(&config, client);
    let thread = ThreadId("work-thread".into());
    let mut conversation = Conversation::new(&core);
    conversation.open_thread(&mut core, config.id.clone(), thread.clone());
    let scope = PresentationScope::capture(&conversation, &core).unwrap();
    let mut app = aqua::UiApp::new(SharedFonts::new(), SharedRuns::new());
    app.set_viewport(800.0, 600.0, 1.0);
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
                    id: id.clone(),
                    thread_id: thread.clone(),
                    position: u64::try_from(index).unwrap() + 1,
                    seq: u64::try_from(index).unwrap() + 1,
                    run_id: Some(RunId("run".into())),
                    parent_id: None,
                    status: Status::Running,
                    created_at: 0,
                    updated_at: 0,
                    kind: ItemKind::Tool {
                        tool: ToolKind::Read {
                            path: "src/main.rs".into(),
                            range: None,
                        },
                        title: None,
                        output: None,
                        exit_code: None,
                        approval: None,
                    },
                });
                RealizedRow {
                    model: RowModel {
                        row: Row::WorkEntry {
                            id: id.clone(),
                            run: Some(RunId("run".into())),
                        },
                        version: 1,
                        items: HashMap::from([(id.clone(), item)]),
                        live: HashMap::new(),
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
                        }),
                        author: None,
                        gap: 8.0,
                        end: 0.0,
                    },
                    disclosure: Default::default(),
                    held: false,
                }
            })
            .collect()
    };
    let factory = |at| {
        Rc::new(
            RowFactory::new(
                rows(),
                scope.clone(),
                markdown.clone(),
                Rc::new(RefCell::new(StatusShimmers::default())),
                *ArtisanTheme::for_mode(ThemeMode::Dark),
                false,
                Rc::new(|| SelectionSourceId(9000)),
                Rc::new(|| {}),
                Rc::new(|_| {}),
                Rc::new(|_| {}),
                Rc::new(|_| {}),
            )
            .with_dispatch(Rc::new(|_| {}))
            .with_admission(conversation.reveal.clone())
            .with_row_cache(row_cache.clone())
            .with_work(motion.clone(), HashMap::new(), at, 0.0, 0),
        )
    };
    let first = factory(now);
    let callback = first.callback();
    let row_keys = keys.clone();
    let list = aqua_ui::collection::List::new()
        .items(vec![0_usize, 1])
        .key(move |index: &usize| format!("{:?}", row_keys[*index]))
        .try_batched_row({
            let keys = keys.clone();
            move |index: &usize| callback(*index, &keys[*index])
        })
        .virtualize()
        .estimated_row_height(aqua_ui::style::Length::Px(30.0))
        .style(
            aqua_ui::style::Style::new()
                .width(aqua_ui::style::Length::Px(600.0))
                .height(aqua_ui::style::Length::Px(300.0)),
        )
        .build();
    app.mount(|_| list).unwrap();
    app.layout().unwrap();
    app.build_frame().unwrap();
    assert_eq!(
        conversation.reveal.borrow().frontier_key(),
        Some(keys[0].clone())
    );
    assert!(first.sync_after_layout(&mut app));

    let next = factory(now + Duration::from_millis(250));
    next.realize(0, &keys[0]).unwrap();
    next.realize(1, &keys[1]).unwrap();
    assert_eq!(
        conversation.reveal.borrow().frontier_key(),
        Some(keys[1].clone())
    );
    core.remove_host(&config.id).unwrap();
    let replacement = ForgeClient::new(
        &config.address,
        &config.fingerprint,
        core.identity.as_ref().unwrap(),
    )
    .unwrap();
    core.add_test_host(&config, replacement);
    conversation.open_thread(&mut core, config.id.clone(), thread.clone());
    let replacement_scope = PresentationScope::capture(&conversation, &core).unwrap();
    let _replacement_factory = RowFactory::new(
        Vec::new(),
        replacement_scope,
        markdown.clone(),
        Rc::new(RefCell::new(StatusShimmers::default())),
        *ArtisanTheme::for_mode(ThemeMode::Dark),
        false,
        Rc::new(|| SelectionSourceId(9001)),
        Rc::new(|| {}),
        Rc::new(|_| {}),
        Rc::new(|_| {}),
        Rc::new(|_| {}),
    )
    .with_row_cache(row_cache.clone())
    .with_work(motion.clone(), HashMap::new(), now, 0.0, 0);
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

fn exercise_mounted_live_reveal(stored_saved: bool) {
    use artisan_api::{
        Event, Item, ItemKind, LiveCoverage, LiveTarget, Phase, Status, TextContent,
    };
    use editor_aqua::conversation::adapter::row_model;
    use editor_aqua::conversation::fold::{Row, RowKey};
    use editor_aqua::conversation::live::LiveBinding;
    use editor_aqua::conversation::view::factory::{RealizedRow, RowFactory};
    use editor_aqua::conversation::view::reveal::MountedReveals;
    use editor_aqua::conversation::view::status::StatusShimmers;
    use editor_aqua::conversation::view::transcript::TranscriptObservations;
    use editor_aqua::core::runtime::runtime;
    use editor_aqua::core::{CoreEvent, HostEvent, Screen};
    use editor_aqua::ui::theme::{ArtisanTheme, ThemeMode};
    use std::cell::{Cell, RefCell};
    use std::time::{Duration, Instant};

    let directory = tempfile::tempdir().unwrap();
    let mut core = Core::for_tests(EditorFiles {
        dir: directory.path().to_owned(),
    })
    .unwrap();
    let config = HostConfig {
        id: HostId("live-host".into()),
        name: "Forge".into(),
        address: "127.0.0.1:1".into(),
        fingerprint: "0".repeat(64),
        kind: HostKind::Remote,
    };
    let client = |core: &Core| {
        ForgeClient::new(
            &config.address,
            &config.fingerprint,
            core.identity.as_ref().unwrap(),
        )
        .unwrap()
    };
    core.add_test_host(&config, client(&core));
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
        let deadline = Instant::now() + Duration::from_secs(15);
        let mut delivered = false;
        while !delivered {
            core.drain_with(|core, event| {
                delivered |= matches!(&event, CoreEvent::Host { host, event: HostEvent::Live { thread_id, target: actual } }
                    if host == &config.id && thread_id == &thread && actual == &target);
                conversation.core_event(core, &event, false);
            });
            assert!(
                Instant::now() < deadline,
                "matching live event was not delivered"
            );
        }
    };
    runtime().block_on(async {
        sender
            .event(Event::Item {
                item: Item {
                    id: id.clone(),
                    thread_id: thread.clone(),
                    position: 1,
                    seq: 1,
                    run_id: None,
                    parent_id: None,
                    status: Status::Running,
                    created_at: 0,
                    updated_at: 0,
                    kind: ItemKind::AssistantMessage {
                        text: if stored_saved {
                            TextContent::Stored(artisan_api::ContentRef::from_text("saved reply"))
                        } else {
                            TextContent::Inline(String::new())
                        },
                        phase: Phase::Final,
                    },
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
    let mut app = aqua::UiApp::new(SharedFonts::new(), SharedRuns::new());
    app.set_viewport(800.0, 600.0, 1.0);
    let markdown = Rc::new(RefCell::new(MarkdownOwners::new(SelectionDocumentId(801))));
    markdown.borrow_mut().rebind(&mut app, Some(scope.clone()));
    let owners = Rc::new(RefCell::new(MountedReveals::default()));
    let row_cache = Rc::new(RefCell::new(
        editor_aqua::conversation::view::row_cache::RowCache::default(),
    ));
    let observations = TranscriptObservations::default();
    let next = Rc::new(Cell::new(900));
    let allocate = next.clone();
    let factory = Rc::new(
        RowFactory::new(
            vec![RealizedRow {
                model,
                disclosure: Default::default(),
                held: false,
            }],
            scope.clone(),
            markdown.clone(),
            Rc::new(RefCell::new(StatusShimmers::default())),
            *ArtisanTheme::for_mode(ThemeMode::Dark),
            true,
            Rc::new(move || {
                let id = allocate.get() + 1;
                allocate.set(id);
                SelectionSourceId(id)
            }),
            Rc::new(|| {}),
            Rc::new(|_| {}),
            Rc::new(|_| {}),
            Rc::new(|_| {}),
        )
        .with_observations(observations.clone())
        .with_row_cache(row_cache.clone())
        .with_reveals(&conversation, &core, owners.clone(), Instant::now(), false)
        .unwrap(),
    );
    let row_key = RowKey::Item(id.clone());
    let captured = row_key.clone();
    let build = factory.callback();
    let list = aqua_ui::collection::List::new()
        .items(vec![0_usize])
        .key(move |_: &usize| format!("{captured:?}"))
        .try_batched_row(move |index: &usize| {
            build(*index, &RowKey::Item(ItemId("live-reply".into())))
        })
        .virtualize()
        .estimated_row_height(aqua_ui::style::Length::Px(30.0))
        .style(
            aqua_ui::style::Style::new()
                .width(aqua_ui::style::Length::Px(600.0))
                .height(aqua_ui::style::Length::Px(300.0)),
        )
        .build();
    app.mount(|_| list).unwrap();
    app.layout().unwrap();
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
    let replacement = RowFactory::new(
        vec![RealizedRow {
            model: row_model(&conversation, index, None).unwrap(),
            disclosure: Default::default(),
            held: false,
        }],
        scope.clone(),
        markdown.clone(),
        Rc::new(RefCell::new(StatusShimmers::default())),
        *ArtisanTheme::for_mode(ThemeMode::Dark),
        true,
        Rc::new(|| SelectionSourceId(9999)),
        Rc::new(|| {}),
        Rc::new(|_| {}),
        Rc::new(|_| {}),
        Rc::new(|_| {}),
    )
    .with_row_cache(row_cache.clone())
    .with_reveals(&conversation, &core, owners.clone(), Instant::now(), false)
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
        RowFactory::new(
            vec![],
            scope.clone(),
            markdown.clone(),
            Rc::new(RefCell::new(StatusShimmers::default())),
            *ArtisanTheme::for_mode(ThemeMode::Dark),
            true,
            Rc::new(|| SelectionSourceId(9999)),
            Rc::new(|| {}),
            Rc::new(|_| {}),
            Rc::new(|_| {}),
            Rc::new(|_| {}),
        )
        .with_lazy_rows(&conversation, &core, None)
        .unwrap()
        .with_observations(complete_observations.clone())
        .with_row_cache(row_cache.clone())
        .with_reveals(&conversation, &core, owners.clone(), Instant::now(), false)
        .unwrap(),
    );
    let mut complete_app = aqua::UiApp::new(SharedFonts::new(), SharedRuns::new());
    complete_app.set_viewport(800.0, 600.0, 1.0);
    let complete_key = row_key.clone();
    let callback_key = row_key.clone();
    let callback = complete_factory.callback();
    let list = aqua_ui::collection::List::new()
        .items(vec![complete_index])
        .key(move |_: &usize| format!("{complete_key:?}"))
        .try_batched_row(move |index: &usize| callback(*index, &callback_key))
        .virtualize()
        .estimated_row_height(aqua_ui::style::Length::Px(30.0))
        .style(
            aqua_ui::style::Style::new()
                .width(aqua_ui::style::Length::Px(600.0))
                .height(aqua_ui::style::Length::Px(300.0)),
        )
        .build();
    complete_app.mount(|_| list).unwrap();
    complete_app.layout().unwrap();
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
    let key_hash = aqua_ui::tree::hash_key(aqua_ui::element::Key::from(format!("{row_key:?}")));
    let node = complete_app
        .tree()
        .traverse()
        .into_iter()
        .find(|node| {
            complete_app
                .tree()
                .get(*node)
                .is_some_and(|record| record.key == Some(key_hash))
        })
        .unwrap();
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
    core.add_test_host(&config, client(&core));
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
fn realized_factory_rows_share_registered_sources_and_stale_factory_is_rejected() {
    use artisan_api::{Item, ItemKind, Phase, Status, TextContent};
    use editor_aqua::conversation::adapter::RowModel;
    use editor_aqua::conversation::disclosure::RowDisclosure;
    use editor_aqua::conversation::fold::{Row, RowKey};
    use editor_aqua::conversation::view::factory::{RealizedRow, RowFactory};
    use editor_aqua::conversation::view::status::StatusShimmers;
    use editor_aqua::ui::theme::{ArtisanTheme, ThemeMode};
    use std::cell::{Cell, RefCell};
    use std::collections::HashMap;

    let dir = tempfile::tempdir().unwrap();
    let mut core = Core::for_tests(EditorFiles {
        dir: dir.path().to_owned(),
    })
    .unwrap();
    let config = HostConfig {
        id: HostId("factory-host".into()),
        name: "Forge".into(),
        address: "127.0.0.1:1".into(),
        fingerprint: "0".repeat(64),
        kind: HostKind::Remote,
    };
    let client = ForgeClient::new(
        &config.address,
        &config.fingerprint,
        core.identity.as_ref().unwrap(),
    )
    .unwrap();
    core.add_test_host(&config, client);
    let thread = ThreadId("thread".into());
    let mut conversation = Conversation::new(&core);
    conversation.open_thread(&mut core, config.id, thread.clone());
    let scope = PresentationScope::capture(&conversation, &core).unwrap();
    let mut app = aqua::UiApp::new(SharedFonts::new(), SharedRuns::new());
    app.set_viewport(800.0, 600.0, 1.0);
    let document = SelectionDocumentId(28);
    let mut markdown = editor_aqua::conversation::presentation::MarkdownOwners::new(document);
    markdown.rebind(&mut app, Some(scope.clone()));
    let id = ItemId("reply".into());
    let text = "First paragraph.\n\nSecond paragraph.";
    let key = BodyKey {
        item: id.clone(),
        slot: Slot::Prose,
    };
    let item = Arc::new(Item {
        id: id.clone(),
        thread_id: thread,
        position: 1,
        seq: 1,
        run_id: None,
        parent_id: None,
        status: Status::Done,
        created_at: 0,
        updated_at: 0,
        kind: ItemKind::AssistantMessage {
            text: TextContent::Inline(text.into()),
            phase: Phase::Final,
        },
    });
    let make_row = |block| RealizedRow {
        model: RowModel {
            row: Row::Text {
                id: id.clone(),
                slot: Slot::Prose,
                block,
                run: None,
            },
            version: 1,
            items: HashMap::from([(id.clone(), Arc::clone(&item))]),
            live: HashMap::new(),
            ui: HashMap::new(),
            run: None,
            author: None,
            gap: 0.0,
            end: 0.0,
        },
        disclosure: RowDisclosure::default(),
        held: false,
    };
    let markdown = Rc::new(RefCell::new(markdown));
    let next = Rc::new(Cell::new(40));
    let allocator = Rc::clone(&next);
    let factory = Rc::new(RowFactory::new(
        vec![make_row(0), make_row(1)],
        scope.clone(),
        markdown.clone(),
        Rc::new(RefCell::new(StatusShimmers::default())),
        *ArtisanTheme::for_mode(ThemeMode::Dark),
        true,
        Rc::new(move || {
            let value = allocator.get() + 1;
            allocator.set(value);
            SelectionSourceId(value)
        }),
        Rc::new(|| {}),
        Rc::new(|_| {}),
        Rc::new(|_| {}),
        Rc::new(|_| {}),
    ));
    let keys = [RowKey::Item(id.clone()), RowKey::Text(id, Slot::Prose, 1)];
    let callback = factory.callback();
    let list_keys = keys.clone();
    let list = aqua_ui::collection::List::new()
        .items(vec![0_usize, 1])
        .key(|index: &usize| format!("test-row-{index}"))
        .try_batched_row(move |index: &usize| callback(*index, &list_keys[*index]))
        .virtualize()
        .estimated_row_height(aqua_ui::style::Length::Px(30.0))
        .style(
            aqua_ui::style::Style::new()
                .width(aqua_ui::style::Length::Px(600.0))
                .height(aqua_ui::style::Length::Px(300.0)),
        )
        .build();
    app.mount(|_| list).unwrap();
    app.layout().unwrap();
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
        .prepare_on_realization(&scope, key.clone(), "reply", &engine, "replacement", 2)
        .unwrap();
    assert!(!markdown.borrow().accepts_projected(&scope, &proof, text));
    factory.dispose();
    assert!(factory.realize(0, &keys[0]).is_err());

    let activations = Rc::new(Cell::new(0));
    let activated = activations.clone();
    let gap_factory = RowFactory::new(
        vec![RealizedRow {
            model: RowModel {
                row: Row::Gap,
                version: 3,
                items: HashMap::new(),
                live: HashMap::new(),
                ui: HashMap::new(),
                run: None,
                author: None,
                gap: 0.0,
                end: 0.0,
            },
            disclosure: RowDisclosure::default(),
            held: false,
        }],
        scope.clone(),
        markdown.clone(),
        Rc::new(RefCell::new(StatusShimmers::default())),
        *ArtisanTheme::for_mode(ThemeMode::Dark),
        true,
        Rc::new(|| SelectionSourceId(999)),
        Rc::new(|| {}),
        Rc::new(|_| {}),
        Rc::new(|_| {}),
        Rc::new(|_| {}),
    )
    .with_dispatch(Rc::new(move |_| activated.set(activated.get() + 1)))
    .with_observations(
        editor_aqua::conversation::view::transcript::TranscriptObservations::default(),
    );
    let gap = gap_factory.realize(0, &RowKey::Gap).unwrap().element;
    let mut gap_app = aqua::UiApp::new(SharedFonts::new(), SharedRuns::new());
    gap_app.set_viewport(800.0, 600.0, 1.0);
    gap_app.mount(|_| gap).unwrap();
    gap_app.layout().unwrap();
    gap_app.build_frame().unwrap();
    let key = aqua_ui::tree::hash_key(aqua_ui::element::Key::from("load-newer"));
    let node = gap_app
        .tree()
        .traverse()
        .into_iter()
        .find(|node| {
            gap_app
                .tree()
                .get(*node)
                .is_some_and(|record| record.key == Some(key))
        })
        .unwrap();
    let bounds = gap_app.tree().visual_rect(node).unwrap();
    let pointer = aqua_core::PointerEvent {
        contact: 1,
        kind: aqua_core::PointerKind::Mouse,
        position: aqua_core::Point::new(bounds.x + bounds.w / 2.0, bounds.y + bounds.h / 2.0),
        buttons: 1,
    };
    aqua_ui::events::dispatch_input(
        gap_app.tree_mut(),
        &aqua_core::InputEvent::PointerDown(pointer, aqua_core::PointerButton::Primary),
    );
    aqua_ui::events::dispatch_input(
        gap_app.tree_mut(),
        &aqua_core::InputEvent::PointerUp(
            aqua_core::PointerEvent {
                buttons: 0,
                ..pointer
            },
            aqua_core::PointerButton::Primary,
        ),
    );
    assert_eq!(activations.get(), 1);

    let user_id = ItemId("user-message".into());
    let user_text = "First user paragraph.\n\nSecond user paragraph.";
    let user_item = Arc::new(Item {
        id: user_id.clone(),
        thread_id: item.thread_id.clone(),
        position: 2,
        seq: 2,
        run_id: None,
        parent_id: None,
        status: Status::Done,
        created_at: 0,
        updated_at: 0,
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
    });
    let user_slot = Slot::Text(editor_aqua::conversation::fold::TextSection::User);
    let user_row = |block| RealizedRow {
        model: RowModel {
            row: Row::Text {
                id: user_id.clone(),
                slot: user_slot,
                block,
                run: None,
            },
            version: 2,
            items: HashMap::from([(user_id.clone(), Arc::clone(&user_item))]),
            live: HashMap::new(),
            ui: HashMap::new(),
            run: None,
            author: None,
            gap: 0.0,
            end: 0.0,
        },
        disclosure: RowDisclosure::default(),
        held: false,
    };
    let user_ids = Rc::new(Cell::new(100));
    let allocated = user_ids.clone();
    let blob_events = Rc::new(RefCell::new(Vec::new()));
    let emitted = blob_events.clone();
    let images = core.host(&scope.host).unwrap().blob_images();
    let user_factory = Rc::new(
        RowFactory::new(
            vec![user_row(0), user_row(1)],
            scope.clone(),
            markdown.clone(),
            Rc::new(RefCell::new(StatusShimmers::default())),
            *ArtisanTheme::for_mode(ThemeMode::Dark),
            true,
            Rc::new(move || {
                let id = allocated.get() + 1;
                allocated.set(id);
                SelectionSourceId(id)
            }),
            Rc::new(|| {}),
            Rc::new(|_| {}),
            Rc::new(|_| {}),
            Rc::new(|_| {}),
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
    let callback = user_factory.callback();
    let list = aqua_ui::collection::List::new()
        .items(vec![0_usize, 1])
        .key(|index: &usize| format!("test-user-row-{index}"))
        .try_batched_row(move |index: &usize| callback(*index, &user_keys[*index]))
        .virtualize()
        .estimated_row_height(aqua_ui::style::Length::Px(30.0))
        .style(
            aqua_ui::style::Style::new()
                .width(aqua_ui::style::Length::Px(600.0))
                .height(aqua_ui::style::Length::Px(300.0)),
        )
        .build();
    let mut user_app = aqua::UiApp::new(SharedFonts::new(), SharedRuns::new());
    user_app.set_viewport(800.0, 600.0, 1.0);
    user_app.mount(|_| list).unwrap();
    user_app.layout().unwrap();
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
    let emitted = empty_events.clone();
    let empty_factory = Rc::new(
        RowFactory::new(
            vec![empty_row],
            scope.clone(),
            markdown.clone(),
            Rc::new(RefCell::new(StatusShimmers::default())),
            *ArtisanTheme::for_mode(ThemeMode::Dark),
            true,
            Rc::new(|| SelectionSourceId(999)),
            Rc::new(|| {}),
            Rc::new(|_| {}),
            Rc::new(|_| {}),
            Rc::new(|_| {}),
        )
        .with_blob_images(
            core.host(&scope.host).unwrap().blob_images(),
            Rc::new(move |id, seq, action| {
                emitted.borrow_mut().push((id, seq, action));
            }),
        ),
    );
    let callback = empty_factory.callback();
    let empty_key = RowKey::Item(user_id.clone());
    let list = aqua_ui::collection::List::new()
        .items(vec![0_usize])
        .key(|_: &usize| "empty-user-image")
        .try_batched_row(move |index: &usize| callback(*index, &empty_key))
        .virtualize()
        .estimated_row_height(aqua_ui::style::Length::Px(120.0))
        .style(
            aqua_ui::style::Style::new()
                .width(aqua_ui::style::Length::Px(600.0))
                .height(aqua_ui::style::Length::Px(300.0)),
        )
        .build();
    let mut empty_app = aqua::UiApp::new(SharedFonts::new(), SharedRuns::new());
    empty_app.set_viewport(800.0, 600.0, 1.0);
    empty_app.mount(|_| list).unwrap();
    empty_app.layout().unwrap();
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
    use editor_aqua::conversation::disclosure::RowDisclosure;
    use editor_aqua::conversation::fold::{Row, RowKey, TextSection};
    use editor_aqua::conversation::view::factory::{RealizedRow, RowFactory};
    use editor_aqua::conversation::view::status::StatusShimmers;
    use editor_aqua::ui::theme::{ArtisanTheme, ThemeMode};
    use std::cell::{Cell, RefCell};
    use std::collections::HashMap;

    let dir = tempfile::tempdir().unwrap();
    let mut core = Core::for_tests(EditorFiles {
        dir: dir.path().to_owned(),
    })
    .unwrap();
    let config = HostConfig {
        id: HostId("task-host".into()),
        name: "Forge".into(),
        address: "127.0.0.1:1".into(),
        fingerprint: "0".repeat(64),
        kind: HostKind::Remote,
    };
    let client = ForgeClient::new(
        &config.address,
        &config.fingerprint,
        core.identity.as_ref().unwrap(),
    )
    .unwrap();
    core.add_test_host(&config, client);
    let thread = ThreadId("thread".into());
    let mut conversation = Conversation::new(&core);
    conversation.open_thread(&mut core, config.id, thread.clone());
    let scope = PresentationScope::capture(&conversation, &core).unwrap();
    let mut app = aqua::UiApp::new(SharedFonts::new(), SharedRuns::new());
    app.set_viewport(800.0, 600.0, 1.0);
    let mut markdown = MarkdownOwners::new(SelectionDocumentId(31));
    markdown.rebind(&mut app, Some(scope.clone()));
    let id = ItemId("agent".into());
    let slot = Slot::Text(TextSection::AgentTask);
    let item = Arc::new(Item {
        id: id.clone(),
        thread_id: thread,
        position: 1,
        seq: 1,
        run_id: None,
        parent_id: None,
        status: Status::Running,
        created_at: 0,
        updated_at: 0,
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
    });
    let report = Slot::Text(TextSection::Report);
    let make_row = |slot, block| RealizedRow {
        model: RowModel {
            row: Row::Text {
                id: id.clone(),
                slot,
                block,
                run: None,
            },
            version: 1,
            items: HashMap::from([(id.clone(), Arc::clone(&item))]),
            live: HashMap::new(),
            ui: HashMap::new(),
            run: None,
            author: Some(EngineId::Claude),
            gap: 0.0,
            end: 0.0,
        },
        disclosure: RowDisclosure::default(),
        held: false,
    };
    let next = Rc::new(Cell::new(60));
    let allocator = Rc::clone(&next);
    let factory = Rc::new(RowFactory::new(
        vec![make_row(slot, 0), make_row(slot, 1), make_row(report, 0)],
        scope,
        Rc::new(RefCell::new(markdown)),
        Rc::new(RefCell::new(StatusShimmers::default())),
        *ArtisanTheme::for_mode(ThemeMode::Dark),
        true,
        Rc::new(move || {
            let value = allocator.get() + 1;
            allocator.set(value);
            SelectionSourceId(value)
        }),
        Rc::new(|| {}),
        Rc::new(|_| {}),
        Rc::new(|_| {}),
        Rc::new(|_| {}),
    ));
    let keys = [
        RowKey::AgentTask(id.clone()),
        RowKey::Text(id.clone(), slot, 1),
        RowKey::Text(id, report, 0),
    ];
    let callback = factory.callback();
    let list_keys = keys.clone();
    let list = aqua_ui::collection::List::new()
        .items(vec![0_usize, 1, 2])
        .key(|index: &usize| format!("task-row-{index}"))
        .try_batched_row(move |index: &usize| callback(*index, &list_keys[*index]))
        .virtualize()
        .estimated_row_height(aqua_ui::style::Length::Px(30.0))
        .style(
            aqua_ui::style::Style::new()
                .width(aqua_ui::style::Length::Px(600.0))
                .height(aqua_ui::style::Length::Px(300.0)),
        )
        .build();
    app.mount(|_| list).unwrap();
    app.layout().unwrap();
    let painted: Vec<String> = app
        .tree()
        .traverse()
        .into_iter()
        .filter_map(|node| match &app.tree().get(node)?.kind {
            aqua_ui::tree::NodeKind::Text(content) => Some(content.text().to_owned()),
            _ => None,
        })
        .collect();
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
    let dir = tempfile::tempdir().unwrap();
    let mut core = Core::for_tests(EditorFiles {
        dir: dir.path().to_owned(),
    })
    .unwrap();
    let config = HostConfig {
        id: HostId("batch-host".into()),
        name: "Forge".into(),
        address: "127.0.0.1:1".into(),
        fingerprint: "0".repeat(64),
        kind: HostKind::Remote,
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
    let scope = PresentationScope::capture(&conversation, &core).unwrap();
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

    let dir = tempfile::tempdir().unwrap();
    let mut core = Core::for_tests(EditorFiles {
        dir: dir.path().to_owned(),
    })
    .unwrap();
    let config = HostConfig {
        id: HostId("host".into()),
        name: "Forge".into(),
        address: "127.0.0.1:1".into(),
        fingerprint: "0".repeat(64),
        kind: HostKind::Remote,
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
    let scope = PresentationScope::capture(&conversation, &core).unwrap();
    let mut app = aqua::UiApp::new(SharedFonts::new(), SharedRuns::new());
    app.set_viewport(800.0, 600.0, 1.0);
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
                    theme: &theme,
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
    app.mount(|_| element).unwrap();
    app.layout().unwrap();
    assert!(app.focus_key("item-rich-0-funnel-navigation"));
    app.handle_input(&InputEvent::KeyDown(KeyEvent {
        key: Key::Named(NamedKey::End),
        modifiers: Modifiers::NONE,
        repeat: false,
    }));
    let reveal = owners.presenters().next().unwrap().2.funnel_reveals("item");
    assert_eq!(reveal.len(), 1);
    let _ = render(&mut owners, &mut app, &updated);
    assert_eq!(
        owners.presenters().next().unwrap().2.funnel_reveals("item"),
        reveal
    );
    let _ = render(&mut owners, &mut app, &changed);
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
    let dir = tempfile::tempdir().unwrap();
    let mut core = Core::for_tests(EditorFiles {
        dir: dir.path().to_owned(),
    })
    .unwrap();
    let config = HostConfig {
        id: HostId("host".into()),
        name: "Forge".into(),
        address: "127.0.0.1:1".into(),
        fingerprint: "0".repeat(64),
        kind: HostKind::Remote,
    };
    let client = ForgeClient::new(
        &config.address,
        &config.fingerprint,
        core.identity.as_ref().unwrap(),
    )
    .unwrap();
    core.add_test_host(&config, client.clone());
    let mut conversation = Conversation::new(&core);
    conversation.open_thread(&mut core, config.id.clone(), ThreadId("thread".into()));
    let scope = PresentationScope::capture(&conversation, &core).unwrap();
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
    core.add_test_host(&config, client);
    assert!(!scope.current(&conversation, &core));
    owners.rebind(&mut app, None);
}

#[test]
fn running_inline_reply_without_live_stream_realizes_from_its_text() {
    use artisan_api::{Event, Item, ItemKind, Phase, Status, TextContent};
    use editor_aqua::conversation::adapter::row_model;
    use editor_aqua::conversation::fold::{Row, RowKey};
    use editor_aqua::conversation::view::factory::{RealizedRow, RowFactory};
    use editor_aqua::conversation::view::reveal::MountedReveals;
    use editor_aqua::conversation::view::status::StatusShimmers;
    use editor_aqua::core::runtime::runtime;
    use editor_aqua::core::{CoreEvent, HostEvent, Screen};
    use editor_aqua::ui::theme::{ArtisanTheme, ThemeMode};
    use std::cell::RefCell;
    use std::time::{Duration, Instant};

    let directory = tempfile::tempdir().unwrap();
    let mut core = Core::for_tests(EditorFiles {
        dir: directory.path().to_owned(),
    })
    .unwrap();
    let config = HostConfig {
        id: HostId("inline-host".into()),
        name: "Forge".into(),
        address: "127.0.0.1:1".into(),
        fingerprint: "0".repeat(64),
        kind: HostKind::Remote,
    };
    let client = ForgeClient::new(
        &config.address,
        &config.fingerprint,
        core.identity.as_ref().unwrap(),
    )
    .unwrap();
    core.add_test_host(&config, client);
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
            id: id.clone(),
            thread_id: thread.clone(),
            position: 1,
            seq: 1,
            run_id: None,
            parent_id: None,
            status: Status::Running,
            created_at: 0,
            updated_at: 0,
            kind: ItemKind::AssistantMessage {
                text: TextContent::Inline("streamed words so far".into()),
                phase: Phase::Final,
            },
        },
    }));
    let deadline = Instant::now() + Duration::from_secs(15);
    let mut delivered = false;
    while !delivered {
        core.drain_with(|core, event| {
            delivered |= matches!(&event, CoreEvent::Host { event: HostEvent::Item(item), .. } if item.id == id);
            conversation.core_event(core, &event, false);
        });
        assert!(Instant::now() < deadline, "item event was not delivered");
    }
    let index = conversation
        .rows
        .iter()
        .position(|(row, _)| matches!(row, Row::Text { id: row_id, slot: Slot::Prose, .. } if row_id == &id))
        .unwrap();
    let model = row_model(&conversation, index, None).unwrap();
    let scope = PresentationScope::capture(&conversation, &core).unwrap();
    let mut app = aqua::UiApp::new(SharedFonts::new(), SharedRuns::new());
    app.set_viewport(800.0, 600.0, 1.0);
    let markdown = Rc::new(RefCell::new(MarkdownOwners::new(SelectionDocumentId(805))));
    markdown.borrow_mut().rebind(&mut app, Some(scope.clone()));
    let owners = Rc::new(RefCell::new(MountedReveals::default()));
    let row_cache = Rc::new(RefCell::new(
        editor_aqua::conversation::view::row_cache::RowCache::default(),
    ));
    let next = std::cell::Cell::new(1000);
    let factory = RowFactory::new(
        vec![RealizedRow {
            model,
            disclosure: Default::default(),
            held: false,
        }],
        scope,
        markdown,
        Rc::new(RefCell::new(StatusShimmers::default())),
        *ArtisanTheme::for_mode(ThemeMode::Dark),
        true,
        Rc::new(move || {
            next.set(next.get() + 1);
            SelectionSourceId(next.get())
        }),
        Rc::new(|| {}),
        Rc::new(|_| {}),
        Rc::new(|_| {}),
        Rc::new(|_| {}),
    )
    .with_row_cache(row_cache)
    .with_reveals(&conversation, &core, owners, Instant::now(), false)
    .unwrap();
    assert!(factory.realize(0, &RowKey::Item(id)).is_ok());
}
