use artisan_api::ContentRef;
use axum::http::StatusCode;
use editor_aqua::{
    core::{Core, HostConfig, RangeState},
    sources::ContentText,
};
use std::time::Duration;
use test_support::editor::core::{Delivered, connect, stub_host};
use test_support::editor::forge::StubForge;

fn forge() -> StubForge {
    editor_aqua::core::runtime::runtime().block_on(StubForge::serve(
        test_support::editor::forge::empty_state(),
        axum::Router::new(),
    ))
}

fn connected(core: &mut Core, forge: &StubForge) -> HostConfig {
    let config = stub_host("same-host", "Forge", forge);
    connect(core, &config);
    config
}

fn replace(core: &mut Core, config: &HostConfig) {
    core.remove_host(&config.id)
        .expect("the replaced host is removed");
    connect(core, config);
}

fn pending_copy(core: &Core, config: &HostConfig, forge: &StubForge) -> ContentText {
    let source = ContentRef::from_text("stored content");
    let path = format!("/content/{}", source.id);
    let mut view = ContentText::new(
        core.host(&config.id).expect("the test host is configured"),
        source,
    );
    let delivered = Delivered::watch(view.action_wake());
    view.copy(core);
    for _ in ["the source window", "the copy"] {
        forge.take("GET", &path).fail(
            StatusCode::SERVICE_UNAVAILABLE,
            "internal",
            "source unavailable",
        );
    }
    delivered.wait("the source copy");
    view
}

#[test]
fn copy_and_range_completions_are_discarded_after_removal_replacement_and_a_source_round_trip() {
    let directory = tempfile::tempdir().unwrap();
    let mut core = test_support::editor::core::test_core(directory.path());
    let forge = forge();
    let config = connected(&mut core, &forge);

    let mut current = pending_copy(&core, &config, &forge);
    assert!(current.drain(&core).is_none());
    assert!(
        current
            .error()
            .is_some_and(|error| error.starts_with("Couldn't copy source:"))
    );

    let mut removed = pending_copy(&core, &config, &forge);
    let old = core.host(&config.id).unwrap().instance();
    core.remove_host(&config.id).unwrap();
    assert!(removed.drain(&core).is_none());
    assert!(removed.error().is_none());
    assert!(
        matches!(removed.window().state(), RangeState::Failed(error) if error == "This Forge is no longer connected.")
    );

    connect(&mut core, &config);
    assert_ne!(old, core.host(&config.id).unwrap().instance());
    let mut replaced = pending_copy(&core, &config, &forge);
    replace(&mut core, &config);
    assert!(replaced.drain(&core).is_none());
    assert!(replaced.error().is_none());
    assert!(
        matches!(replaced.window().state(), RangeState::Failed(error) if error == "This Forge is no longer connected.")
    );

    let mut switched = pending_copy(&core, &config, &forge);
    switched.set_source(ContentRef::from_text("other content"));
    switched.set_source(ContentRef::from_text("stored content"));
    switched.copy(&core);
    assert!(switched.drain(&core).is_none());
    assert!(
        switched.busy(),
        "a copy that finished before switching away and back must not end the copy started after it"
    );
    assert!(switched.error().is_none());
}

#[test]
fn collection_actions_do_not_reach_a_recreated_host() {
    use artisan_api::{CollectionRange, CollectionRef};
    use editor_aqua::sources::{CollectionBrowser, CollectionPreview};
    use std::cell::Cell;
    let directory = tempfile::tempdir().unwrap();
    let mut core = test_support::editor::core::test_core(directory.path());
    let forge = forge();
    let config = connected(&mut core, &forge);
    let source = CollectionRef {
        source: ContentRef::from_text("entry"),
        count: 1,
    };
    let page = CollectionRange {
        source: source.source.clone(),
        start: 0,
        end: 1,
        total: 1,
        records: vec!["entry".to_owned()],
    };
    let browser = CollectionBrowser::<String>::new(core.host(&config.id).unwrap(), source);
    let activated = Cell::new(false);
    let present = |_: &String, _: u64| CollectionPreview {
        label: "entry".into(),
        detail: None,
        selected: false,
        disabled: false,
    };
    assert!(browser.activate(&core, &page, 0, present, |_, _| activated.set(true)));
    activated.set(false);
    core.remove_host(&config.id).unwrap();
    assert!(!browser.activate(&core, &page, 0, present, |_, _| activated.set(true)));
    connect(&mut core, &config);
    assert!(!browser.activate(&core, &page, 0, present, |_, _| activated.set(true)));
    assert!(!activated.get());
}

#[test]
fn late_save_dialog_cannot_export_a_replaced_source_or_host() {
    let directory = tempfile::tempdir().unwrap();
    let mut core = test_support::editor::core::test_core(directory.path());
    let forge = forge();
    let config = connected(&mut core, &forge);
    let mut view = ContentText::new(
        core.host(&config.id).unwrap(),
        ContentRef::from_text("first"),
    );
    let old_picker = view.begin_export(&core).unwrap();
    assert!(view.busy());
    view.set_source(ContentRef::from_text("second"));
    let current_picker = view.begin_export(&core).unwrap();
    let path = directory.path().join("source.txt");
    view.finish_export(&core, old_picker, Some(path.clone()));
    assert!(view.busy());
    assert!(!path.exists());
    replace(&mut core, &config);
    view.finish_export(&core, current_picker, Some(path.clone()));
    assert!(!view.busy());
    assert!(!path.exists());
}

#[test]
fn picker_failures_only_apply_to_the_current_source_and_host() {
    let directory = tempfile::tempdir().unwrap();
    let mut core = test_support::editor::core::test_core(directory.path());
    let forge = forge();
    let config = connected(&mut core, &forge);
    let mut view = ContentText::new(
        core.host(&config.id).unwrap(),
        ContentRef::from_text("first"),
    );
    let first = view.begin_export(&core).unwrap();
    view.finish_export_result(&core, first, Err("picker failed".into()));
    assert_eq!(view.error(), Some("Couldn't export source: picker failed"));
    let old = view.begin_export(&core).unwrap();
    view.set_source(ContentRef::from_text("second"));
    let current = view.begin_export(&core).unwrap();
    view.finish_export_result(&core, old, Err("old picker".into()));
    assert!(view.error().is_none());
    assert!(view.busy());
    view.finish_export_result(&core, current, Ok(None));
    assert!(view.error().is_none());
    assert!(!view.busy());
    let replaced = view.begin_export(&core).unwrap();
    replace(&mut core, &config);
    view.finish_export_result(&core, replaced, Err("removed host".into()));
    assert!(view.error().is_none());
    assert!(!view.busy());
}

#[test]
fn source_change_reporting_rejects_noops_and_keeps_picker_effects() {
    use editor_aqua::sources::{ContentAction, ContentEffect, dispatch_content_changed};
    let directory = tempfile::tempdir().unwrap();
    let mut core = test_support::editor::core::test_core(directory.path());
    let forge = forge();
    let config = connected(&mut core, &forge);
    let mut view = ContentText::new(
        core.host(&config.id).unwrap(),
        ContentRef::from_text("source"),
    );
    let (changed, effect) = dispatch_content_changed(&mut view, &core, ContentAction::Previous);
    assert!(!changed);
    assert!(effect.is_none());
    let (changed, effect) = dispatch_content_changed(&mut view, &core, ContentAction::Export);
    assert!(changed);
    let Some(ContentEffect::SavePath(generation)) = effect else {
        panic!("picker was not requested");
    };
    let (changed, effect) = dispatch_content_changed(&mut view, &core, ContentAction::Export);
    assert!(!changed);
    assert!(effect.is_none());
    let before = view.view_state();
    view.finish_export_result(&core, generation + 1, Err("stale".into()));
    assert_eq!(view.view_state(), before);
    view.finish_export_result(&core, generation, Ok(None));
    assert_ne!(view.view_state(), before);
    core.remove_host(&config.id).unwrap();
    let (changed, _) = dispatch_content_changed(&mut view, &core, ContentAction::Copy);
    assert!(changed);
    let (changed, _) = dispatch_content_changed(&mut view, &core, ContentAction::Copy);
    assert!(!changed);
}

#[test]
fn collection_change_reporting_ignores_repeated_back_and_pending_request() {
    use artisan_api::CollectionRef;
    use editor_aqua::sources::{
        CollectionAction, CollectionBrowser, CollectionPreview, dispatch_collection_changed,
    };
    let directory = tempfile::tempdir().unwrap();
    let mut core = test_support::editor::core::test_core(directory.path());
    let forge = forge();
    let config = connected(&mut core, &forge);
    let source = CollectionRef {
        source: ContentRef::from_text("entry"),
        count: 1,
    };
    let mut browser = CollectionBrowser::<String>::new(core.host(&config.id).unwrap(), source);
    let present = |_: &String, _: u64| CollectionPreview {
        label: "entry".into(),
        detail: None,
        selected: false,
        disabled: false,
    };
    assert!(!dispatch_collection_changed(
        &mut browser,
        &core,
        CollectionAction::Back,
        present,
        None
    ));
    assert!(!dispatch_collection_changed(
        &mut browser,
        &core,
        CollectionAction::Request(0),
        present,
        None
    ));
    core.remove_host(&config.id).unwrap();
    assert!(dispatch_collection_changed(
        &mut browser,
        &core,
        CollectionAction::Back,
        present,
        None
    ));
    assert!(!dispatch_collection_changed(
        &mut browser,
        &core,
        CollectionAction::Back,
        present,
        None
    ));
}

#[test]
fn a_clipboard_that_refuses_the_copy_shows_the_failure_in_the_source() {
    use aqua_text::document::{SelectionDocumentId, SelectionSourceId};
    use editor_aqua::sources::{Copied, content_view};
    use editor_aqua::ui::theme::ArtisanTheme;

    let directory = tempfile::tempdir().unwrap();
    let mut core = test_support::editor::core::test_core(directory.path());
    let forge = forge();
    let config = connected(&mut core, &forge);
    let mut view = ContentText::new(core.host(&config.id).unwrap(), ContentRef::from_text(""));
    view.copy(&core);
    let copied = test_support::wait::wait_for("the source copy", Duration::from_secs(15), || {
        view.drain(&core)
    });
    let refused = copied.write(&core, |_| {
        Err(std::io::Error::other(
            "the clipboard is locked by another app",
        ))
    });
    assert_eq!(refused, Copied::Failed);
    let ignore: std::rc::Rc<dyn Fn(_)> = std::rc::Rc::new(|_| {});
    let rendered = content_view(
        &view,
        &ArtisanTheme::resolved(),
        SelectionDocumentId(1),
        SelectionSourceId(1),
        &ignore,
    );
    let app = test_support::editor::ui::mounted(600.0, 400.0, rendered.element);
    assert!(
        test_support::editor::ui::painted_texts(&app)
            .iter()
            .any(|text| text
                == "Couldn't copy source to the clipboard: the clipboard is locked by another app"),
        "the source view tells the user the copy failed"
    );
    view.copy(&core);
    let copied = test_support::wait::wait_for("the second copy", Duration::from_secs(15), || {
        view.drain(&core)
    });
    assert_eq!(
        copied.write(&core, |_| Ok::<(), std::io::Error>(())),
        Copied::Written
    );
    assert_eq!(view.shown_error(), None);
}
