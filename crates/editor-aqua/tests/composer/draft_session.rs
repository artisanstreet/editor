use artisan_api::{DraftSnapshot, DraftWriteResult};
use editor_aqua::composer::draft_body::DraftBody;
use editor_aqua::composer::draft_session::{DraftSession, Operation, Posting};
use serde_json::json;

fn typed(text: &str) -> DraftBody {
    DraftBody {
        text: text.to_owned(),
        ..DraftBody::default()
    }
}

fn type_in(session: &mut DraftSession, body: DraftBody) {
    let seen = session.epoch();
    session.queue(body, seen, false);
}

fn wire(body: &DraftBody) -> serde_json::Value {
    serde_json::to_value(body).unwrap()
}

#[test]
fn pending_shutdown_tracks_unsaved_edits_and_clears_only_after_save_or_invalidation() {
    let mut session = DraftSession::new("new".into());
    assert!(!session.pending());
    assert!(matches!(session.next_operation(), Some(Operation::Load)));
    assert!(!session.pending());
    type_in(&mut session, typed("edited during load"));
    assert!(session.pending());
    session.loaded(Ok(DraftSnapshot::default()));
    let Some(Operation::Write { .. }) = session.next_operation() else {
        panic!("expected a draft write");
    };
    session.written(Err(unanswered("offline")));
    assert!(session.pending());
    session.retry();
    assert!(matches!(session.next_operation(), Some(Operation::Load)));
    assert!(session.pending());
    session.loaded(Ok(DraftSnapshot::default()));
    let Some(Operation::Write { body: written, .. }) = session.next_operation() else {
        panic!("expected a retried draft write");
    };
    session.written(Ok(DraftWriteResult {
        saved: true,
        current: DraftSnapshot {
            revision: 1,
            body: wire(&written),
        },
    }));
    assert!(!session.pending());
    type_in(&mut session, typed("stale pairing"));
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
    assert!(matches!(session.next_operation(), Some(Operation::Load)));
    session.loaded(Ok(DraftSnapshot {
        revision: 1,
        body: json!({"text": "old"}),
    }));
    type_in(&mut session, typed("local"));
    let Some(Operation::Write { .. }) = session.next_operation() else {
        panic!("expected a draft write");
    };
    session.written(Ok(DraftWriteResult {
        saved: false,
        current: DraftSnapshot {
            revision: 2,
            body: json!({"text": "remote"}),
        },
    }));
    assert!(session.conflict());
    assert!(session.next_operation().is_none());
    session.keep_local();
    let Some(Operation::Write {
        expected_revision,
        body: written,
        ..
    }) = session.next_operation()
    else {
        panic!("expected a retry");
    };
    assert_eq!(expected_revision, 2);
    assert_eq!(*written, typed("local"));
}

#[test]
fn edits_during_a_write_are_not_discarded() {
    let mut session = DraftSession::new("thread:one".into());
    session.next_operation();
    session.loaded(Ok(DraftSnapshot::default()));
    type_in(&mut session, typed("first"));
    let Some(Operation::Write { body: written, .. }) = session.next_operation() else {
        panic!("expected a draft write");
    };
    type_in(&mut session, typed("second"));
    session.written(Ok(DraftWriteResult {
        saved: true,
        current: DraftSnapshot {
            revision: 1,
            body: wire(&written),
        },
    }));
    let Some(Operation::Write {
        expected_revision,
        body: written,
        ..
    }) = session.next_operation()
    else {
        panic!("expected the newer draft");
    };
    assert_eq!(expected_revision, 1);
    assert_eq!(*written, typed("second"));
}

#[test]
#[expect(
    clippy::too_many_lines,
    reason = "one host-replacement scenario walked step by step"
)]
fn replacing_same_host_id_does_not_reuse_old_composer_or_draft_session() {
    use artisan_api::tls::{Identity, fingerprint};
    use editor_aqua::composer::controller::{Composer, ComposerTarget};
    use editor_aqua::composer::view::ComposerAction;
    use editor_aqua::core::{Core, ForgeClient, HostConfig, HostId, HostKind};

    let directory = tempfile::tempdir().unwrap();
    let mut core = test_support::editor::core::test_core(&directory.path().join("editor"));
    let identity = Identity::generate("composer-test").unwrap();
    let fingerprint = fingerprint(identity.certificate_der().unwrap().as_ref());
    let forge = test_support::net::SilentListener::bind();
    let address = forge.address().to_string();
    let client = ForgeClient::new(&address, &fingerprint, &identity).unwrap();
    let id = HostId("same-host".into());
    let config = HostConfig {
        id: id.clone(),
        name: "Forge".into(),
        address,
        fingerprint,
        kind: HostKind::Local,
    };
    test_support::editor::core::connect_client(&mut core, &config, client.clone());
    let first = core.host(&id).unwrap().instance();
    let (_spool, mut sessions) = crate::draft_sessions();
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
    assert!(
        !owner
            .dispatch_with_change(&mut core, ComposerAction::SelectPane(None), None)
            .1
    );
    assert!(
        !owner
            .dispatch_with_change(&mut core, ComposerAction::HoverMention(999, true), None)
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
                        content: Some(content),
                        hovered: true,
                    },
                    None
                )
                .1
        );
        let deadline = owner.tooltip_deadline().unwrap();
        assert!(
            !owner.advance_tooltip(
                deadline
                    .checked_sub(std::time::Duration::from_millis(1))
                    .unwrap()
            )
        );
        assert!(owner.advance_tooltip(deadline));
        let tooltip = owner
            .tooltip_presentation(
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
    drop(old.session.as_mut().unwrap().drain_events());
    drop(other.drain_events());
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
            .map(|file| file.path.as_path().to_path_buf())
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
    test_support::editor::core::connect_client(&mut core, &config, client);
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
            .read(|session| session.body().clone()),
        DraftBody::default()
    );
    let editor = std::rc::Rc::clone(&current.editor);
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
    assert_eq!(current.files[0].path.as_path(), second_file);
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStringExt as _;
        let non_unicode = std::path::PathBuf::from(std::ffi::OsString::from_vec(
            b"/tmp/draft-\xff.txt".to_vec(),
        ));
        current.add_paths(vec![non_unicode]);
        assert_eq!(current.files.len(), 1);
        assert!(current.notice.as_ref().is_some_and(|notice| {
            notice.headline() == "This path can't be linked."
                && notice
                    .detail()
                    .is_some_and(|detail| detail.contains("is not a Unicode path"))
        }));
        current.save();
    }
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
    assert!(light.measure(EngineId::Codex, 4.0, 32.0, now, false));
    assert_eq!(light.frame(now), Some((4.0, false)));
    assert!(light.measure(EngineId::Codex, 8.0, 32.0, now, false));
    assert_eq!(light.frame(now), Some((8.0, false)));
    assert!(light.measure(EngineId::Claude, 44.0, 32.0, now, false));
    assert_eq!(light.frame(now), Some((8.0, true)));
    assert!(light.animating());
    assert_eq!(
        light.frame(now + Duration::from_millis(250)),
        Some((44.0, false))
    );
    assert!(!light.animating());
}

fn unanswered(check: &'static str) -> editor_aqua::core::client::CallError {
    test_support::editor::forge::inconsistent("/drafts/new", check)
}

fn not_sent() -> editor_aqua::composer::draft_session::PostMessageError {
    editor_aqua::composer::draft_session::PostMessageError::Send {
        thread: submission().thread,
        message: submission().message.id,
        source: unanswered("offline"),
    }
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
    let Some(Operation::Write {
        expected_revision,
        body: written,
    }) = session.next_operation()
    else {
        panic!("expected a draft write");
    };
    session.written(Ok(DraftWriteResult {
        saved: true,
        current: DraftSnapshot {
            revision: expected_revision + 1,
            body: wire(&written),
        },
    }));
}

fn send_again(session: &mut DraftSession) {
    let mut again = submission();
    again.message.id = artisan_api::ItemId::from("second-attempt");
    again.thread = artisan_api::ThreadId::from("second-thread");
    again.create.as_mut().unwrap().id = again.thread.clone();
    session.submit(again);
}

fn assert_original_submission(session: &mut DraftSession) {
    let Some(Operation::Post(request)) = session.next_operation() else {
        panic!("expected the original submission to resume");
    };
    let original = submission();
    assert_eq!(request.message.id, original.message.id);
    assert_eq!(request.message.text, original.message.text);
    assert_eq!(request.thread, original.thread);
    assert_eq!(request.create.unwrap().id, original.create.unwrap().id);
    session.posted(Ok(()), submission().thread);
    assert!(!session.sending());
    assert!(!session.pending());
    assert_eq!(*session.body(), DraftBody::default());
}

#[test]
fn draft_load_failure_hands_the_message_back_and_sending_it_again_keeps_its_identity() {
    let mut session = DraftSession::new("new:project".into());
    assert!(matches!(session.next_operation(), Some(Operation::Load)));
    let body = typed("Keep my draft");
    type_in(&mut session, body.clone());
    session.submit(submission());
    assert!(session.sending());
    session.loaded(Err(unanswered("offline")));
    assert!(!session.sending());
    assert_eq!(*session.body(), body);
    assert!(session.pending());
    assert!(session.next_operation().is_none());
    send_again(&mut session);
    assert!(session.sending());
    assert!(matches!(session.next_operation(), Some(Operation::Load)));
    session.loaded(Ok(DraftSnapshot::default()));
    save_next(&mut session);
    assert_original_submission(&mut session);
}

#[test]
fn draft_write_failure_hands_the_message_back_without_losing_the_draft() {
    let mut session = DraftSession::new("new:project".into());
    assert!(matches!(session.next_operation(), Some(Operation::Load)));
    session.loaded(Ok(DraftSnapshot::default()));
    let body = typed("Keep my draft");
    type_in(&mut session, body.clone());
    session.submit(submission());
    let Some(Operation::Write { .. }) = session.next_operation() else {
        panic!("expected a draft write");
    };
    session.written(Err(unanswered("offline")));
    assert!(!session.sending());
    assert_eq!(*session.body(), body);
    assert!(session.pending());
    send_again(&mut session);
    assert!(
        matches!(session.next_operation(), Some(Operation::Load)),
        "a save with no answer is checked against Forge before writing again"
    );
    session.loaded(Ok(DraftSnapshot::default()));
    assert!(!session.conflict());
    save_next(&mut session);
    assert_original_submission(&mut session);
}

#[test]
fn a_message_whose_draft_another_client_replaced_is_sent_and_their_draft_stays() {
    let mut session = DraftSession::new("new:project".into());
    assert!(matches!(session.next_operation(), Some(Operation::Load)));
    session.loaded(Ok(DraftSnapshot::default()));
    type_in(&mut session, typed("Keep my draft"));
    session.submit(submission());
    let Some(Operation::Write { .. }) = session.next_operation() else {
        panic!("expected a draft write");
    };
    session.written(Ok(DraftWriteResult {
        saved: false,
        current: DraftSnapshot {
            revision: 7,
            body: json!({"text": "other client"}),
        },
    }));
    assert!(
        !session.conflict(),
        "nothing but the sent message was in this client's draft"
    );
    assert!(
        session.sending(),
        "the message the user sent still goes out"
    );
    let Some(Operation::Post(request)) = session.next_operation() else {
        panic!("expected the message to be posted");
    };
    assert_eq!(request.message.id, submission().message.id);
    assert_eq!(
        request.message.draft, None,
        "the send doesn't clear the other client's draft"
    );
    session.posted(Ok(()), submission().thread);
    assert_eq!(*session.body(), typed("other client"));
    assert!(!session.pending(), "their draft is not written back");
}

#[test]
fn a_save_forge_answered_too_late_is_not_shown_as_another_clients_change() {
    let mut session = DraftSession::new("thread:one".into());
    session.next_operation();
    session.loaded(Ok(DraftSnapshot {
        revision: 7,
        body: json!({"text": "old"}),
    }));
    type_in(&mut session, typed("typed while Forge was slow"));
    let Some(Operation::Write { .. }) = session.next_operation() else {
        panic!("expected a draft write");
    };
    session.written(Err(unanswered("operation timed out")));
    type_in(&mut session, typed("typed while Forge was slow, then more"));
    assert!(
        matches!(session.next_operation(), Some(Operation::Load)),
        "the unanswered save is checked before the next one"
    );
    session.loaded(Ok(DraftSnapshot {
        revision: 8,
        body: json!({"text": "typed while Forge was slow"}),
    }));
    assert!(
        !session.conflict(),
        "the timed-out save landing late is this client's own draft"
    );
    assert!(session.error().is_none(), "no error is shown for it");
    let Some(Operation::Write {
        expected_revision,
        body: written,
        ..
    }) = session.next_operation()
    else {
        panic!("expected the newest text to be saved on top");
    };
    assert_eq!(expected_revision, 8, "it saves on top of its own write");
    assert_eq!(
        *written,
        typed("typed while Forge was slow, then more"),
        "the newest text is what gets saved"
    );
}

#[test]
fn repeated_unanswered_saves_never_pile_up() {
    let mut session = DraftSession::new("thread:one".into());
    session.next_operation();
    session.loaded(Ok(DraftSnapshot {
        revision: 3,
        body: json!({"text": "old"}),
    }));
    type_in(&mut session, typed("mine"));
    for _ in 0..3 {
        let Some(Operation::Write {
            expected_revision, ..
        }) = session.next_operation()
        else {
            panic!("expected a draft write");
        };
        assert_eq!(expected_revision, 3);
        session.written(Err(unanswered("offline")));
        session.retry();
        assert!(matches!(session.next_operation(), Some(Operation::Load)));
        session.loaded(Ok(DraftSnapshot {
            revision: 3,
            body: json!({"text": "old"}),
        }));
        assert!(!session.conflict());
    }
}

#[test]
fn another_clients_change_after_a_timed_out_save_still_asks_which_to_keep() {
    let mut session = DraftSession::new("thread:one".into());
    session.next_operation();
    session.loaded(Ok(DraftSnapshot {
        revision: 7,
        body: json!({"text": "old"}),
    }));
    type_in(&mut session, typed("mine"));
    let Some(Operation::Write { .. }) = session.next_operation() else {
        panic!("expected a draft write");
    };
    session.written(Err(unanswered("operation timed out")));
    type_in(&mut session, typed("mine, edited"));
    assert!(matches!(session.next_operation(), Some(Operation::Load)));
    session.loaded(Ok(DraftSnapshot {
        revision: 8,
        body: json!({"text": "written on the other machine"}),
    }));
    assert!(
        session.conflict(),
        "a draft this client never sent is another client's change"
    );
    assert_eq!(*session.body(), typed("mine, edited"));
}

#[test]
fn typing_before_the_first_load_asks_which_draft_to_keep() {
    let mut session = DraftSession::new("thread:one".into());
    assert!(matches!(session.next_operation(), Some(Operation::Load)));
    type_in(&mut session, typed("typed before the draft arrived"));
    session.loaded(Ok(DraftSnapshot {
        revision: 4,
        body: json!({"text": "saved on another machine"}),
    }));
    assert!(
        session.conflict(),
        "the saved draft would be overwritten unseen"
    );
    assert!(session.next_operation().is_none());
    assert_eq!(*session.body(), typed("typed before the draft arrived"));
    session.use_remote();
    assert_eq!(*session.body(), typed("saved on another machine"));
}

#[test]
fn typing_before_the_first_load_of_an_empty_draft_just_saves() {
    let mut session = DraftSession::new("thread:one".into());
    assert!(matches!(session.next_operation(), Some(Operation::Load)));
    type_in(&mut session, typed("typed early"));
    session.loaded(Ok(DraftSnapshot {
        revision: 2,
        body: serde_json::Value::Null,
    }));
    assert!(!session.conflict());
    let Some(Operation::Write {
        expected_revision,
        body: written,
        ..
    }) = session.next_operation()
    else {
        panic!("expected the early text to be saved");
    };
    assert_eq!(expected_revision, 2);
    assert_eq!(*written, typed("typed early"));
}

#[test]
fn the_message_posts_before_a_draft_typed_after_it_is_saved() {
    let mut session = DraftSession::new("new:project".into());
    assert!(matches!(session.next_operation(), Some(Operation::Load)));
    session.loaded(Ok(DraftSnapshot::default()));
    type_in(&mut session, typed("Keep my draft"));
    session.submit(submission());
    save_next(&mut session);
    type_in(&mut session, typed("the next message"));
    let Some(Operation::Post(request)) = session.next_operation() else {
        panic!("expected the message to post before the new draft is saved");
    };
    assert_eq!(request.message.text, "Keep my draft");
}

#[test]
fn a_draft_typed_while_the_message_posts_survives_its_acceptance() {
    let mut session = DraftSession::new("new:project".into());
    assert!(matches!(session.next_operation(), Some(Operation::Load)));
    session.loaded(Ok(DraftSnapshot::default()));
    type_in(&mut session, typed("Keep my draft"));
    session.submit(submission());
    save_next(&mut session);
    assert!(matches!(session.next_operation(), Some(Operation::Post(_))));
    let typed = typed("the next message");
    type_in(&mut session, typed.clone());
    session.posted(Ok(()), submission().thread);
    assert!(!session.sending());
    assert_eq!(*session.body(), typed);
    assert!(session.pending());
    assert!(matches!(session.next_operation(), Some(Operation::Load)));
    session.loaded(Ok(DraftSnapshot {
        revision: 2,
        body: serde_json::Value::Null,
    }));
    let Some(Operation::Write {
        expected_revision,
        body: written,
        ..
    }) = session.next_operation()
    else {
        panic!("expected the typed draft to be saved");
    };
    assert_eq!(*written, typed);
    assert_eq!(expected_revision, 2);
}

#[test]
fn a_failed_post_reports_the_failure_and_keeps_the_messages_identity_for_sending_again() {
    let mut session = DraftSession::new("new:project".into());
    assert!(matches!(session.next_operation(), Some(Operation::Load)));
    session.loaded(Ok(DraftSnapshot::default()));
    type_in(&mut session, typed("Keep my draft"));
    session.submit(submission());
    save_next(&mut session);
    assert!(matches!(session.next_operation(), Some(Operation::Post(_))));
    assert!(!session.send_failed());
    session.posted(Err(not_sent()), submission().thread);
    assert!(session.send_failed());
    assert!(!session.sending());
    session.retry();
    assert!(
        session.next_operation().is_none(),
        "Try again doesn't send behind the user's back"
    );
    send_again(&mut session);
    assert!(!session.send_failed());
    assert_original_submission(&mut session);
}

#[test]
fn posting_follows_a_new_thread_send_from_queued_to_accepted() {
    let mut session = DraftSession::new("new:project".into());
    assert!(matches!(session.next_operation(), Some(Operation::Load)));
    session.loaded(Ok(DraftSnapshot::default()));
    assert_eq!(session.posting(), Posting::Idle);
    type_in(&mut session, typed("Keep my draft"));
    session.submit(submission());
    let thread = submission().thread;
    assert_eq!(session.posting(), Posting::Sending(thread.clone()));
    save_next(&mut session);
    assert!(matches!(session.next_operation(), Some(Operation::Post(_))));
    session.posted(Err(not_sent()), thread.clone());
    assert_eq!(session.posting(), Posting::Failed);
    send_again(&mut session);
    assert_eq!(session.posting(), Posting::Sending(thread.clone()));
    assert!(matches!(session.next_operation(), Some(Operation::Post(_))));
    session.posted(Ok(()), thread.clone());
    assert_eq!(session.posting(), Posting::Accepted(thread));
    session.submit(submission());
    assert!(matches!(session.posting(), Posting::Sending(_)));
}
