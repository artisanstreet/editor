use artisan_api::ContentRef;
use editor_aqua::{
    core::{Core, EditorFiles, ForgeClient, HostConfig, HostId, HostKind, RangeState},
    sources::ContentText,
};
use std::time::{Duration, Instant};

fn config() -> HostConfig {
    HostConfig {
        id: HostId("same-host".into()),
        name: "Forge".into(),
        address: "127.0.0.1:1".into(),
        fingerprint: "0".repeat(64),
        kind: HostKind::Remote,
    }
}

fn client(core: &Core) -> ForgeClient {
    ForgeClient::new(
        "127.0.0.1:1",
        &"0".repeat(64),
        core.identity.as_ref().unwrap(),
    )
    .unwrap()
}

fn pending_copy(core: &Core, config: &HostConfig) -> ContentText {
    let mut view = ContentText::new(
        core.host(&config.id).unwrap(),
        ContentRef::from_text("stored content"),
    );
    view.copy(core);
    let deadline = Instant::now() + Duration::from_secs(15);
    while !view.action_finished() {
        assert!(Instant::now() < deadline, "source copy did not finish");
        std::thread::sleep(Duration::from_millis(5));
    }
    view
}

#[test]
fn copy_and_range_completions_are_discarded_after_removal_and_replacement() {
    let directory = tempfile::tempdir().unwrap();
    let mut core = Core::for_tests(EditorFiles {
        dir: directory.path().to_owned(),
    })
    .unwrap();
    let config = config();
    core.add_test_host(&config, client(&core));

    let mut current = pending_copy(&core, &config);
    assert!(current.drain(&core).is_none());
    assert!(
        current
            .error
            .as_deref()
            .is_some_and(|error| error.starts_with("Couldn't copy source:"))
    );

    let mut removed = pending_copy(&core, &config);
    let old = core.host(&config.id).unwrap().instance();
    core.remove_host(&config.id).unwrap();
    assert!(removed.drain(&core).is_none());
    assert!(removed.error.is_none());
    assert!(
        matches!(removed.window().state(), RangeState::Failed(error) if error == "This Forge is no longer connected.")
    );

    core.add_test_host(&config, client(&core));
    assert_ne!(old, core.host(&config.id).unwrap().instance());
    let mut replaced = pending_copy(&core, &config);
    core.add_test_host(&config, client(&core));
    assert!(replaced.drain(&core).is_none());
    assert!(replaced.error.is_none());
    assert!(
        matches!(replaced.window().state(), RangeState::Failed(error) if error == "This Forge is no longer connected.")
    );
}

#[test]
fn collection_actions_do_not_reach_a_recreated_host() {
    use artisan_api::{CollectionRange, CollectionRef};
    use editor_aqua::sources::{CollectionBrowser, CollectionPreview};
    use std::cell::Cell;
    let directory = tempfile::tempdir().unwrap();
    let mut core = Core::for_tests(EditorFiles {
        dir: directory.path().to_owned(),
    })
    .unwrap();
    let config = config();
    core.add_test_host(&config, client(&core));
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
    core.add_test_host(&config, client(&core));
    assert!(!browser.activate(&core, &page, 0, present, |_, _| activated.set(true)));
    assert!(!activated.get());
}

#[test]
fn late_save_dialog_cannot_export_a_replaced_source_or_host() {
    let directory = tempfile::tempdir().unwrap();
    let mut core = Core::for_tests(EditorFiles {
        dir: directory.path().to_owned(),
    })
    .unwrap();
    let config = config();
    core.add_test_host(&config, client(&core));
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
    core.add_test_host(&config, client(&core));
    view.finish_export(&core, current_picker, Some(path.clone()));
    assert!(!view.busy());
    assert!(!path.exists());
}

#[test]
fn picker_failures_only_apply_to_the_current_source_and_host() {
    let directory = tempfile::tempdir().unwrap();
    let mut core = Core::for_tests(EditorFiles {
        dir: directory.path().to_owned(),
    })
    .unwrap();
    let config = config();
    core.add_test_host(&config, client(&core));
    let mut view = ContentText::new(
        core.host(&config.id).unwrap(),
        ContentRef::from_text("first"),
    );
    let first = view.begin_export(&core).unwrap();
    view.finish_export_result(&core, first, Err("picker failed".into()));
    assert_eq!(
        view.error.as_deref(),
        Some("Couldn't export source: picker failed")
    );
    let old = view.begin_export(&core).unwrap();
    view.set_source(ContentRef::from_text("second"));
    let current = view.begin_export(&core).unwrap();
    view.finish_export_result(&core, old, Err("old picker".into()));
    assert!(view.error.is_none());
    assert!(view.busy());
    view.finish_export_result(&core, current, Ok(None));
    assert!(view.error.is_none());
    assert!(!view.busy());
    let replaced = view.begin_export(&core).unwrap();
    core.add_test_host(&config, client(&core));
    view.finish_export_result(&core, replaced, Err("removed host".into()));
    assert!(view.error.is_none());
    assert!(!view.busy());
}

#[test]
fn source_change_reporting_rejects_noops_and_keeps_picker_effects() {
    use editor_aqua::sources::{ContentAction, ContentEffect, dispatch_content_changed};
    let directory = tempfile::tempdir().unwrap();
    let mut core = Core::for_tests(EditorFiles {
        dir: directory.path().to_owned(),
    })
    .unwrap();
    let config = config();
    core.add_test_host(&config, client(&core));
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
    let mut core = Core::for_tests(EditorFiles {
        dir: directory.path().to_owned(),
    })
    .unwrap();
    let config = config();
    core.add_test_host(&config, client(&core));
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
