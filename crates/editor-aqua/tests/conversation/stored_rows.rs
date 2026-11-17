use aqua_text::document::{SelectionDocumentId, SelectionSourceId};
use artisan_api::{Collection, CollectionRef, ContentId, ContentRef, Item, ItemKind, Status};
use editor_aqua::conversation::fold::{Row, RowKey};
use editor_aqua::conversation::presentation::MarkdownOwners;
use editor_aqua::conversation::view::row_cache::RowCache;
use editor_aqua::core::Core;
use editor_aqua::ui::theme::{ArtisanTheme, ThemeMode};
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;
use test_support::editor::core::{OpenThread, connect, remote_host, test_core};
use test_support::editor::rows::{listed, realized, row_factory, row_list, row_model, source_ids};
use test_support::editor::ui::{click, mount, mounted, ui_app};
use test_support::items::item;

fn question(id: &str, digit: char) -> Arc<Item> {
    Arc::new(Item {
        position: if id == "one" { 1 } else { 2 },
        status: Status::AwaitingUser,
        ..item(
            id,
            "thread",
            1,
            ItemKind::Question {
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
        )
    })
}

#[test]
fn simultaneous_stored_question_rows_keep_independent_leases_across_cache_eviction() {
    let opened = OpenThread::new("question-host", "thread");
    let scope = opened.scope();
    let OpenThread {
        directory: _directory,
        core,
        conversation,
        ..
    } = opened;
    let provider = core.source_provider(&scope.host, scope.instance).unwrap();
    let cache = Rc::new(RefCell::new(RowCache::default()));
    let markdown = Rc::new(RefCell::new(MarkdownOwners::new(SelectionDocumentId(415))));
    let items = [question("one", '1'), question("two", '2')];
    let rows = items
        .iter()
        .map(|item| realized(row_model(Row::Prompt(item.id.clone()), [item.clone()])))
        .collect();
    let (ids, allocate) = source_ids(0);
    let emitted = Rc::new(RefCell::new(Vec::new()));
    let capture = emitted.clone();
    let factory = Rc::new(
        row_factory(rows, scope.clone(), markdown, true, allocate)
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
    let mut app = mounted(
        800.0,
        600.0,
        row_list(&factory, listed(keys.clone()), 120.0, 500.0),
    );
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
    let mut failed_app = mounted(800.0, 600.0, factory.realize(0, &keys[0]).unwrap().element);
    click(&mut failed_app, "collection-retry");
    let stale = emitted.borrow_mut().pop().unwrap();
    let mut changed = (*items[0]).clone();
    changed.seq += 1;
    assert!(!stale.dispatch(&core, &scope, &changed).0);
    assert!(matches!(
        browser.borrow().window().state(),
        editor_aqua::core::RangeState::Failed(_)
    ));
    click(&mut failed_app, "collection-retry");
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
    let mut ready_app = mounted(800.0, 600.0, factory.realize(0, &keys[0]).unwrap().element);
    click(&mut ready_app, "collection-entry-0");
    let choose = emitted.borrow_mut().pop().expect("question row action");
    assert!(choose.dispatch(&core, &scope, &items[0]).0);
    assert_eq!(
        browser.borrow().inline_text.as_deref(),
        Some("Ready question")
    );
    let mut detail_app = mounted(
        800.0,
        600.0,
        row_list(&factory, listed([keys[0].clone()]), 200.0, 500.0),
    );
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
    let mut cleanup_app = ui_app(800.0, 600.0);
    cache.borrow().drain_cleanup(&mut cleanup_app);
}

#[test]
fn recreated_stored_source_rejects_old_export_even_when_picker_generation_repeats() {
    use artisan_api::{NoticeKind, TextContent};
    use editor_aqua::conversation::content::{SourceEffect, SourceField, SourceOwners};
    let dir = tempfile::tempdir().unwrap();
    let mut core = test_core(dir.path());
    let config = remote_host("export-host");
    connect(&mut core, &config);
    let instance = core.host(&config.id).unwrap().instance();
    let provider = core.source_provider(&config.id, instance).unwrap();
    let reference = ContentRef {
        id: ContentId("a".repeat(64)),
        bytes: 10,
        lines: 1,
    };
    let item = Arc::new(item(
        "notice",
        "thread",
        1,
        ItemKind::Notice {
            kind: NoticeKind::Info,
            text: TextContent::Stored(reference),
        },
    ));
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
        let mut app = ui_app(800.0, 600.0);
        mount(&mut app, element);
        click(&mut app, "source-export");
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
        let admitted = test_support::wait::within(
            "the range completion's admission",
            Duration::from_secs(10),
            async {
                loop {
                    tokio::select! {
                        admitted = &mut send => break admitted,
                        () = tokio::task::yield_now() => { browser.borrow_mut().drain(core); }
                    }
                }
            },
        )
        .await;
        assert!(admitted, "the browser admits the range completion");
    });
    browser.borrow_mut().drain(core);
}
