use super::*;

fn diagram(width: f32, height: f32) -> Arc<DiagramSvg> {
    Arc::new(DiagramSvg {
        source: Arc::from(""),
        light_source: Arc::from(""),
        width_bits: width.to_bits(),
        height_bits: height.to_bits(),
    })
}

fn viewport() -> Rect {
    Rect {
        x: 10.0,
        y: 20.0,
        w: 400.0,
        h: 200.0,
    }
}

#[test]
fn fit_centers_the_diagram_and_never_upscales() {
    let mut state = DiagramState::new();
    let small = state.destination(viewport(), &diagram(100.0, 50.0));
    assert_eq!((small.w, small.h), (100.0, 50.0));
    assert_eq!((small.x, small.y), (160.0, 95.0));
    let large = state.destination(viewport(), &diagram(800.0, 200.0));
    assert_eq!((large.w, large.h), (400.0, 100.0));
}

#[test]
fn zoom_is_bounded_and_scales_the_pan_with_it() {
    let mut state = DiagramState::new();
    state.pan = Point { x: 8.0, y: -4.0 };
    state.zoom_by(2.0);
    assert_eq!((state.zoom, state.pan.x, state.pan.y), (2.0, 16.0, -8.0));
    for _ in 0..40 {
        state.zoom_by(ZOOM_STEP);
    }
    assert_eq!(state.zoom, MAX_ZOOM);
    for _ in 0..80 {
        state.zoom_by(1.0 / ZOOM_STEP);
    }
    assert_eq!(state.zoom, MIN_ZOOM);
}

#[test]
fn pan_stays_inside_the_enlarged_diagram_and_reset_restores_the_fit() {
    let mut state = DiagramState::new();
    let diagram = diagram(400.0, 200.0);
    state.pan = Point { x: 500.0, y: 500.0 };
    let fitted = state.destination(viewport(), &diagram);
    assert_eq!((fitted.x, fitted.y), (10.0, 20.0));
    state.zoom_by(2.0);
    state.pan = Point {
        x: 500.0,
        y: -500.0,
    };
    let zoomed = state.destination(viewport(), &diagram);
    assert_eq!((zoomed.x, zoomed.y), (10.0, -180.0));
    state.reset();
    assert_eq!((state.zoom, state.pan.x, state.pan.y), (1.0, 0.0, 0.0));
}

#[test]
fn dragging_pans_until_released_and_a_new_diagram_resets_the_view() {
    let mut state = DiagramState::new();
    let first = diagram(400.0, 200.0);
    state.sync(&first);
    state.zoom_by(2.0);
    assert!(!state.drag_to(Point { x: 5.0, y: 5.0 }));
    state.drag = Some(Point { x: 100.0, y: 100.0 });
    assert!(state.drag_to(Point { x: 70.0, y: 90.0 }));
    assert_eq!((state.pan.x, state.pan.y), (-30.0, -10.0));
    state.sync(&first);
    assert_eq!(state.zoom, 2.0);
    state.sync(&diagram(400.0, 200.0));
    assert_eq!((state.zoom, state.pan.x, state.drag), (1.0, 0.0, None));
}

#[test]
fn keys_zoom_pan_and_reset() {
    let mut state = DiagramState::new();
    assert!(state.key(Key::Char('+')));
    assert_eq!(state.zoom, ZOOM_STEP);
    assert!(state.key(Key::Named(NamedKey::Left)));
    assert_eq!(state.pan.x, KEY_PAN_PX);
    assert!(state.key(Key::Char('0')));
    assert_eq!((state.zoom, state.pan.x), (1.0, 0.0));
    assert!(!state.key(Key::Char('x')));
}
