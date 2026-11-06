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
