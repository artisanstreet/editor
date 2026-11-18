use super::*;
use aqua_core::{Key, Modifiers, NamedKey, Point};
use std::sync::{Arc, mpsc};
use std::time::Duration;

#[test]
fn loading_gap_recreates_camera_but_consecutive_ready_retarget_keeps_it() {
    let state = ModelState::default();
    let initial = Camera::new(30.0, 18.0);
    let owner = Some(("forge".to_owned(), Arc::<[u64]>::from([7])));
    state.ready("model", initial, Shading::Shaded, owner.clone(), 1);
    state
        .0
        .borrow_mut()
        .entries
        .get_mut("model")
        .unwrap()
        .orbit(10.0, 0.0);
    let generation = state.0.borrow().entries["model"].generation;
    state.ready("model", initial, Shading::Wireframe, owner.clone(), 2);
    assert_eq!(state.0.borrow().entries["model"].camera.yaw, 40.0);
    assert_ne!(state.0.borrow().entries["model"].generation, generation);
    state.suspend("model");
    state.ready("model", initial, Shading::Wireframe, owner.clone(), 2);
    assert_eq!(state.0.borrow().entries["model"].camera, initial);
    assert_eq!(
        state.0.borrow().entries["model"].shading,
        Shading::Wireframe
    );
    state
        .0
        .borrow_mut()
        .entries
        .get_mut("model")
        .unwrap()
        .orbit(20.0, 0.0);
    state.ready(
        "model",
        initial,
        Shading::Shaded,
        Some(("forge".to_owned(), Arc::<[u64]>::from([8]))),
        2,
    );
    assert_eq!(state.0.borrow().entries["model"].camera, initial);
}

#[test]
fn camera_input_clamps_and_reset_restores_the_authored_view() {
    let state = ModelState::default();
    let initial = Camera::new(37.0, -23.0);
    state.ensure("model", initial, Shading::Wireframe);
    let mut inner = state.0.borrow_mut();
    let entry = inner.entries.get_mut("model").unwrap();
    assert!(entry.key(Key::Named(NamedKey::Right), Modifiers::SHIFT));
    assert_eq!(entry.camera.yaw, 39.0);
    entry.orbit(720.0, 500.0);
    assert_eq!(entry.camera.pitch, 89.0);
    for _ in 0..100 {
        assert!(entry.key(Key::Char('+'), Modifiers::NONE));
    }
    assert_eq!(entry.camera.distance, Camera::NEAREST);
    entry.bounds.set(Some(Rect {
        x: 0.0,
        y: 0.0,
        w: 100.0,
        h: 100.0,
    }));
    entry.drag = Some(Drag {
        last: Point { x: 0.0, y: 0.0 },
        pan: true,
    });
    assert!(entry.move_to(Point {
        x: 100_000.0,
        y: -100_000.0
    }));
    assert_eq!(entry.camera.pan, (-1.5, -1.5));
    assert!(entry.key(Key::Char('0'), Modifiers::NONE));
    assert_eq!(entry.camera, initial);
    assert!(entry.drag.is_none());
    assert!(!entry.key(Key::Char('q'), Modifiers::NONE));
}

#[test]
fn released_view_rejects_old_software_completion_for_reused_id() {
    let state = ModelState::default();
    let (wake, received) = mpsc::channel();
    state.0.borrow().mailbox.set_waker(Arc::new(move || {
        let _ = wake.send(());
    }));
    let initial = Camera::default();
    state.ensure("model", initial, Shading::Shaded);
    let old_generation = state.0.borrow().entries["model"].generation;
    state.release("model");
    state.ensure("model", initial, Shading::Shaded);
    let key = FrameKey {
        model: 1,
        camera: initial,
        look: Look {
            base: glam::Vec3::ONE,
            edges: None,
        },
        width: 1,
        height: 1,
        samples: 1,
    };
    let generation = {
        let mut state = state.0.borrow_mut();
        let entry = state.entries.get_mut("model").unwrap();
        entry.rendering = Some(Rendering {
            key,
            stop: CancellationToken::new(),
        });
        entry.generation
    };
    assert_ne!(generation, old_generation);
    let sender = state.0.borrow().delivery.sender();
    runtime::runtime().spawn(async move {
        assert!(
            sender
                .send(Completion {
                    id: "model".to_owned(),
                    generation: old_generation,
                    key,
                    frame: crate::ui::media::model::Frame {
                        width: 1,
                        height: 1,
                        pixels: vec![255, 0, 0, 255],
                    },
                })
                .await
                .is_ok()
        );
    });
    received.recv_timeout(Duration::from_secs(10)).unwrap();
    let images = ImageAssets::new(aqua::images::ImageAssetBudget::editor());
    state.drain(&images);
    let inner = state.0.borrow();
    let entry = &inner.entries["model"];
    assert!(entry.shown.is_none());
    assert!(entry.rendering.is_some());
}

#[test]
fn releasing_a_view_cancels_its_queued_software_render_and_frees_the_model() {
    let busy = runtime::runtime()
        .block_on(Arc::clone(&RASTER_WORKERS).acquire_many_owned(2))
        .unwrap();
    let state = ModelState::default();
    let model = Arc::new(LoadedModel::from_model(
        crate::ui::media::model::parse(b"v 0 0 0\nv 1 0 0\nv 0 1 0\nf 1 2 3\n", None, None)
            .unwrap(),
        RendererCapability::Software,
    ));
    let initial = Camera::default();
    state.ensure("model", initial, Shading::Shaded);
    let key = FrameKey {
        model: 1,
        camera: initial,
        look: Look {
            base: glam::Vec3::ONE,
            edges: None,
        },
        width: 8,
        height: 8,
        samples: 1,
    };
    state.render_software("model", key, Arc::clone(&model));
    let stop = state.0.borrow().entries["model"]
        .rendering
        .as_ref()
        .unwrap()
        .stop
        .clone();
    assert!(!stop.is_cancelled());
    state.release("model");
    assert!(stop.is_cancelled());
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while Arc::strong_count(&model) > 1 {
        assert!(
            std::time::Instant::now() < deadline,
            "the cancelled render still holds the model"
        );
        std::thread::yield_now();
    }
    drop(busy);
    assert!(state.0.borrow_mut().delivery.drain().is_empty());
}

#[test]
fn the_model_viewport_shows_a_focus_ring_when_tab_reaches_it() {
    use aqua_core::{InputEvent, KeyEvent};
    use aqua_text::{SharedFonts, SharedRuns};
    use aqua_ui::tree::hash_key;
    let block: ModelBlock = serde_json::from_str(
        r#"{"src":"data:text/plain,v%200%200%200%0Av%201%200%200%0Av%200%201%200%0Af%201%202%203%0A","format":"obj"}"#,
    )
    .unwrap();
    let assets = ModelAssets::new();
    let services = crate::ui::rich::media::model::services(
        "model",
        crate::ui::theme::ArtisanTheme::resolved(),
        assets.clone(),
        Rc::new(|| Some(RendererCapability::Software)),
        Rc::new(|_, size, _| ui::Box::new().width(size).height(size).build()),
        Rc::new(|_| {}),
        Rc::new(Cell::new(false)),
        Rc::new(|| {}),
    );
    let state = ModelState::default();
    let frame = hash_key(aqua_ui::element::Key::from("model-frame"));
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    let mut app = loop {
        assets.drain();
        let element = block.view("model", 400.0, 1.0, None, state.clone(), &services);
        let mut app = aqua::UiApp::new(SharedFonts::new(), SharedRuns::new());
        app.mount(move |_| element).unwrap();
        app.set_viewport(600.0, 600.0, 1.0);
        app.build_frame().unwrap();
        if app.tree().sole_node_with_key(frame).is_some() {
            break app;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the model loads in time"
        );
        std::thread::yield_now();
    };
    let viewport = app.tree().sole_node_with_key(frame).unwrap();
    let painted = |app: &mut aqua::UiApp| {
        app.tree_mut()
            .emit_scene()
            .into_iter()
            .filter(|(owner, _)| *owner == viewport)
            .count()
    };
    let resting = painted(&mut app);
    let tab = KeyEvent {
        key: Key::Named(NamedKey::Tab),
        modifiers: Modifiers::default(),
        repeat: false,
    };
    for _ in 0..8 {
        if app.tree().focused() == Some(viewport) {
            break;
        }
        app.handle_input(&InputEvent::KeyDown(tab));
        app.handle_input(&InputEvent::KeyUp(tab));
    }
    assert_eq!(app.tree().focused(), Some(viewport));
    app.build_frame().unwrap();
    assert!(
        painted(&mut app) > resting,
        "keyboard focus paints an indicator around the viewport"
    );
}
