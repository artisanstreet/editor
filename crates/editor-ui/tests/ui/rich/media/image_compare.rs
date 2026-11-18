use super::*;
use aqua::images::ImageAssets;

fn fetcher() -> MediaFetcher {
    MediaFetcher::new("test", |_| Box::pin(async { Err(MediaFailure::Missing) })).owned_by(&[1], 1)
}

fn model() -> ImageCompare {
    serde_json::from_str(r#"{"before":{"src":""},"after":{"src":""}}"#).unwrap()
}

#[test]
fn slider_keyboard_drag_and_toggle_follow_source_rules() {
    let mut state = CompareState::new(&model(), fetcher(), Images::new());
    assert!(state.dispatch(Action::Key {
        key: Key::Named(NamedKey::Right),
        shift: false
    }));
    assert!((state.split - 0.55).abs() < 0.0001);
    assert!(state.dispatch(Action::Key {
        key: Key::Named(NamedKey::Left),
        shift: true
    }));
    assert!((state.split - 0.54).abs() < 0.0001);
    assert!(state.dispatch(Action::Press {
        x: 250.0,
        width: 100.0,
        clicks: 1
    }));
    assert_eq!(state.split, 1.0);
    assert!(!state.dispatch(Action::Key {
        key: Key::Named(NamedKey::End),
        shift: false
    }));
    assert!(state.dispatch(Action::Release));
    assert!(!state.dispatch(Action::Release));
    assert!(state.dispatch(Action::Press {
        x: 0.0,
        width: 100.0,
        clicks: 2
    }));
    assert_eq!(state.split, 0.5);
    assert!(state.dispatch(Action::Mode(2)));
    assert!(state.dispatch(Action::Key {
        key: Key::Char(' '),
        shift: false
    }));
    assert!(state.showing_after);
    assert!(!state.dispatch(Action::Mode(2)));
}

#[test]
fn sizing_uses_hints_and_height_cap_when_images_are_unavailable() {
    let hint = super::super::super::SizeHint {
        width: Some(1200.0),
        height: Some(600.0),
    };
    let images = [ImageState::Loading, ImageState::Loading];
    assert_eq!(frame_size(&images, hint), (2.0, 960.0));
    assert_eq!(
        frame_size(&images, super::super::super::SizeHint::default()),
        (16.0 / 9.0, 480.0 * 16.0 / 9.0)
    );
}

#[test]
fn data_images_use_asset_decoder_and_release_leases_on_scope_change() {
    use base64::Engine as _;
    use std::io::Cursor;
    use std::time::{Duration, Instant};

    let mut encoded = Cursor::new(Vec::new());
    image::DynamicImage::new_rgba8(2, 1)
        .write_to(&mut encoded, image::ImageFormat::Png)
        .unwrap();
    let data = format!(
        "data:image/png;base64,{}",
        base64::engine::general_purpose::STANDARD.encode(encoded.into_inner())
    );
    let value = serde_json::json!({"before": {"src": data.clone()}, "after": {"src": data}});
    let model: ImageCompare = serde_json::from_value(value).unwrap();
    let assets = ImageAssets::new(aqua::images::ImageAssetBudget::editor());
    let loader = Images::new();
    let fetcher = fetcher();
    let mut state = CompareState::new(&model, fetcher.clone(), loader.clone());
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        loader.drain(&assets);
        state.refresh();
        if state.images.iter().all(|image| matches!(image, ImageState::Decoding(handle) if matches!(handle.status(), ImageStatus::Ready { width: 2, height: 1 }))) { break; }
        assert!(Instant::now() < deadline, "image decoder did not finish");
        std::thread::sleep(Duration::from_millis(2));
    }
    assert_eq!(frame_size(&state.images, model.hint), (2.0, 48.0));
    assert!(!state.reconcile(&model, fetcher.clone(), false));
    assert!(state.reconcile(&model, fetcher.clone(), true));
    drop(state);
    fetcher.retain_resources(&[]);
    loader.retain_owners(&[]);
    assert_eq!(assets.stats().images, 0);
}

#[test]
fn a_failed_side_offers_retry_that_refetches_only_that_side() {
    use aqua_text::{SharedFonts, SharedRuns};
    use std::{cell::RefCell, sync::atomic::AtomicBool};
    let reachable = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&reachable);
    let fetcher = MediaFetcher::new("test", move |_| {
        let up = flag.load(Ordering::SeqCst);
        Box::pin(async move {
            if up {
                Err(MediaFailure::Missing)
            } else {
                Err(MediaFailure::Unreachable)
            }
        })
    })
    .owned_by(&[1], 1);
    let model: ImageCompare = serde_json::from_str(
        r#"{"before":{"src":"before.png"},"after":{"src":"after.png"},"mode":"side-by-side"}"#,
    )
    .unwrap();
    let loader = Images::new();
    let assets = ImageAssets::default();
    let mut state = CompareState::new(&model, fetcher, loader.clone());
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while state.failure(Side::Before).is_none() || state.failure(Side::After).is_none() {
        loader.drain(&assets);
        state.refresh();
        assert!(
            std::time::Instant::now() < deadline,
            "both sides fail in time"
        );
        std::thread::yield_now();
    }
    let actions = Rc::new(RefCell::new(Vec::new()));
    let sink = Rc::clone(&actions);
    let view = state
        .view(
            &model,
            crate::ui::theme::ArtisanTheme::resolved(),
            "compare",
            800.0,
            Rc::new(move |action| sink.borrow_mut().push(action)),
        )
        .unwrap();
    let mut app = aqua::UiApp::new(SharedFonts::new(), SharedRuns::new());
    app.mount(|_| view).unwrap();
    app.set_viewport(900.0, 700.0, 1.0);
    app.build_frame().unwrap();
    let said: Vec<String> = app
        .tree()
        .traverse()
        .into_iter()
        .filter_map(|node| match &app.tree().get(node)?.kind {
            aqua_ui::tree::NodeKind::Text(text) => Some(text.text().to_owned()),
            _ => None,
        })
        .collect();
    assert_eq!(
        said.iter()
            .filter(|text| *text == "The host didn't answer")
            .count(),
        2
    );
    assert!(app.focus_key("compare-retry-after"));
    let enter = aqua_core::KeyEvent {
        key: Key::Named(NamedKey::Enter),
        modifiers: aqua_core::Modifiers::default(),
        repeat: false,
    };
    app.handle_input(&InputEvent::KeyDown(enter));
    app.handle_input(&InputEvent::KeyUp(enter));
    assert_eq!(actions.borrow().as_slice(), &[Action::Retry(Side::After)]);
    reachable.store(true, Ordering::SeqCst);
    assert!(state.dispatch(Action::Retry(Side::After)));
    while state.failure(Side::After) != Some(Failure::Fetch(MediaFailure::Missing)) {
        loader.drain(&assets);
        state.refresh();
        assert!(
            std::time::Instant::now() < deadline,
            "the retried side settles in time"
        );
        std::thread::yield_now();
    }
    assert_eq!(
        state.failure(Side::Before),
        Some(Failure::Fetch(MediaFailure::Unreachable))
    );
}
