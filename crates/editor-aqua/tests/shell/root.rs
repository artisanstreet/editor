use artisan_api::ItemId;
use editor_aqua::conversation::fold::{Row, Slot};
use editor_aqua::conversation::presentation::BodyKey;
use editor_aqua::core::Screen;
use editor_aqua::shell::application::SectionViews;
use editor_aqua::shell::root::RootViews;
use std::sync::Arc;
use test_support::clock::ManualClock;
use test_support::editor::shell::{Motion, RootFrames, ThreadShell, headless, window_id};
use test_support::editor::ui::text_opacity;
use test_support::items::{assistant_reply, item, user_message};

fn replies(opened: &mut ThreadShell, count: u64) {
    let thread = opened.thread.clone();
    let conversation = &mut opened.workspace().conversation;
    for index in 0..count {
        let reply = Arc::new(item(
            format!("reply-{index}"),
            thread.clone(),
            index + 1,
            assistant_reply(format!("Reply {index}.")),
        ));
        conversation.rows.push((
            Row::Text {
                id: reply.id.clone(),
                slot: Slot::Prose,
                block: 0,
                run: None,
            },
            1,
        ));
        conversation
            .view
            .as_mut()
            .unwrap()
            .context
            .insert(reply.id.clone(), reply);
    }
}

#[test]
fn landing_completion_rebuilds_hidden_transcript_before_subtree_refresh() {
    for expire in [false, true] {
        let clock = ManualClock::starting_at(1_700_000_000_000);
        let mut opened = ThreadShell::open_at(
            "landing-host",
            "landing-thread",
            Motion::Reduced,
            clock.clock(),
        );
        let window = opened.window("landing");
        let item = Arc::new(item(
            "landing-message",
            opened.thread.clone(),
            1,
            user_message("Loaded transcript message"),
        ));
        let ThreadShell {
            mut shell,
            mut resources,
            ..
        } = opened;
        let workspace = shell.entry.workspace_mut().unwrap();
        workspace.conversation.view.as_mut().unwrap().apply_page(
            editor_aqua::conversation::viewport::Fetch::Tail,
            artisan_api::ItemsPage {
                items: vec![item.as_ref().clone()],
                runs: Vec::new(),
                more: false,
                work: Vec::new(),
            },
        );
        workspace.conversation.rows.push((
            Row::Text {
                id: item.id.clone(),
                slot: Slot::Text(editor_aqua::conversation::fold::TextSection::User),
                block: 0,
                run: None,
            },
            1,
        ));
        let settle = std::time::Duration::from_secs(60);
        let deadline = shell.core.clock().now() + settle;
        workspace.conversation.landing =
            Some(editor_aqua::conversation::controller::Landing::Settling { until: deadline });
        let mut root = RootViews::default();
        let element = root.build(&mut shell, &mut resources.ui, &window).unwrap();
        resources.ui.mount(|_| element).unwrap();
        resources.ui.build_frame().unwrap();
        let transcript_opacity =
            |ui: &mut aqua::UiApp| text_opacity(ui, "Loaded transcript message");
        assert_eq!(
            transcript_opacity(&mut resources.ui),
            0.0,
            "landing must initially hide loaded rows"
        );
        let conversation = &mut shell.entry.workspace_mut().unwrap().conversation;
        if expire {
            clock.advance(settle);
            assert!(
                conversation.expire_landing(),
                "the scheduled deadline must end landing"
            );
        } else {
            use aqua_ui::collection_runtime::{CollectionViewport, FollowState};
            use aqua_ui::virtualization::RowRange;
            editor_aqua::conversation::view::transcript::observer(
                conversation,
                &shell.core,
                CollectionViewport {
                    visible: RowRange { start: 0, end: 1 },
                    mounted: RowRange { start: 0, end: 1 },
                    scroll_y: 0.0,
                    viewport_height: 600.0,
                    distance_from_start: 0.0,
                    distance_from_end: 0.0,
                    follow_state: FollowState::Tail { is_following: true },
                },
                true,
                true,
                false,
                |_, _| true,
                |_| false,
            );
            assert!(
                conversation.landing.is_none(),
                "loaded viewport observation must end landing"
            );
        }
        assert!(
            !root.refresh_transcript(&mut shell, &mut resources.ui, &window),
            "a landing transition must rebuild the outer frame, not only the inner list"
        );
        let element = root.build(&mut shell, &mut resources.ui, &window).unwrap();
        resources.ui.refresh(vec![element]).unwrap();
        resources.ui.build_frame().unwrap();
        assert_eq!(
            transcript_opacity(&mut resources.ui),
            1.0,
            "the next rebuilt scene must expose loaded rows"
        );
        assert!(
            root.refresh_transcript(&mut shell, &mut resources.ui, &window),
            "visible transcripts must still allow ordinary subtree refreshes"
        );
        resources.ui.build_frame().unwrap();
        assert_eq!(
            transcript_opacity(&mut resources.ui),
            1.0,
            "later subtree refreshes must preserve visible scene text"
        );
        let mut runtime = headless(window.clone());
        runtime.dispatch_frame_completed(
            &mut RootFrames {
                root: &mut root,
                shell: &mut shell,
                ui: &mut resources.ui,
            },
            window_id(),
            1,
        );
        let mut resized = window.clone();
        assert!(!root.resize_needs_rebuild(&shell, &resized));
        for (width, height) in [
            (980.0, 700.0),
            (960.0, 700.0),
            (960.0, 600.0),
            (1000.0, 700.0),
        ] {
            resized.inner_size.width = width;
            resized.inner_size.height = height;
            assert!(root.resize_needs_rebuild(&shell, &resized));
            resources.ui.set_viewport(width as f32, height as f32, 1.0);
            let element = root.build(&mut shell, &mut resources.ui, &resized).unwrap();
            resources.ui.refresh(vec![element]).unwrap();
            resources.ui.build_frame().unwrap();
            assert!(!root.resize_needs_rebuild(&shell, &resized));
            assert_eq!(transcript_opacity(&mut resources.ui), 1.0);
        }
    }
}

#[test]
fn mounted_root_prepares_only_realized_bodies_and_releases_sources_on_navigation() {
    let mut opened = ThreadShell::open("root-host", "root-thread", Motion::Full);
    let workspace = opened.workspace();
    let document = workspace.selections.conversation;
    let markdown = workspace.markdown.clone();
    replies(&mut opened, 1000);
    let window = opened.window("root");
    let ThreadShell {
        mut shell,
        mut resources,
        ..
    } = opened;
    let mut root = RootViews::default();
    let element = root.build(&mut shell, &mut resources.ui, &window).unwrap();
    resources.ui.mount(|_| element).unwrap();
    resources.ui.layout().unwrap();
    let key = |index| BodyKey {
        item: ItemId(format!("reply-{index}")),
        slot: Slot::Prose,
    };
    assert!(markdown.borrow().projected_range(&key(0), 0).is_some());
    assert!(markdown.borrow().projected_range(&key(999), 0).is_none());
    let sources: Vec<_> = resources
        .ui
        .tree()
        .traverse()
        .into_iter()
        .filter_map(|node| {
            let aqua_ui::tree::NodeKind::Text(text) = &resources.ui.tree().get(node)?.kind else {
                return None;
            };
            text.selection_source
                .as_ref()
                .filter(|binding| binding.document == document)
                .map(|binding| binding.slice.source)
        })
        .collect();
    assert!(!sources.is_empty());
    assert!(sources.len() < 100);
    let removed = ItemId("reply-0".into());
    let workspace = shell.entry.workspace_mut().unwrap();
    workspace
        .conversation
        .view
        .as_mut()
        .unwrap()
        .context
        .remove(&removed);
    workspace.conversation.rows.remove(0);
    let element = root.build(&mut shell, &mut resources.ui, &window).unwrap();
    resources.ui.refresh(vec![element]).unwrap();
    resources.ui.layout().unwrap();
    assert!(markdown.borrow().projected_range(&key(0), 0).is_none());
    assert!(markdown.borrow().projected_range(&key(1), 0).is_some());
    shell.core.navigate(Screen::Home);
    let element = root.build(&mut shell, &mut resources.ui, &window).unwrap();
    resources.ui.refresh(vec![element]).unwrap();
    assert!(markdown.borrow().projected_range(&key(1), 0).is_none());
    for source in sources {
        assert!(
            !resources
                .ui
                .unregister_selection_source_in(document, source)
        );
    }
}

#[test]
fn tail_controls_force_full_refresh_only_when_visibility_changes() {
    let mut opened = ThreadShell::open("root-host", "root-thread", Motion::Reduced);
    opened.workspace().conversation.landing = None;
    replies(&mut opened, 40);
    let window = opened.window("root");
    let ThreadShell {
        mut shell,
        mut resources,
        ..
    } = opened;
    let mut root = RootViews::default();
    let element = root.build(&mut shell, &mut resources.ui, &window).unwrap();
    resources.ui.mount(|_| element).unwrap();
    resources.ui.layout().unwrap();
    let key = aqua_ui::tree::hash_key(aqua_ui::element::Key::from("jump-latest"));
    assert!(resources.ui.tree().sole_node_with_key(key).is_none());
    assert!(root.refresh_transcript(&mut shell, &mut resources.ui, &window));
    let workspace = shell.entry.workspace_mut().unwrap();
    workspace.conversation.viewport(
        &shell.core,
        editor_aqua::conversation::viewport::Input::ScrolledAway {
            anchor: 1,
            through: 20,
        },
    );
    assert!(!root.refresh_transcript(&mut shell, &mut resources.ui, &window));
    let element = root.build(&mut shell, &mut resources.ui, &window).unwrap();
    resources.ui.refresh(vec![element]).unwrap();
    resources.ui.layout().unwrap();
    assert!(resources.ui.tree().sole_node_with_key(key).is_some());
    let workspace = shell.entry.workspace_mut().unwrap();
    workspace.conversation.viewport(
        &shell.core,
        editor_aqua::conversation::viewport::Input::ScrolledAway {
            anchor: 2,
            through: 21,
        },
    );
    assert!(root.refresh_transcript(&mut shell, &mut resources.ui, &window));
    let workspace = shell.entry.workspace_mut().unwrap();
    workspace.conversation.viewport(
        &shell.core,
        editor_aqua::conversation::viewport::Input::BottomReached,
    );
    assert!(!root.refresh_transcript(&mut shell, &mut resources.ui, &window));
    let element = root.build(&mut shell, &mut resources.ui, &window).unwrap();
    resources.ui.refresh(vec![element]).unwrap();
    resources.ui.layout().unwrap();
    assert!(resources.ui.tree().sole_node_with_key(key).is_none());
}
