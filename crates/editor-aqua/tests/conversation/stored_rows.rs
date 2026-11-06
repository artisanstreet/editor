use aqua_text::document::{SelectionDocumentId, SelectionSourceId};
use aqua_text::{SharedFonts, SharedRuns};
use artisan_api::{
    Collection, CollectionRef, ContentId, ContentRef, Item, ItemId, ItemKind, Status, ThreadId,
};
use editor_aqua::conversation::adapter::RowModel;
use editor_aqua::conversation::controller::Conversation;
use editor_aqua::conversation::fold::{Row, RowKey};
use editor_aqua::conversation::presentation::{MarkdownOwners, PresentationScope};
use editor_aqua::conversation::view::factory::{RealizedRow, RowFactory};
use editor_aqua::conversation::view::row_cache::RowCache;
use editor_aqua::conversation::view::status::StatusShimmers;
use editor_aqua::core::{Core, EditorFiles, ForgeClient, HostConfig, HostId, HostKind};
use editor_aqua::ui::theme::{ArtisanTheme, ThemeMode};
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

fn question(id: &str, digit: char) -> Arc<Item> {
    Arc::new(Item {
        id: ItemId(id.into()),
        thread_id: ThreadId("thread".into()),
        position: if id == "one" { 1 } else { 2 },
        seq: 1,
        run_id: None,
        parent_id: None,
        status: Status::AwaitingUser,
        created_at: 0,
        updated_at: 0,
        kind: ItemKind::Question {
            blocking: true,
            questions: Collection::Stored(CollectionRef {
                source: ContentRef {
                    id: ContentId(digit.to_string().repeat(64)),
                    bytes: 10,
                    lines: 1,
                },
                count: 1,
            }),
            answers: None,
            answer_text: None,
        },
    })
}

#[test]
fn simultaneous_stored_question_rows_keep_independent_leases_across_cache_eviction() {
    let dir = tempfile::tempdir().unwrap();
    let mut core = Core::for_tests(EditorFiles {
        dir: dir.path().to_owned(),
    })
    .unwrap();
    let config = HostConfig {
        id: HostId("question-host".into()),
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
    conversation.open_thread(&mut core, config.id.clone(), ThreadId("thread".into()));
    let scope = PresentationScope::capture(&conversation, &core).unwrap();
    let provider = core.source_provider(&scope.host, scope.instance).unwrap();
    let cache = Rc::new(RefCell::new(RowCache::default()));
    let markdown = Rc::new(RefCell::new(MarkdownOwners::new(SelectionDocumentId(415))));
    let items = [question("one", '1'), question("two", '2')];
    let rows = items
        .iter()
        .map(|item| RealizedRow {
            model: RowModel {
                row: Row::Prompt(item.id.clone()),
                version: 1,
                items: HashMap::from([(item.id.clone(), item.clone())]),
                live: HashMap::new(),
                ui: HashMap::new(),
                run: None,
                gap: 0.0,
                end: 0.0,
            },
            disclosure: Default::default(),
            held: false,
        })
        .collect();
    let ids = Rc::new(Cell::new(0));
    let allocate = ids.clone();
    let emitted = Rc::new(RefCell::new(Vec::new()));
    let capture = emitted.clone();
    let factory = Rc::new(
        RowFactory::new(
            rows,
            scope.clone(),
            markdown,
            Rc::new(RefCell::new(StatusShimmers::default())),
            *ArtisanTheme::for_mode(ThemeMode::Dark),
            true,
            Rc::new(move || {
                allocate.set(allocate.get() + 1);
                SelectionSourceId(allocate.get())
            }),
            Rc::new(|| {}),
            Rc::new(|_| {}),
            Rc::new(|_| {}),
            Rc::new(|_| {}),
        )
        .with_row_cache(cache.clone())
        .with_sources(
            provider.clone(),
            SelectionDocumentId(415),
            Rc::new(move |action| capture.borrow_mut().push(action)),
        )
        .unwrap(),
    );
    let keys = [
        RowKey::Item(items[0].id.clone()),
        RowKey::Item(items[1].id.clone()),
    ];
    let callback = factory.callback();
    let callback_keys = keys.clone();
    let list = aqua_ui::collection::List::new()
        .items(vec![0_usize, 1])
        .key(|index: &usize| format!("stored-question-{index}"))
        .try_batched_row(move |index: &usize| callback(*index, &callback_keys[*index]))
        .virtualize()
        .estimated_row_height(aqua_ui::style::Length::Px(120.0))
        .style(
            aqua_ui::style::Style::new()
                .width(aqua_ui::style::Length::Px(600.0))
                .height(aqua_ui::style::Length::Px(500.0)),
        )
        .build();
    let mut app = aqua::UiApp::new(SharedFonts::new(), SharedRuns::new());
    app.set_viewport(800.0, 600.0, 1.0);
    app.mount(|_| list).unwrap();
    app.layout().unwrap();
    app.build_frame().unwrap();
    let first = cache.borrow().get(&keys[0]).unwrap();
    let second = cache.borrow().get(&keys[1]).unwrap();
    assert!(!Rc::ptr_eq(&first, &second));
    assert_eq!(
        cache
            .borrow()
            .live_owners()
            .iter()
            .filter(|owner| owner.strong_count() > 0)
            .count(),
        2
    );
    assert_eq!(ids.get(), 2);
    let browser = first.question_browser_for_tests().unwrap();
    let other_browser = second.question_browser_for_tests().unwrap();
    assert!(!Rc::ptr_eq(&browser, &other_browser));
    let ItemKind::Question {
        questions: Collection::Stored(other_source),
        ..
    } = &items[1].kind
    else {
        panic!("expected stored questions")
    };
    other_browser.borrow_mut().retry(&core);
    let _other_completion = other_browser.borrow_mut().completion_for_tests();
    assert_eq!(other_browser.borrow().window().source(), other_source);
    assert!(matches!(
        other_browser.borrow().window().state(),
        editor_aqua::core::RangeState::Loading
    ));
    let reference = items[0].kind.clone();
    let ItemKind::Question {
        questions: Collection::Stored(reference),
        ..
    } = reference
    else {
        panic!("expected stored questions")
    };
    let page = artisan_api::CollectionRange {
        source: reference.source.clone(),
        start: 0,
        end: 1,
        total: 1,
        records: vec![artisan_api::Question {
            id: "ready".into(),
            header: None,
            text: artisan_api::TextContent::Inline("Ready question".into()),
            multi_select: false,
            options: Collection::Inline(Vec::new()),
        }],
    };
    let old = browser.borrow_mut().completion_for_tests();
    let failed = browser.borrow_mut().completion_for_tests();
    deliver(
        &browser,
        &failed,
        Err(anyhow::anyhow!("test failure")),
        &core,
    );
    assert!(matches!(
        browser.borrow().window().state(),
        editor_aqua::core::RangeState::Failed(_)
    ));
    deliver(&browser, &old, Ok(page.clone()), &core);
    assert!(matches!(
        browser.borrow().window().state(),
        editor_aqua::core::RangeState::Failed(_)
    ));
    assert_eq!(other_browser.borrow().window().source(), other_source);
    assert!(matches!(
        other_browser.borrow().window().state(),
        editor_aqua::core::RangeState::Loading
    ));
    let failed_row = factory.realize(0, &keys[0]).unwrap().element;
    let mut failed_app = aqua::UiApp::new(SharedFonts::new(), SharedRuns::new());
    failed_app.set_viewport(800.0, 600.0, 1.0);
    failed_app.mount(|_| failed_row).unwrap();
    failed_app.layout().unwrap();
    click_key(&mut failed_app, "collection-retry");
    let stale = emitted.borrow_mut().pop().unwrap();
    let mut changed = (*items[0]).clone();
    changed.seq += 1;
    assert!(!stale.dispatch(&core, &scope, &changed).0);
    assert!(matches!(
        browser.borrow().window().state(),
        editor_aqua::core::RangeState::Failed(_)
    ));
    click_key(&mut failed_app, "collection-retry");
    let action = emitted.borrow_mut().pop().unwrap();
    assert!(action.dispatch(&core, &scope, &items[0]).0);
    assert!(matches!(
        browser.borrow().window().state(),
        editor_aqua::core::RangeState::Loading
    ));
    let ready = browser.borrow_mut().completion_for_tests();
    deliver(&browser, &ready, Ok(page), &core);
    assert!(
        matches!(browser.borrow().window().state(), editor_aqua::core::RangeState::Ready(page) if page.records[0].id == "ready")
    );
    assert_eq!(other_browser.borrow().window().source(), other_source);
    assert!(matches!(
        other_browser.borrow().window().state(),
        editor_aqua::core::RangeState::Loading
    ));
    let ready_row = factory.realize(0, &keys[0]).unwrap().element;
    let mut ready_app = aqua::UiApp::new(SharedFonts::new(), SharedRuns::new());
    ready_app.set_viewport(800.0, 600.0, 1.0);
    ready_app.mount(|_| ready_row).unwrap();
    ready_app.layout().unwrap();
    click_key(&mut ready_app, "collection-entry-0");
    let choose = emitted.borrow_mut().pop().expect("question row action");
    assert!(choose.dispatch(&core, &scope, &items[0]).0);
    assert_eq!(
        browser.borrow().inline_text.as_deref(),
        Some("Ready question")
    );
    let detail_factory = factory.callback();
    let detail_key = keys[0].clone();
    let list = aqua_ui::collection::List::new()
        .items(vec![0_usize])
        .key(|_: &usize| "question-detail-row")
        .try_batched_row(move |index: &usize| detail_factory(*index, &detail_key))
        .virtualize()
        .estimated_row_height(aqua_ui::style::Length::Px(200.0))
        .style(
            aqua_ui::style::Style::new()
                .width(aqua_ui::style::Length::Px(600.0))
                .height(aqua_ui::style::Length::Px(500.0)),
        )
        .build();
    let mut detail_app = aqua::UiApp::new(SharedFonts::new(), SharedRuns::new());
    detail_app.set_viewport(800.0, 600.0, 1.0);
    detail_app.mount(|_| list).unwrap();
    detail_app.layout().unwrap();
    assert!(
        detail_app.unregister_selection_source_in(SelectionDocumentId(415), SelectionSourceId(1))
    );
    drop(detail_app);
    drop(ready_app);
    assert!(
        editor_aqua::conversation::read::evidence(
            &conversation,
            &core,
            &Row::Prompt(items[0].id.clone()),
            |_, _| true
        )
        .is_none()
    );
    drop(failed_app);
    drop(browser);
    drop(other_browser);
    let weak = Rc::downgrade(&first);
    drop(first);
    drop(second);
    cache.borrow_mut().rebind(None);
    assert!(weak.upgrade().is_some());
    drop(app);
    assert!(weak.upgrade().is_none());
    let mut cleanup_app = aqua::UiApp::new(SharedFonts::new(), SharedRuns::new());
    cache.borrow().drain_cleanup(&mut cleanup_app);
}

#[test]
fn recreated_stored_source_rejects_old_export_even_when_picker_generation_repeats() {
    use artisan_api::{NoticeKind, TextContent};
    use editor_aqua::conversation::content::{SourceEffect, SourceField, SourceOwners};
    let dir = tempfile::tempdir().unwrap();
    let mut core = Core::for_tests(EditorFiles {
        dir: dir.path().to_owned(),
    })
    .unwrap();
    let config = HostConfig {
        id: HostId("export-host".into()),
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
    let instance = core.host(&config.id).unwrap().instance();
    let provider = core.source_provider(&config.id, instance).unwrap();
    let reference = ContentRef {
        id: ContentId("a".repeat(64)),
        bytes: 10,
        lines: 1,
    };
    let item = Arc::new(Item {
        id: ItemId("notice".into()),
        thread_id: ThreadId("thread".into()),
        position: 1,
        seq: 1,
        run_id: None,
        parent_id: None,
        status: Status::Done,
        created_at: 0,
        updated_at: 0,
        kind: ItemKind::Notice {
            kind: NoticeKind::Info,
            text: TextContent::Stored(reference),
        },
    });
    let mut owners = SourceOwners::default();
    let actions = Rc::new(RefCell::new(Vec::new()));
    let make_view = |owners: &mut SourceOwners| {
        assert!(owners.open_provider(&provider, &item, SourceField::Notice));
        let actions = actions.clone();
        owners
            .view(
                &item.id,
                &SourceField::Notice,
                *ArtisanTheme::for_mode(ThemeMode::Dark),
                SelectionDocumentId(416),
                SelectionSourceId(1),
                Rc::new(move |action| actions.borrow_mut().push(action)),
            )
            .unwrap()
            .element
    };
    let click_export = |element| {
        let mut app = aqua::UiApp::new(SharedFonts::new(), SharedRuns::new());
        app.set_viewport(800.0, 600.0, 1.0);
        app.mount(|_| element).unwrap();
        app.layout().unwrap();
        let key = aqua_ui::tree::hash_key(aqua_ui::element::Key::from("source-export"));
        let node = app
            .tree()
            .traverse()
            .into_iter()
            .find(|node| {
                app.tree()
                    .get(*node)
                    .is_some_and(|record| record.key == Some(key))
            })
            .unwrap();
        let bounds = app.tree().visual_rect(node).unwrap();
        let down = aqua_core::PointerEvent {
            contact: 1,
            kind: aqua_core::PointerKind::Mouse,
            position: aqua_core::Point::new(bounds.x + bounds.w / 2.0, bounds.y + bounds.h / 2.0),
            buttons: 1,
        };
        aqua_ui::events::dispatch_input(
            app.tree_mut(),
            &aqua_core::InputEvent::PointerDown(down, aqua_core::PointerButton::Primary),
        );
        aqua_ui::events::dispatch_input(
            app.tree_mut(),
            &aqua_core::InputEvent::PointerUp(
                aqua_core::PointerEvent { buttons: 0, ..down },
                aqua_core::PointerButton::Primary,
            ),
        );
    };
    click_export(make_view(&mut owners));
    let Some(SourceEffect::SavePath(old)) =
        owners.dispatch(&core, actions.borrow_mut().pop().unwrap())
    else {
        panic!("expected export")
    };
    assert!(owners.export_is_current(&old));
    owners.retain(&core, |_| None);
    click_export(make_view(&mut owners));
    let Some(SourceEffect::SavePath(new)) =
        owners.dispatch(&core, actions.borrow_mut().pop().unwrap())
    else {
        panic!("expected replacement export")
    };
    assert!(!owners.export_is_current(&old));
    assert!(owners.export_is_current(&new));
    owners.finish_export_result(&core, old, Ok(None));
    assert!(owners.export_is_current(&new));
}

fn deliver(
    browser: &Rc<RefCell<editor_aqua::sources::CollectionBrowser<artisan_api::Question>>>,
    completion: &editor_aqua::core::RangeCompletion<
        artisan_api::CollectionRange<artisan_api::Question>,
    >,
    result: anyhow::Result<artisan_api::CollectionRange<artisan_api::Question>>,
    core: &Core,
) {
    editor_aqua::core::runtime::runtime().block_on(async {
        let send = completion.complete(result);
        tokio::pin!(send);
        let deadline = tokio::time::sleep(std::time::Duration::from_secs(10));
        tokio::pin!(deadline);
        loop {
            tokio::select! {
                admitted = &mut send => { assert!(admitted); break; }
                () = &mut deadline => panic!("range completion was not admitted"),
                () = tokio::task::yield_now() => { browser.borrow_mut().drain(core); }
            }
        }
    });
    browser.borrow_mut().drain(core);
}

fn click_key(app: &mut aqua::UiApp, key: &str) {
    let key = aqua_ui::tree::hash_key(aqua_ui::element::Key::from(key));
    let node = app
        .tree()
        .traverse()
        .into_iter()
        .find(|node| {
            app.tree()
                .get(*node)
                .is_some_and(|record| record.key == Some(key))
        })
        .unwrap();
    let bounds = app.tree().visual_rect(node).unwrap();
    let down = aqua_core::PointerEvent {
        contact: 1,
        kind: aqua_core::PointerKind::Mouse,
        position: aqua_core::Point::new(bounds.x + bounds.w / 2.0, bounds.y + bounds.h / 2.0),
        buttons: 1,
    };
    aqua_ui::events::dispatch_input(
        app.tree_mut(),
        &aqua_core::InputEvent::PointerDown(down, aqua_core::PointerButton::Primary),
    );
    aqua_ui::events::dispatch_input(
        app.tree_mut(),
        &aqua_core::InputEvent::PointerUp(
            aqua_core::PointerEvent { buttons: 0, ..down },
            aqua_core::PointerButton::Primary,
        ),
    );
}
