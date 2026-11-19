use aqua_text::document::{SelectionDocumentId, SelectionSourceId};
use artisan_api::ThreadId;
use artisan_api::{Collection, CollectionRef, ContentId, ContentRef, Item, ItemKind, Status};
use axum::http::StatusCode;
use editor_aqua::conversation::controller::Conversation;
use editor_aqua::conversation::fold::{Row, RowKey};
use editor_aqua::conversation::presentation::MarkdownOwners;
use editor_aqua::conversation::presentation::PresentationScope;
use editor_aqua::conversation::view::row_cache::RowCache;
use editor_aqua::ui::theme::{ArtisanTheme, ThemeMode};
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;
use test_support::editor::core::{StubbedHost, connect, remote_host, test_core};
use test_support::editor::forge::empty_state;
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
#[expect(
    clippy::too_many_lines,
    reason = "one eviction scenario walked step by step"
)]
fn simultaneous_stored_question_rows_keep_independent_leases_across_cache_eviction() {
    let mut stubbed = StubbedHost::start(empty_state(), axum::Router::new());
    let mut conversation = Conversation::new(&stubbed.core);
    let host = stubbed.host.clone();
    conversation.open_thread(&mut stubbed.core, host, ThreadId::from("thread"));
    let scope = PresentationScope::capture(&conversation, &stubbed.core).unwrap();
    let StubbedHost {
        core,
        forge,
        directory: _directory,
        ..
    } = stubbed;
    let provider = core.source_provider(&scope.host, scope.instance).unwrap();
    let sources = provider.clone();
    let cache = Rc::new(RefCell::new(RowCache::default()));
    let markdown = Rc::new(RefCell::new(MarkdownOwners::new(SelectionDocumentId(415))));
    let items = [question("one", '1'), question("two", '2')];
    let rows = items
        .iter()
        .map(|item| realized(row_model(Row::Prompt(item.id.clone()), [Arc::clone(item)])))
        .collect();
    let (ids, allocate) = source_ids(0);
    let emitted = Rc::new(RefCell::new(Vec::new()));
    let capture = Rc::clone(&emitted);
    let factory = Rc::new(
        row_factory(rows, scope.clone(), markdown, true, allocate)
            .with_row_cache(Rc::clone(&cache))
            .with_sources(
                sources,
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
    let realize = |row: usize| {
        mounted(
            800.0,
            600.0,
            factory.realize(row, &keys[row]).unwrap().element,
        )
    };
    let has = |app: &aqua::UiApp, key: &str| {
        app.tree()
            .last_node_with_key(aqua_ui::tree::hash_key(aqua_ui::element::Key::from(key)))
            .is_some()
    };
    let failed = |app: &aqua::UiApp| has(app, "collection-retry");
    let loading = |app: &aqua::UiApp| {
        test_support::editor::ui::painted_texts(app).contains(&"Loading entry…".to_owned())
            && !failed(app)
    };
    let ready = |app: &aqua::UiApp| has(app, "collection-entry-0");
    let resolve = |id: &artisan_api::ItemId| items.iter().find(|item| &item.id == id).cloned();
    let settle = |row: usize, what: &str, done: &dyn Fn(&aqua::UiApp) -> bool| {
        let owner = if row == 0 { &first } else { &second };
        test_support::wait::wait_until(what, Duration::from_secs(10), || {
            drop(owner.drain_sources(&core, &scope, &provider, resolve));
            done(&realize(row))
        });
    };
    let page = |id: &str, text: &str, row: usize| {
        let ItemKind::Question {
            questions: Collection::Stored(reference),
            ..
        } = &items[row].kind
        else {
            panic!("expected stored questions")
        };
        artisan_api::CollectionRange {
            source: reference.source.clone(),
            start: 0,
            end: 1,
            total: 1,
            records: vec![artisan_api::Question {
                id: id.into(),
                header: None,
                text: artisan_api::TextContent::Inline(text.into()),
                multi_select: false,
                options: Collection::Inline(Vec::new()),
            }],
        }
    };
    let other_read = forge.take("GET", &records('2'));
    forge.take("GET", &records('1')).fail(
        StatusCode::INTERNAL_SERVER_ERROR,
        "internal",
        "test failure",
    );
    settle(0, "the first read fails", &failed);
    let mut failed_app = realize(0);
    click(&mut failed_app, "collection-retry");
    let superseded = emitted.borrow_mut().pop().unwrap();
    click(&mut failed_app, "collection-retry");
    let newer = emitted.borrow_mut().pop().unwrap();
    assert!(superseded.dispatch(&core, &scope, &items[0]).0);
    let old = forge.take("GET", &records('1'));
    let (changed, effect) = newer.dispatch(&core, &scope, &items[0]);
    assert!(
        !changed,
        "a second retry keeps showing the load already running"
    );
    assert!(effect.is_none());
    forge.take("GET", &records('1')).fail(
        StatusCode::INTERNAL_SERVER_ERROR,
        "internal",
        "test failure",
    );
    settle(0, "the current read fails", &failed);
    old.respond(page("ready", "Ready question", 0));
    settle(0, "the superseded read lands", &|_| true);
    assert!(failed(&realize(0)));
    assert!(!ready(&realize(0)));
    assert!(loading(&realize(1)));
    click(&mut failed_app, "collection-retry");
    let stale = emitted.borrow_mut().pop().unwrap();
    let mut changed = (*items[0]).clone();
    changed.seq += 1;
    assert!(!stale.dispatch(&core, &scope, &changed).0);
    assert!(failed(&realize(0)));
    click(&mut failed_app, "collection-retry");
    let action = emitted.borrow_mut().pop().unwrap();
    assert!(action.dispatch(&core, &scope, &items[0]).0);
    assert!(loading(&realize(0)));
    forge
        .take("GET", &records('1'))
        .respond(page("ready", "Ready question", 0));
    settle(0, "the retried read lands", &ready);
    assert!(
        test_support::editor::ui::painted_texts(&realize(0))
            .contains(&"1. Ready question".to_owned())
    );
    assert!(loading(&realize(1)));
    let mut ready_app = realize(0);
    click(&mut ready_app, "collection-entry-0");
    let choose = emitted.borrow_mut().pop().expect("question row action");
    assert!(choose.dispatch(&core, &scope, &items[0]).0);
    let detail = realize(0);
    assert!(has(&detail, "collection-back"));
    assert!(
        test_support::editor::ui::painted_texts(&detail).contains(&"Ready question".to_owned())
    );
    other_read.respond(page("other", "Other question", 1));
    settle(1, "the other row's read lands", &ready);
    assert!(
        test_support::editor::ui::painted_texts(&realize(1))
            .contains(&"1. Other question".to_owned())
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
    drop(detail);
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
        let actions = Rc::clone(&actions);
        owners
            .view(
                &item.id,
                &SourceField::Notice,
                ArtisanTheme::for_mode(ThemeMode::Dark),
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

fn records(digit: char) -> String {
    format!("/content/{}/records", digit.to_string().repeat(64))
}
