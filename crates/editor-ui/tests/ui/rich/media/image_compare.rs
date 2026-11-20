use super::*;
use aqua::images::ImageAssets;
use std::sync::Arc;

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
    assert_eq!((state.split).to_bits(), 1.0_f32.to_bits());
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
    assert_eq!((state.split).to_bits(), 0.5_f32.to_bits());
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

    let mut encoded = Cursor::new(Vec::new());
    image::DynamicImage::new_rgba8(2, 1)
        .write_to(&mut encoded, image::ImageFormat::Png)
        .unwrap();
    let data = format!(
        "data:image/png;base64,{}",
        base64::engine::general_purpose::STANDARD.encode(encoded.into_inner())
    );
    let value = serde_json::json!({"before": {"src": data}, "after": {"src": data}});
    let model: ImageCompare = serde_json::from_value(value).unwrap();
    let assets = ImageAssets::new(aqua::images::ImageAssetBudget::editor());
    let loader = Images::new();
    let fetcher = fetcher();
    let mut state = CompareState::new(&model, fetcher.clone(), loader.clone());
    test_support::wait::wait_until(
        "both images to decode",
        test_support::wait::LIVENESS,
        || {
            loader.drain(&assets);
            state.refresh();
            state.images.iter().all(|image| matches!(image, ImageState::Decoding(handle) if matches!(handle.status(), ImageStatus::Ready { width: 2, height: 1 })))
        },
    );
    assert_eq!(frame_size(&state.images, model.hint), (2.0, 48.0));
    assert!(!state.reconcile(&model, fetcher.clone(), false));
    assert!(state.reconcile(&model, fetcher.clone(), true));
    drop(state);
    fetcher.retain_resources(&[]);
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
                Err(MediaFailure::ForgeUnreachable)
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
    test_support::wait::wait_until("both sides to fail", test_support::wait::LIVENESS, || {
        loader.drain(&assets);
        state.refresh();
        state.failure(Side::Before).is_some() && state.failure(Side::After).is_some()
    });
    let actions = Rc::new(RefCell::new(Vec::new()));
    let sink = Rc::clone(&actions);
    let view = state
        .view(
            &model,
            &crate::ui::theme::ArtisanTheme::resolved(),
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
            aqua_ui::tree::NodeKind::Box(_)
            | aqua_ui::tree::NodeKind::Media(_)
            | aqua_ui::tree::NodeKind::Scroll(_)
            | aqua_ui::tree::NodeKind::Canvas(_)
            | aqua_ui::tree::NodeKind::Viewport3D(_) => None,
        })
        .collect();
    assert_eq!(
        said.iter()
            .filter(|text| *text == "Forge didn't answer")
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
    test_support::wait::wait_until(
        "the retried side to settle",
        test_support::wait::LIVENESS,
        || {
            loader.drain(&assets);
            state.refresh();
            state.failure(Side::After) == Some(Failure::Fetch(MediaFailure::Missing))
        },
    );
    assert_eq!(
        state.failure(Side::Before),
        Some(Failure::Fetch(MediaFailure::ForgeUnreachable))
    );
}

#[test]
#[expect(
    clippy::too_many_lines,
    reason = "one scenario operating the comparison frame as slider and as switch"
)]
fn the_comparison_frame_is_a_named_slider_or_switch_that_assistive_tech_operates() {
    use aqua_core::Role;
    use aqua_text::{SharedFonts, SharedRuns};
    use aqua_ui::tree::{AccessibilityRequest, ActionEvent, ActionKind};
    use std::cell::RefCell;
    let model: ImageCompare = serde_json::from_str(
        r#"{"before":{"src":"","label":"Old"},"after":{"src":"","label":"New"}}"#,
    )
    .unwrap();
    let mut state = CompareState::new(&model, fetcher(), Images::new());
    let actions = Rc::new(RefCell::new(Vec::new()));
    let render = |state: &CompareState| {
        let sink = Rc::clone(&actions);
        let view = state
            .view(
                &model,
                &crate::ui::theme::ArtisanTheme::resolved(),
                "compare",
                800.0,
                Rc::new(move |action| sink.borrow_mut().push(action)),
            )
            .unwrap();
        let mut app = aqua::UiApp::new(SharedFonts::new(), SharedRuns::new());
        app.mount(|_| view).unwrap();
        app.set_viewport(900.0, 700.0, 1.0);
        app.build_frame().unwrap();
        app
    };
    let frame = |app: &aqua::UiApp, role: Role, label: &str| {
        app.tree()
            .emit_semantics()
            .into_iter()
            .find(|node| node.role == role && node.label == label)
            .unwrap_or_else(|| panic!("no {role:?} named {label:?}"))
    };
    let fire = |app: &aqua::UiApp, node, kind, request| {
        app.tree().fire_action(ActionEvent {
            source: None,
            node,
            kind,
            text: None,
            focused: None,
            request,
        });
    };
    let app = render(&state);
    let slider = frame(&app, Role::Slider, "Split between Old and New");
    assert!(slider.focusable);
    assert_eq!(
        slider
            .numeric_range
            .map(|range| (range.min, range.max, range.value)),
        Some((0.0, 100.0, 50.0))
    );
    for request in [
        AccessibilityRequest::SetValue(30.0),
        AccessibilityRequest::Increment,
    ] {
        fire(
            &app,
            slider.node,
            ActionKind::AccessibilityRequest,
            Some(request),
        );
    }
    let emitted = actions.borrow_mut().split_off(0);
    assert_eq!(
        emitted,
        vec![
            Action::Split(0.3),
            Action::Key {
                key: Key::Named(NamedKey::Right),
                shift: false
            }
        ]
    );
    for action in emitted {
        assert!(state.dispatch(action));
    }
    let app = render(&state);
    let slider = frame(&app, Role::Slider, "Split between Old and New");
    assert_eq!(
        slider.numeric_range.map(|range| range.value.round()),
        Some(35.0)
    );
    assert!(state.dispatch(Action::Mode(3)));
    let app = render(&state);
    frame(&app, Role::Slider, "Blend from Old to New");
    assert!(state.dispatch(Action::Mode(2)));
    let app = render(&state);
    let switch = frame(&app, Role::Switch, "Show New instead of Old");
    assert_eq!(switch.checked, Some(false));
    fire(&app, switch.node, ActionKind::AccessibilityActivate, None);
    let emitted = actions.borrow_mut().split_off(0);
    assert_eq!(
        emitted,
        vec![Action::Key {
            key: Key::Named(NamedKey::Enter),
            shift: false
        }]
    );
    for action in emitted {
        assert!(state.dispatch(action));
    }
    let app = render(&state);
    let switch = frame(&app, Role::Switch, "Show New instead of Old");
    assert_eq!(switch.checked, Some(true));
}
