use aqua_text::document::SelectionDocumentId;
use artisan_api::{
    Answer, Approval, ApprovalRequest, ContentId, ContentRef, Decision, Item, ItemKind, Status,
    TextContent, ToolKind,
};
use editor_aqua::conversation::adapter::{Action, ActionScope, RowModel};
use editor_aqua::conversation::fold::{Row, RowKey};
use editor_aqua::conversation::presentation::MarkdownOwners;
use editor_aqua::conversation::stored::StoredConversation;
use editor_aqua::conversation::thread::AnswerState;
use editor_aqua::conversation::view::row_cache::RowCache;
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Instant;
use test_support::editor::core::{OpenThread, connect};
use test_support::editor::rows::{
    fixed_source, realized, row_factory, row_factory_with, row_model,
};
use test_support::editor::ui::{click, mounted};
use test_support::items;

#[test]
#[expect(
    clippy::too_many_lines,
    reason = "one approval scenario walked through each fence in turn"
)]
fn approval_pointer_dispatch_and_stored_source_are_fenced_by_item_and_host_lifetime() {
    let opened = OpenThread::new("approval-host", "thread");
    let scope = opened.scope();
    let OpenThread {
        directory: _directory,
        mut core,
        host: config,
        thread,
        mut conversation,
    } = opened;
    let action_scope = ActionScope::new(&conversation).unwrap();
    let provider = core.source_provider(&scope.host, scope.instance).unwrap();
    let approval = items::item(
        "approval",
        thread.clone(),
        7,
        ItemKind::Tool {
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
    );
    let item = Arc::new(Item {
        position: 1,
        status: Status::AwaitingUser,
        ..approval
    });
    let key = RowKey::Item(item.id.clone());
    let cache = Rc::new(RefCell::new(RowCache::default()));
    let markdown = Rc::new(RefCell::new(MarkdownOwners::new(SelectionDocumentId(510))));
    let actions = Rc::new(RefCell::new(Vec::new()));
    let observed = Rc::clone(&actions);
    let source_actions = Rc::new(RefCell::new(Vec::new()));
    let observed_sources = Rc::clone(&source_actions);
    let row = |sending| {
        realized(RowModel {
            version: 7,
            answers: HashMap::from([(
                item.id.clone(),
                if sending {
                    AnswerState::Sending
                } else {
                    AnswerState::Failed("Previous failure".into())
                },
            )]),
            ..row_model(Row::Prompt(item.id.clone()), [Arc::clone(&item)])
        })
    };
    let factory = |sending| {
        row_factory_with(
            vec![row(sending)],
            scope.clone(),
            Rc::clone(&markdown),
            true,
            fixed_source(511),
            Instant::now(),
            {
                let observed = Rc::clone(&observed);
                let action_scope = action_scope.clone();
                Rc::new(move |action: Action| {
                    observed
                        .borrow_mut()
                        .push((action.clone(), action_scope.bind(action)));
                })
            },
            Rc::default(),
        )
        .with_row_cache(Rc::clone(&cache))
        .with_sources(provider.clone(), SelectionDocumentId(510), {
            let observed_sources = Rc::clone(&observed_sources);
            Rc::new(move |action| observed_sources.borrow_mut().push(action))
        })
        .unwrap()
    };
    let ready = factory(false);
    let mut app = mounted(800.0, 900.0, ready.realize(0, &key).unwrap().element);
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
    let mut sending_app = mounted(800.0, 900.0, sending.realize(0, &key).unwrap().element);
    click(&mut sending_app, "approve-approval-0");
    assert!(actions.borrow().is_empty());
    let notice = Arc::new(Item {
        position: 2,
        ..items::item(
            "notice",
            thread,
            8,
            ItemKind::Notice {
                kind: artisan_api::NoticeKind::Info,
                text: TextContent::Stored(ContentRef {
                    id: ContentId("b".repeat(64)),
                    bytes: 10,
                    lines: 1,
                }),
            },
        )
    });
    let notice_key = RowKey::Item(notice.id.clone());
    let notice_row = realized(RowModel {
        version: 8,
        ..row_model(
            Row::WorkEntry {
                id: notice.id.clone(),
                run: None,
            },
            [Arc::clone(&notice)],
        )
    });
    let notice_factory = row_factory(
        vec![notice_row],
        scope.clone(),
        Rc::clone(&markdown),
        true,
        fixed_source(512),
    )
    .with_row_cache(Rc::clone(&cache))
    .with_sources(provider.clone(), SelectionDocumentId(510), {
        let observed_sources = Rc::clone(&observed_sources);
        Rc::new(move |action| observed_sources.borrow_mut().push(action))
    })
    .unwrap();
    let mut notice_app = mounted(
        800.0,
        900.0,
        notice_factory.realize(0, &notice_key).unwrap().element,
    );
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
    core.remove_host(&config.id).unwrap();
    connect(&mut core, &config);
    assert!(!scoped.dispatch(
        &mut conversation,
        &mut StoredConversation::default(),
        &core,
        true,
        Instant::now()
    ));
}
