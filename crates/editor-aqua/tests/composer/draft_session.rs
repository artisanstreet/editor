use artisan_api::{DraftSnapshot, DraftWriteResult};
use editor_aqua::composer::draft_session::{DraftSession, Operation};
use serde_json::json;

#[test]
fn pending_shutdown_tracks_unsaved_edits_and_clears_only_after_save_or_invalidation() {
    let mut session = DraftSession::new("new".into());
    assert!(!session.pending());
    assert!(matches!(session.next(), Some(Operation::Load)));
    assert!(!session.pending());
    session.queue(json!({"text": "edited during load"}));
    assert!(session.pending());
    session.loaded(Ok(DraftSnapshot::default()));
    let Some(Operation::Write { request, edited }) = session.next() else {
        panic!("expected a draft write");
    };
    session.written(&request, edited, Err("offline".into()));
    assert!(session.pending());
    session.retry();
    let Some(Operation::Write { request, edited }) = session.next() else {
        panic!("expected a retried draft write");
    };
    session.written(
        &request,
        edited,
        Ok(DraftWriteResult {
            saved: true,
            current: DraftSnapshot {
                revision: 1,
                body: request.body.clone(),
            },
        }),
    );
    assert!(!session.pending());
    session.queue(json!({"text": "stale pairing"}));
    assert!(session.pending());
    session.remove();
    assert!(!session.pending());
}

#[test]
fn picker_tooltip_delay_and_retarget_cancel_stale_deadline() {
    use editor_aqua::composer::picker::{PickerTooltip, TooltipContent, TooltipPlacement};
    use std::time::{Duration, Instant};
    let now = Instant::now();
    let item = |anchor: &str| TooltipContent {
        anchor: anchor.to_owned(),
        advisory: None,
        text: anchor.to_owned(),
        placement: TooltipPlacement::Beside,
    };
    let mut tooltip = PickerTooltip::default();
    tooltip.begin(item("first"), now);
    assert!(!tooltip.show_due(now + Duration::from_millis(499)));
    tooltip.begin(item("second"), now + Duration::from_millis(250));
    tooltip.clear_for("first");
    assert!(!tooltip.show_due(now + Duration::from_millis(500)));
    assert!(tooltip.show_due(now + Duration::from_millis(750)));
    assert_eq!(tooltip.shown().unwrap().anchor, "second");
    tooltip.clear_for("second");
    assert!(tooltip.shown().is_none());
    assert!(tooltip.deadline().is_none());
}

#[test]
fn concurrent_draft_conflict_requires_an_explicit_resolution() {
    let mut session = DraftSession::new("thread:one".into());
    assert!(matches!(session.next(), Some(Operation::Load)));
    session.loaded(Ok(DraftSnapshot {
        revision: 1,
        body: json!({"text": "old"}),
    }));
    session.queue(json!({"text": "local"}));
    let Some(Operation::Write { request, edited }) = session.next() else {
        panic!("expected a draft write");
    };
    session.written(
        &request,
        edited,
        Ok(DraftWriteResult {
            saved: false,
            current: DraftSnapshot {
                revision: 2,
                body: json!({"text": "remote"}),
            },
        }),
    );
    assert!(session.conflict);
    assert!(session.next().is_none());
    session.keep_local();
    let Some(Operation::Write { request, .. }) = session.next() else {
        panic!("expected a retry");
    };
    assert_eq!(request.expected_revision, 2);
    assert_eq!(request.body, json!({"text": "local"}));
}

#[test]
fn edits_during_a_write_are_not_discarded() {
    let mut session = DraftSession::new("thread:one".into());
    session.next();
    session.loaded(Ok(DraftSnapshot::default()));
    session.queue(json!({"text": "first"}));
    let Some(Operation::Write { request, edited }) = session.next() else {
        panic!("expected a draft write");
    };
    session.queue(json!({"text": "second"}));
    session.written(
        &request,
        edited,
        Ok(DraftWriteResult {
            saved: true,
            current: DraftSnapshot {
                revision: 1,
                body: request.body.clone(),
            },
        }),
    );
    let Some(Operation::Write { request, .. }) = session.next() else {
        panic!("expected the newer draft");
    };
    assert_eq!(request.expected_revision, 1);
    assert_eq!(request.body, json!({"text": "second"}));
}

#[test]
fn replacing_same_host_id_does_not_reuse_old_composer_or_draft_session() {
    use artisan_api::tls::{Identity, fingerprint};
    use editor_aqua::composer::{
        controller::{Composer, ComposerTarget},
        persistence::DraftSessions,
    };
    use editor_aqua::core::{Core, EditorFiles, ForgeClient, HostConfig, HostId, HostKind};

    let directory = tempfile::tempdir().unwrap();
    let mut core = Core::for_tests(EditorFiles {
        dir: directory.path().join("editor"),
    })
    .unwrap();
    let identity = Identity::generate("composer-test").unwrap();
    let fingerprint = fingerprint(identity.certificate_der().unwrap().as_ref());
    let client = ForgeClient::new("127.0.0.1:1", &fingerprint, &identity).unwrap();
    let id = HostId("same-host".into());
    let config = HostConfig {
        id: id.clone(),
        name: "Forge".into(),
        address: "127.0.0.1:1".into(),
        fingerprint,
        kind: HostKind::Local,
    };
    core.add_test_host(&config, client.clone());
    let first = core.host(&id).unwrap().instance();
    let mut sessions = DraftSessions::default();
    let target = ComposerTarget::NewThread {
        host: id.clone(),
        project: None,
    };
    let mut owner = editor_aqua::composer::view::ComposerOwner::new(
        &core,
        &mut sessions,
        target.clone(),
        std::rc::Rc::new(|_| {}),
    );
    use editor_aqua::composer::view::ComposerAction;
    assert!(
        !owner
            .dispatch_with_change(&mut core, ComposerAction::SelectPane(None), None)
            .1
    );
    assert!(
        !owner
            .dispatch_with_change(&mut core, ComposerAction::HoverMention(999), None)
            .1
    );
    assert!(
        owner
            .dispatch_with_change(&mut core, ComposerAction::ToggleProject, None)
            .1
    );
    assert!(
        owner
            .dispatch_with_change(&mut core, ComposerAction::ClosePicker, None)
            .1
    );
    assert!(
        !owner
            .dispatch_with_change(&mut core, ComposerAction::ClosePicker, None)
            .1
    );
    {
        use editor_aqua::core::DraggedThread;
        let thread = |host: &HostId, title: &str| DraggedThread {
            host: host.clone(),
            thread: artisan_api::ThreadId("t".into()),
            title: title.into(),
        };
        owner
            .controller
            .borrow()
            .editor
            .borrow_mut()
            .set_text("see");
        owner.dispatch_with_change(
            &mut core,
            ComposerAction::DroppedThread(thread(&HostId("other".into()), "Fix")),
            None,
        );
        assert_eq!(owner.controller.borrow().editor.borrow().text(), "see");
        let (_, changed) = owner.dispatch_with_change(
            &mut core,
            ComposerAction::DroppedThread(thread(&id, "Fix")),
            None,
        );
        assert!(changed);
        assert_eq!(
            owner.controller.borrow().editor.borrow().text(),
            "see @Fix "
        );
    }
    {
        use editor_aqua::ui::text::mount::EditorEvent;
        let type_text = |owner: &mut editor_aqua::composer::view::ComposerOwner,
                         core: &mut Core,
                         text: &str| {
            owner.controller.borrow().editor.borrow_mut().set_text(text);
            owner
                .dispatch_with_change(core, ComposerAction::Editor(EditorEvent::Changed), None)
                .1
        };
        type_text(&mut owner, &mut core, "");
        assert!(type_text(&mut owner, &mut core, "h"));
        assert!(!type_text(&mut owner, &mut core, "he"));
        assert!(!type_text(&mut owner, &mut core, "hel"));
        assert!(type_text(&mut owner, &mut core, ""));
    }
    {
        use editor_aqua::composer::picker::{Menu, TooltipContent, TooltipPlacement};
        use editor_aqua::ui::theme::{ArtisanTheme, ThemeMode};
        owner.controller.borrow_mut().picker.menu = Some(Menu::Model);
        let generation = owner.controller.borrow().generation();
        let content = TooltipContent {
            anchor: "composer-engine-codex".into(),
            advisory: None,
            text: "Unavailable".into(),
            placement: TooltipPlacement::Above,
        };
        assert!(
            owner
                .dispatch_with_change(
                    &mut core,
                    ComposerAction::TooltipHover {
                        instance: first,
                        generation,
                        anchor: content.anchor.clone(),
                        content: Some(content.clone()),
                        hovered: true,
                    },
                    None
                )
                .1
        );
        let deadline = owner.tooltip_deadline().unwrap();
        assert!(!owner.advance_tooltip(deadline - std::time::Duration::from_millis(1)));
        assert!(owner.advance_tooltip(deadline));
        let tooltip = owner
            .tooltip_presentation(
                *ArtisanTheme::for_mode(ThemeMode::Dark),
                aqua_core::Rect {
                    x: 10.0,
                    y: 30.0,
                    w: 32.0,
                    h: 32.0,
                },
                aqua_core::Size::new(400.0, 300.0),
            )
            .unwrap();
        assert_eq!(tooltip.side, aqua_ui::overlay::Side::Above);
        assert!(tooltip.snap_to_window);
        owner.dispatch_with_change(&mut core, ComposerAction::ClosePicker, None);
        assert!(owner.shown_tooltip().is_none());
    }
    {
        use aqua_ui::element::Box as UiBox;
        use editor_aqua::composer::view::PopupKind;
        let mut app = aqua::UiApp::new(aqua_text::SharedFonts::new(), aqua_text::SharedRuns::new());
        app.mount(|_| {
            UiBox::new()
                .width(400.0)
                .height(300.0)
                .child(
                    UiBox::new()
                        .width(80.0)
                        .height(30.0)
                        .build()
                        .with_key("composer-anchor")
                        .focusable(true),
                )
                .build()
        })
        .unwrap();
        app.set_viewport(400.0, 300.0, 1.0);
        app.build_frame().unwrap();
        assert!(app.focus_key("composer-anchor"));
        let anchor = app.tree().focused().unwrap();
        let panel = || UiBox::new().width(120.0).height(80.0).build();
        owner
            .open_popup_content(&mut app, anchor, PopupKind::Model, panel(), 0.0)
            .unwrap();
        let root = owner.popup_root(PopupKind::Model, &app).unwrap();
        owner.advance_popups(&mut app, 0.11).unwrap();
        owner.close_popup(&mut app, PopupKind::Model, 0.2).unwrap();
        owner.advance_popups(&mut app, 0.25).unwrap();
        let before = app.tree().paint(root).unwrap();
        owner
            .open_popup_content(&mut app, anchor, PopupKind::Model, panel(), 0.25)
            .unwrap();
        assert_eq!(owner.popup_root(PopupKind::Model, &app), Some(root));
        let after = app.tree().paint(root).unwrap();
        assert_eq!(before.opacity, after.opacity);
        assert_eq!(before.transform, after.transform);
        owner.advance_popups(&mut app, 0.36).unwrap();
        assert!(owner.popup_is_open(PopupKind::Model, &app));
        owner.dispose_popups(&mut app);
    }
    let mut old = Composer::new(&core, &mut sessions, target.clone());
    let mut other = sessions
        .open(id.clone(), first, target.scope(), client.clone())
        .unwrap();
    let _ = old.session.as_mut().unwrap().drain_events();
    let _ = other.drain_events();
    old.editor.borrow_mut().set_text("old draft");
    old.edited();
    assert!(
        old.session
            .as_mut()
            .unwrap()
            .drain_events()
            .iter()
            .any(|event| matches!(
                event,
                editor_aqua::composer::persistence::SessionEvent::Changed
            ))
    );
    assert!(other.drain_events().iter().any(|event| matches!(
        event,
        editor_aqua::composer::persistence::SessionEvent::Changed
    )));
    let generation = owner.controller.borrow().generation();
    let paths = vec![
        directory.path().join("first.txt"),
        directory.path().join("second.txt"),
    ];
    assert!(
        owner
            .dispatch_with_change(
                &mut core,
                ComposerAction::DroppedPaths {
                    paths: paths.clone(),
                    instance: first,
                    generation,
                },
                None
            )
            .1
    );
    assert_eq!(
        owner
            .controller
            .borrow()
            .files
            .iter()
            .map(|file| file.path.clone())
            .collect::<Vec<_>>(),
        paths
    );
    assert!(
        !owner
            .dispatch_with_change(
                &mut core,
                ComposerAction::DroppedPaths {
                    paths: vec![directory.path().join("stale.txt")],
                    instance: first,
                    generation: generation.wrapping_add(1),
                },
                None
            )
            .1
    );
    core.remove_host(&id).unwrap();
    core.add_test_host(&config, client);
    assert!(
        !owner
            .dispatch_with_change(&mut core, ComposerAction::ToggleProject, None)
            .1
    );
    let assets = aqua::images::ImageAssets::default();
    owner.drain_async(&core, &assets);
    let drained = owner.drain_async(&core, &assets);
    assert!(!drained.changed);
    assert!(drained.events.is_empty());
    assert!(drained.effects.is_empty());
    assert_ne!(core.host(&id).unwrap().instance(), first);
    old.sync(&core);
    assert!(old.session.is_none());
    let mut current = Composer::new(&core, &mut sessions, target);
    assert_eq!(
        current
            .session
            .as_ref()
            .unwrap()
            .read(|session| session.body.clone()),
        serde_json::Value::Null
    );
    let editor = current.editor.clone();
    current.editor.borrow_mut().replace(0..0, "unsent");
    current.edited();
    current.set_target(
        &core,
        &mut sessions,
        ComposerTarget::Thread {
            host: id,
            thread: artisan_api::ThreadId::new(),
        },
    );
    assert!(std::rc::Rc::ptr_eq(&editor, &current.editor));
    assert_eq!(current.editor.borrow().text(), "");
    assert!(!current.editor.borrow_mut().undo());
    let first_file = directory.path().join("first.txt");
    let second_file = directory.path().join("second.txt");
    current.add_paths(vec![first_file.clone(), second_file.clone(), first_file]);
    assert_eq!(current.files.len(), 2);
    let removed = current.files[0].key;
    assert_eq!(current.file_shake.map(|(key, _)| key), Some(removed));
    current.remove_file(removed);
    current.remove_file(removed);
    assert_eq!(current.files.len(), 1);
    assert_eq!(current.files[0].path, second_file);
}

#[test]
fn strip_prioritizes_parked_then_unclosed_note_then_goal() {
    use editor_aqua::composer::goal_strip::{
        ComposerGoal, ComposerGoalState, ComposerNote, StripState,
    };
    use std::time::{Duration, Instant};
    let now = Instant::now();
    let mut strip = StripState::default();
    strip.set_goal(
        Some(ComposerGoal {
            objective: "Goal".into(),
            state: ComposerGoalState::Active,
            detail: None,
        }),
        now,
        false,
    );
    strip.set_note(
        Some(ComposerNote {
            id: "one".into(),
            heads_up: true,
            text: "Note".into(),
        }),
        now,
        false,
    );
    assert_eq!(
        strip.motion.frame(now, false).unwrap().goal.state,
        ComposerGoalState::HeadsUp
    );
    strip.set_parked(Some(123), |_| "15:00".into(), now, false);
    assert_eq!(
        strip.motion.frame(now, false).unwrap().goal.objective,
        "This thread will pick up at 15:00"
    );
    strip.set_parked(None, |_| unreachable!(), now, false);
    strip.close_note(now, false);
    assert_eq!(
        strip.motion.frame(now, false).unwrap().goal.objective,
        "Goal"
    );
    strip.settle();
    strip.set_goal(None, now, false);
    assert!(
        strip
            .motion
            .frame(now + Duration::from_millis(499), false)
            .is_some()
    );
    assert!(
        strip
            .motion
            .frame(now + Duration::from_millis(500), false)
            .is_none()
    );
}

#[test]
fn special_command_token_is_atomic_and_only_recognized_at_start() {
    use editor_aqua::composer::mentions::Mentions;
    use editor_aqua::ui::text::editor::{Editor, Mode};
    use editor_aqua::ui::theme::ArtisanTheme;
    let mentions = Mentions::default();
    let tokenizer = mentions.tokenizer(ArtisanTheme::for_mode(
        editor_aqua::ui::theme::ThemeMode::Dark,
    ));
    let mut editor = Editor::new("/goal ship", Mode::Prompt);
    editor.set_styled_tokenizer(Some(tokenizer));
    editor.set_selection(5..5, false);
    editor.delete_toward(false, false);
    assert_eq!(editor.text(), " ship");
    assert!(
        mentions.tokenizer(ArtisanTheme::for_mode(
            editor_aqua::ui::theme::ThemeMode::Dark
        ))("see /goal later")
        .is_empty()
    );
}

#[test]
fn axis_keyboard_moves_from_hover_or_selection_and_excludes_headers() {
    use editor_aqua::composer::picker::{AxisCursor, AxisKey, AxisKeyResult, PickerState};
    let mut picker = PickerState::default();
    assert_eq!(picker.axis_key(AxisKey::Down, 1, 3), AxisKeyResult::Moved);
    assert_eq!(picker.keyboard_axis_cursor(), Some(2));
    assert_eq!(picker.axis_key(AxisKey::Down, 1, 3), AxisKeyResult::Moved);
    assert_eq!(picker.keyboard_axis_cursor(), Some(2));
    picker.axis_cursor = Some(AxisCursor::Pointer(1));
    assert_eq!(picker.keyboard_axis_cursor(), None);
    assert_eq!(
        picker.axis_key(AxisKey::Accept, 0, 3),
        AxisKeyResult::Choose(1)
    );
    assert_eq!(picker.axis_key(AxisKey::Home, 0, 3), AxisKeyResult::Moved);
    assert_eq!(picker.keyboard_axis_cursor(), Some(0));
    assert_eq!(picker.axis_key(AxisKey::End, 0, 0), AxisKeyResult::Ignored);
}

#[test]
fn engine_light_flies_on_tab_change_but_not_on_remeasurement() {
    use artisan_api::EngineId;
    use editor_aqua::composer::engine_light::EngineLight;
    use std::time::{Duration, Instant};
    let now = Instant::now();
    let mut light = EngineLight::default();
    assert!(light.measure(EngineId::Codex, 4.0, 32.0, now));
    assert_eq!(light.frame(now), Some((4.0, false)));
    assert!(light.measure(EngineId::Codex, 8.0, 32.0, now));
    assert_eq!(light.frame(now), Some((8.0, false)));
    assert!(light.measure(EngineId::Claude, 44.0, 32.0, now));
    assert_eq!(light.frame(now), Some((8.0, true)));
    assert!(light.animating());
    assert_eq!(
        light.frame(now + Duration::from_millis(250)),
        Some((44.0, false))
    );
    assert!(!light.animating());
}

fn submission() -> editor_aqua::composer::draft_session::Submission {
    let thread = artisan_api::ThreadId::from("new-thread");
    editor_aqua::composer::draft_session::Submission {
        create: Some(artisan_api::CreateThread {
            id: thread.clone(),
            project_id: artisan_api::ProjectId::from("project"),
            config: artisan_api::EngineConfig::new(artisan_api::EngineId::Codex),
            title: None,
            parent_thread_id: None,
            spawned_by: None,
        }),
        thread,
        message: artisan_api::SendMessage {
            id: artisan_api::ItemId::from("original-message"),
            text: "Keep my draft".into(),
            images: Vec::new(),
            config: None,
            draft: None,
        },
    }
}

fn save_next(session: &mut DraftSession) {
    let Some(Operation::Write { request, edited }) = session.next() else {
        panic!("expected a draft write");
    };
    session.written(
        &request,
        edited,
        Ok(DraftWriteResult {
            saved: true,
            current: DraftSnapshot {
                revision: request.expected_revision + 1,
                body: request.body.clone(),
            },
        }),
    );
}

fn assert_original_submission(session: &mut DraftSession) {
    let Some(Operation::Post(request)) = session.next() else {
        panic!("expected the original submission to resume");
    };
    let original = submission();
    assert_eq!(request.message.id, original.message.id);
    assert_eq!(request.message.text, original.message.text);
    assert_eq!(request.thread, original.thread);
    assert_eq!(request.create.unwrap().id, original.create.unwrap().id);
    session.posted(Ok(()));
    assert!(!session.sending());
    assert!(!session.pending());
    assert!(session.body.is_null());
}

#[test]
fn draft_load_failure_unlocks_the_queued_send_and_retry_preserves_its_identity() {
    let mut session = DraftSession::new("new:project".into());
    assert!(matches!(session.next(), Some(Operation::Load)));
    let body = json!({"text": "Keep my draft"});
    session.queue(body.clone());
    session.submit(submission());
    assert!(session.sending());
    session.loaded(Err("offline".into()));
    assert!(!session.sending());
    assert_eq!(session.body, body);
    assert!(session.pending());
    assert!(session.next().is_none());
    session.retry();
    assert!(session.sending());
    assert!(matches!(session.next(), Some(Operation::Load)));
    session.loaded(Ok(DraftSnapshot::default()));
    save_next(&mut session);
    assert_original_submission(&mut session);
}

#[test]
fn draft_write_failure_unlocks_the_queued_send_without_losing_the_draft() {
    let mut session = DraftSession::new("new:project".into());
    session.loaded(Ok(DraftSnapshot::default()));
    let body = json!({"text": "Keep my draft"});
    session.queue(body.clone());
    session.submit(submission());
    let Some(Operation::Write { request, edited }) = session.next() else {
        panic!("expected a draft write");
    };
    session.written(&request, edited, Err("offline".into()));
    assert!(!session.sending());
    assert_eq!(session.body, body);
    assert!(session.pending());
    session.retry();
    save_next(&mut session);
    assert_original_submission(&mut session);
}

#[test]
fn a_submitted_draft_conflict_unlocks_and_resumes_only_after_resolution() {
    let mut session = DraftSession::new("new:project".into());
    session.loaded(Ok(DraftSnapshot::default()));
    let body = json!({"text": "Keep my draft"});
    session.queue(body.clone());
    session.submit(submission());
    let Some(Operation::Write { request, edited }) = session.next() else {
        panic!("expected a draft write");
    };
    session.written(
        &request,
        edited,
        Ok(DraftWriteResult {
            saved: false,
            current: DraftSnapshot {
                revision: 7,
                body: json!({"text": "other client"}),
            },
        }),
    );
    assert!(!session.sending());
    assert_eq!(session.body, body);
    assert!(session.conflict);
    session.retry();
    assert!(!session.sending());
    assert!(session.next().is_none());
    session.keep_local();
    assert!(session.sending());
    save_next(&mut session);
    assert_original_submission(&mut session);
}
