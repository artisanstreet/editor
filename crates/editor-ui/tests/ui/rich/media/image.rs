use super::*;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use test_support::wait::{LIVENESS, wait_until};

#[test]
fn retry_refetches_the_failed_file_and_releases_the_cached_image() {
    let mut encoded = std::io::Cursor::new(Vec::new());
    image::DynamicImage::new_rgba8(2, 1)
        .write_to(&mut encoded, image::ImageFormat::Png)
        .unwrap();
    let bytes: Arc<[u8]> = encoded.into_inner().into();
    let exists = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&exists);
    let fetcher = MediaFetcher::new("test", move |_| {
        let result = if flag.load(Ordering::SeqCst) {
            Ok(Arc::clone(&bytes))
        } else {
            Err(MediaLoadError::Fetch(
                crate::ui::media::source::MediaFailure::Missing,
            ))
        };
        Box::pin(async move { result })
    })
    .owned_by(&[1], 1);
    let block: ImageBlock = serde_json::from_str(r#"{"src":"image.png"}"#).unwrap();
    let loader = Images::new();
    let assets = aqua::images::ImageAssets::new(aqua::images::ImageAssetBudget::editor());
    let mut state = ImageState::new(&block, fetcher.clone(), loader.clone());
    wait_until("the missing image to fail", LIVENESS, || {
        loader.drain(&assets);
        state.refresh();
        matches!(state.loaded[0], Loaded::Failed(_))
    });
    exists.store(true, Ordering::SeqCst);
    assert!(state.retry(0));
    wait_until("the retried image to load", LIVENESS, || {
        loader.drain(&assets);
        state.refresh();
        matches!(state.loaded[0], Loaded::Ready(_))
    });
    assert!(!state.retry(0));
    drop(state);
    fetcher.retain_resources(&[]);
    assert_eq!(assets.stats().images, 0);
}

#[test]
fn a_new_item_revision_fetches_fresh_image_pixels() {
    use std::sync::atomic::AtomicUsize;
    let mut first = std::io::Cursor::new(Vec::new());
    image::DynamicImage::new_rgba8(2, 1)
        .write_to(&mut first, image::ImageFormat::Png)
        .unwrap();
    let mut second = std::io::Cursor::new(Vec::new());
    image::DynamicImage::new_rgba8(3, 1)
        .write_to(&mut second, image::ImageFormat::Png)
        .unwrap();
    let frames: [Arc<[u8]>; 2] = [first.into_inner().into(), second.into_inner().into()];
    let calls = Arc::new(AtomicUsize::new(0));
    let count = Arc::clone(&calls);
    let fetcher = MediaFetcher::new("test", move |_| {
        let index = count.fetch_add(1, Ordering::SeqCst).min(1);
        let bytes = Arc::clone(&frames[index]);
        Box::pin(async move { Ok(bytes) })
    });
    let block: ImageBlock = serde_json::from_str(r#"{"src":"image.png"}"#).unwrap();
    let loader = Images::new();
    let assets = aqua::images::ImageAssets::default();
    let mut first = ImageState::new(&block, fetcher.clone().owned_by(&[42], 1), loader.clone());
    wait_until("the first revision's pixels", LIVENESS, || {
        loader.drain(&assets);
        first.refresh();
        matches!(&first.loaded[0], Loaded::Ready(image) if matches!(image.status(), ImageStatus::Ready { width: 2, .. }))
    });
    let mut second = ImageState::new(&block, fetcher.owned_by(&[42], 2), loader.clone());
    wait_until("the second revision's pixels", LIVENESS, || {
        loader.drain(&assets);
        second.refresh();
        matches!(&second.loaded[0], Loaded::Ready(image) if matches!(image.status(), ImageStatus::Ready { width: 3, .. }))
    });
    assert_eq!(calls.load(Ordering::SeqCst), 2);
}

#[test]
fn a_failed_image_offers_a_keyboard_reachable_retry_button() {
    use aqua_core::{InputEvent, Key, KeyEvent, Modifiers, NamedKey, Role};
    use aqua_text::{SharedFonts, SharedRuns};
    use std::cell::RefCell;
    let fetcher = MediaFetcher::new("test", |_| {
        Box::pin(async {
            Err(MediaLoadError::Fetch(
                crate::ui::media::source::MediaFailure::ForgeUnreachable(Arc::new(
                    std::io::Error::other("Forge did not answer"),
                )),
            ))
        })
    })
    .owned_by(&[1], 1);
    let block: ImageBlock = serde_json::from_str(r#"{"src":"chart.png","alt":"Chart"}"#).unwrap();
    let loader = Images::new();
    let assets = aqua::images::ImageAssets::new(aqua::images::ImageAssetBudget::editor());
    let mut state = ImageState::new(&block, fetcher, loader.clone());
    wait_until("the fetch to fail", LIVENESS, || {
        loader.drain(&assets);
        state.refresh();
        matches!(state.loaded[0], Loaded::Failed(_))
    });
    let actions = Rc::new(RefCell::new(Vec::new()));
    let sink = Rc::clone(&actions);
    let emit: Rc<dyn Fn(Action)> = Rc::new(move |action| sink.borrow_mut().push(action));
    let view = state
        .view(&block, &ArtisanTheme::resolved(), "gallery", 400.0, &emit)
        .unwrap();
    let mut app = aqua::UiApp::new(SharedFonts::new(), SharedRuns::new());
    app.mount(|_| view).unwrap();
    app.set_viewport(500.0, 400.0, 1.0);
    app.build_frame().unwrap();
    let semantics = app.tree().emit_semantics();
    assert!(
        semantics
            .iter()
            .any(|node| node.role == Role::Button && node.label == "Try again")
    );
    assert!(app.focus_key("gallery-retry-0"));
    let enter = KeyEvent {
        key: Key::Named(NamedKey::Enter),
        modifiers: Modifiers::default(),
        repeat: false,
    };
    app.handle_input(&InputEvent::KeyDown(enter));
    app.handle_input(&InputEvent::KeyUp(enter));
    assert!(matches!(actions.borrow().as_slice(), [Action::Retry(0)]));
}

#[test]
fn the_copy_path_menu_opens_from_the_keyboard_and_assistive_tech() {
    use aqua_core::{InputEvent, Key, KeyEvent, Modifiers, NamedKey};
    use aqua_text::{SharedFonts, SharedRuns};
    use aqua_ui::tree::{ActionEvent, ActionKind};
    use std::cell::RefCell;
    let mut encoded = std::io::Cursor::new(Vec::new());
    image::DynamicImage::new_rgba8(2, 1)
        .write_to(&mut encoded, image::ImageFormat::Png)
        .unwrap();
    let bytes: Arc<[u8]> = encoded.into_inner().into();
    let fetcher = MediaFetcher::new("test", move |_| {
        let bytes = Arc::clone(&bytes);
        Box::pin(async move { Ok(bytes) })
    })
    .owned_by(&[1], 1);
    let block: ImageBlock = serde_json::from_str(r#"{"src":"chart.png","alt":"Chart"}"#).unwrap();
    let loader = Images::new();
    let assets = aqua::images::ImageAssets::new(aqua::images::ImageAssetBudget::editor());
    let mut state = ImageState::new(&block, fetcher, loader.clone());
    wait_until("the image to load", LIVENESS, || {
        loader.drain(&assets);
        state.refresh();
        matches!(state.loaded[0], Loaded::Ready(_))
    });
    let actions = Rc::new(RefCell::new(Vec::new()));
    let sink = Rc::clone(&actions);
    let emit: Rc<dyn Fn(Action)> = Rc::new(move |action| sink.borrow_mut().push(action));
    let view = state
        .view(&block, &ArtisanTheme::resolved(), "gallery", 400.0, &emit)
        .unwrap();
    let mut app = aqua::UiApp::new(SharedFonts::new(), SharedRuns::new());
    app.mount(|_| view).unwrap();
    app.set_viewport(500.0, 400.0, 1.0);
    app.build_frame().unwrap();
    assert!(app.focus_key("gallery-image-0"));
    let copies = |actions: &RefCell<Vec<Action>>| {
        actions
            .borrow()
            .iter()
            .filter(|action| matches!(action, Action::CopyPath { index: 0, .. }))
            .count()
    };
    for (key, modifiers) in [
        (Key::Named(NamedKey::Function(10)), Modifiers::SHIFT),
        (Key::Named(NamedKey::ContextMenu), Modifiers::default()),
        (Key::Named(NamedKey::Function(10)), Modifiers::default()),
    ] {
        let event = KeyEvent {
            key,
            modifiers,
            repeat: false,
        };
        app.handle_input(&InputEvent::KeyDown(event));
        app.handle_input(&InputEvent::KeyUp(event));
    }
    assert_eq!(copies(&actions), 2);
    let frame = app
        .tree()
        .emit_semantics()
        .into_iter()
        .find(|node| node.label == "Chart. Open image preview")
        .unwrap();
    app.tree().fire_action(ActionEvent {
        source: None,
        node: frame.node,
        kind: ActionKind::AccessibilityRequest,
        text: None,
        focused: None,
        request: Some(AccessibilityRequest::ShowContextMenu),
    });
    assert_eq!(copies(&actions), 3);
    assert_eq!(actions.borrow().len(), 3);
}
