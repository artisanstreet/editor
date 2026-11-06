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
    let pending = runtime::runtime().spawn(std::future::pending::<()>());
    let generation = {
        let mut state = state.0.borrow_mut();
        let entry = state.entries.get_mut("model").unwrap();
        entry.rendering = Some((key, pending.abort_handle()));
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
                    frame: Some(crate::ui::media::model::Frame {
                        width: 1,
                        height: 1,
                        pixels: vec![255, 0, 0, 255],
                    }),
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
