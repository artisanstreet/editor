use aqua_text::document::{SelectionDocumentId, SelectionSourceId};
use aqua_text::{SharedFonts, SharedRuns};
use artisan_api::{
    Answer, Approval, ApprovalRequest, ContentId, ContentRef, Decision, Item, ItemId, ItemKind,
    Status, TextContent, ThreadId, ToolKind,
};
use editor_aqua::conversation::adapter::{Action, ActionScope, RowModel};
use editor_aqua::conversation::controller::Conversation;
use editor_aqua::conversation::fold::{Row, RowKey};
use editor_aqua::conversation::presentation::{MarkdownOwners, PresentationScope};
use editor_aqua::conversation::thread::ItemUi;
use editor_aqua::conversation::view::factory::{RealizedRow, RowFactory};
use editor_aqua::conversation::view::row_cache::RowCache;
use editor_aqua::conversation::view::status::StatusShimmers;
use editor_aqua::core::{Core, EditorFiles, ForgeClient, HostConfig, HostId, HostKind};
use editor_aqua::ui::theme::{ArtisanTheme, ThemeMode};
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Instant;

#[test]
fn approval_pointer_dispatch_and_stored_source_are_fenced_by_item_and_host_lifetime() {
    let dir = tempfile::tempdir().unwrap();
    let mut core = Core::for_tests(EditorFiles {
        dir: dir.path().to_owned(),
    })
    .unwrap();
    let config = HostConfig {
        id: HostId("approval-host".into()),
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
    let action_scope = ActionScope::new(&conversation).unwrap();
    let provider = core.source_provider(&scope.host, scope.instance).unwrap();
    let item = Arc::new(Item {
        id: ItemId("approval".into()),
        thread_id: ThreadId("thread".into()),
        position: 1,
        seq: 7,
        run_id: None,
        parent_id: None,
        status: Status::AwaitingUser,
        created_at: 0,
        updated_at: 0,
        kind: ItemKind::Tool {
            tool: ToolKind::Read {
                path: "file".into(),
                range: None,
            },
            title: None,
            output: None,
            exit_code: None,
            approval: Some(Approval {
                request: ApprovalRequest::Action {
                    description: TextContent::Stored(ContentRef {
                        id: ContentId("a".repeat(64)),
                        bytes: 10,
                        lines: 1,
                    }),
                },
                reason: None,
                options: vec![Decision::AllowOnce],
                decision: None,
            }),
        },
    });
    let key = RowKey::Item(item.id.clone());
    let cache = Rc::new(RefCell::new(RowCache::default()));
    let markdown = Rc::new(RefCell::new(MarkdownOwners::new(SelectionDocumentId(510))));
    let actions = Rc::new(RefCell::new(Vec::new()));
    let observed = actions.clone();
    let source_actions = Rc::new(RefCell::new(Vec::new()));
    let observed_sources = source_actions.clone();
    let row = |sending| RealizedRow {
        model: RowModel {
            row: Row::Prompt(item.id.clone()),
            version: 7,
            items: HashMap::from([(item.id.clone(), item.clone())]),
            live: HashMap::new(),
            ui: HashMap::from([(
                item.id.clone(),
                ItemUi {
                    sending,
                    error: Some("Previous failure".into()),
                    ..Default::default()
                },
            )]),
            run: None,
            gap: 0.0,
            end: 0.0,
        },
        disclosure: Default::default(),
        held: false,
    };
    let factory = |sending| {
        RowFactory::new(
            vec![row(sending)],
            scope.clone(),
            markdown.clone(),
            Rc::new(RefCell::new(StatusShimmers::default())),
            *ArtisanTheme::for_mode(ThemeMode::Dark),
            true,
            Rc::new(|| SelectionSourceId(511)),
            Rc::new(|| {}),
            Rc::new(|_| {}),
            Rc::new(|_| {}),
            Rc::new(|_| {}),
        )
        .with_row_cache(cache.clone())
        .with_sources(provider.clone(), SelectionDocumentId(510), {
            let observed_sources = observed_sources.clone();
            Rc::new(move |action| observed_sources.borrow_mut().push(action))
        })
        .unwrap()
        .with_dispatch({
            let observed = observed.clone();
            let action_scope = action_scope.clone();
            Rc::new(move |action: Action| {
                observed
                    .borrow_mut()
                    .push((action.clone(), action_scope.bind(action)))
            })
        })
    };
    let ready = factory(false);
    let mut app = aqua::UiApp::new(SharedFonts::new(), SharedRuns::new());
    app.set_viewport(800.0, 900.0, 1.0);
    app.mount(|_| ready.realize(0, &key).unwrap().element)
        .unwrap();
    app.layout().unwrap();
    click(&mut app, "source-export");
    let stale_source = source_actions
        .borrow_mut()
        .pop()
        .expect("stored source action");
    let mut newer = (*item).clone();
    newer.seq += 1;
    assert!(!stale_source.dispatch(&core, &scope, &newer).0);
    click(&mut app, "approve-approval-0");
    let (action, scoped) = actions.borrow_mut().pop().expect("approval action");
    assert!(
        matches!(action, Action::Answer { id, answer: Answer::Approval { decision: Decision::AllowOnce } } if id == item.id)
    );
    let sending = factory(true);
    let mut sending_app = aqua::UiApp::new(SharedFonts::new(), SharedRuns::new());
    sending_app.set_viewport(800.0, 900.0, 1.0);
    sending_app
        .mount(|_| sending.realize(0, &key).unwrap().element)
        .unwrap();
    sending_app.layout().unwrap();
    click(&mut sending_app, "approve-approval-0");
    assert!(actions.borrow().is_empty());
    let notice = Arc::new(Item {
        id: ItemId("notice".into()),
        thread_id: ThreadId("thread".into()),
        position: 2,
        seq: 8,
        run_id: None,
        parent_id: None,
        status: Status::Done,
        created_at: 0,
        updated_at: 0,
        kind: ItemKind::Notice {
            kind: artisan_api::NoticeKind::Info,
            text: TextContent::Stored(ContentRef {
                id: ContentId("b".repeat(64)),
                bytes: 10,
                lines: 1,
            }),
        },
    });
    let notice_key = RowKey::Item(notice.id.clone());
    let notice_row = RealizedRow {
        model: RowModel {
            row: Row::WorkEntry {
                id: notice.id.clone(),
                run: None,
            },
            version: 8,
            items: HashMap::from([(notice.id.clone(), notice.clone())]),
            live: HashMap::new(),
            ui: HashMap::new(),
            run: None,
            gap: 0.0,
            end: 0.0,
        },
        disclosure: Default::default(),
        held: false,
    };
    let notice_factory = RowFactory::new(
        vec![notice_row],
        scope.clone(),
        markdown.clone(),
        Rc::new(RefCell::new(StatusShimmers::default())),
        *ArtisanTheme::for_mode(ThemeMode::Dark),
        true,
        Rc::new(|| SelectionSourceId(512)),
        Rc::new(|| {}),
        Rc::new(|_| {}),
        Rc::new(|_| {}),
        Rc::new(|_| {}),
    )
    .with_row_cache(cache.clone())
    .with_sources(provider.clone(), SelectionDocumentId(510), {
        let observed_sources = observed_sources.clone();
        Rc::new(move |action| observed_sources.borrow_mut().push(action))
    })
    .unwrap()
    .with_dispatch(Rc::new(|_| {}))
    .with_work(
        Rc::new(RefCell::new(Default::default())),
        HashMap::new(),
        Instant::now(),
        0.0,
        0,
    );
    let mut notice_app = aqua::UiApp::new(SharedFonts::new(), SharedRuns::new());
    notice_app.set_viewport(800.0, 900.0, 1.0);
    notice_app
        .mount(|_| notice_factory.realize(0, &notice_key).unwrap().element)
        .unwrap();
    notice_app.layout().unwrap();
    click(&mut notice_app, "source-export");
    let action = source_actions
        .borrow_mut()
        .pop()
        .expect("notice source action");
    let (changed, effect) = action.dispatch(&core, &scope, &notice);
    assert!(changed);
    assert!(matches!(
        effect,
        Some(editor_aqua::conversation::view::row_sources::RowSourceEffect::SavePath(_))
    ));
    core.add_test_host(&config, client);
    assert!(!scoped.dispatch(&mut conversation, &core, true, Instant::now()));
}

fn click(app: &mut aqua::UiApp, key: &str) {
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
