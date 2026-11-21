use super::*;
use crate::ui::markdown::DiagramInks;
use crate::ui::theme::{Contrast, ThemeMode};
use aqua_core::{PointerEvent, PointerKind, ScrollDelta, Size};
use aqua_reactive::Runtime;
use aqua_ui::binding::View;
use aqua_ui::input::route_input;
use aqua_ui::layout::NaiveMeasurer;
use aqua_ui::tree::{UiTree, hash_key};

fn diagram(width: f32, height: f32) -> Arc<DiagramSvg> {
    Arc::new(DiagramSvg {
        inks: DiagramInks {
            dark: Arc::from(""),
            light: Arc::from(""),
            dark_high: Arc::from(""),
            light_high: Arc::from(""),
        },
        width_bits: width.to_bits(),
        height_bits: height.to_bits(),
        summary: Arc::from(""),
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
    let state = DiagramState::new();
    let small = state.destination(viewport(), &diagram(100.0, 50.0), 1.0);
    assert_eq!((small.w, small.h), (100.0, 50.0));
    assert_eq!((small.x, small.y), (160.0, 95.0));
    let large = state.destination(viewport(), &diagram(800.0, 200.0), 1.0);
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
    assert_eq!((state.zoom).to_bits(), (MAX_ZOOM).to_bits());
    for _ in 0..80 {
        state.zoom_by(1.0 / ZOOM_STEP);
    }
    assert_eq!((state.zoom).to_bits(), (MIN_ZOOM).to_bits());
}

#[test]
fn pan_is_preserved_at_fit_and_when_zoomed_and_reset_restores_the_fit() {
    let mut state = DiagramState::new();
    let diagram = diagram(400.0, 200.0);
    state.pan = Point { x: 500.0, y: 500.0 };
    let fitted = state.destination(viewport(), &diagram, 1.0);
    assert_eq!((fitted.x, fitted.y), (510.0, 520.0));
    state.zoom_by(2.0);
    state.pan = Point {
        x: 500.0,
        y: -500.0,
    };
    let zoomed = state.destination(viewport(), &diagram, 1.0);
    assert_eq!((zoomed.x, zoomed.y), (310.0, -580.0));
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
    assert_eq!((state.zoom).to_bits(), 2.0_f32.to_bits());
    state.sync(&diagram(400.0, 200.0));
    assert_eq!((state.zoom, state.pan.x, state.drag), (1.0, 0.0, None));
}

#[test]
fn keys_zoom_pan_and_reset() {
    let mut state = DiagramState::new();
    assert!(state.key(Key::Char('+')));
    assert_eq!((state.zoom).to_bits(), (ZOOM_STEP).to_bits());
    assert!(state.key(Key::Named(NamedKey::Left)));
    assert_eq!((state.pan.x).to_bits(), (KEY_PAN_PX).to_bits());
    assert!(state.key(Key::Char('0')));
    assert_eq!((state.zoom, state.pan.x), (1.0, 0.0));
    assert!(!state.key(Key::Char('x')));
}

fn mounted() -> (UiTree, Shared, aqua_core::NodeId) {
    let runtime = Runtime::new();
    let mut tree = UiTree::new();
    let state = Rc::new(RefCell::new(DiagramState::new()));
    let invalidate: Rc<dyn Fn()> = Rc::new(|| {});
    let mut mounted = View::new(runtime.create_scope(), tree.command_sink());
    mounted
        .set_content(
            &mut tree,
            vec![view(
                ArtisanTheme::for_mode(ThemeMode::Dark),
                "diagram",
                &diagram(400.0, 200.0),
                &state,
                &invalidate,
            )],
        )
        .unwrap();
    mounted
        .compute_layout(&mut tree, Size::new(400.0, 300.0), &NaiveMeasurer)
        .unwrap();
    let node = tree
        .sole_node_with_key(hash_key(aqua_ui::element::Key::from("diagram-viewport")))
        .unwrap();
    (tree, state, node)
}

fn pointer(x: f32, y: f32, buttons: u8) -> PointerEvent {
    PointerEvent {
        contact: 0,
        kind: PointerKind::Mouse,
        position: Point::new(x, y),
        buttons,
    }
}

#[test]
fn wheel_over_diagram_zooms_both_directions_without_modifiers_and_stops_bubbling() {
    let (mut tree, state, _) = mounted();
    for (delta, enlarged) in [(-16.0, true), (32.0, false)] {
        let outcome = route_input(
            &mut tree,
            &InputEvent::Scroll {
                at: Point::new(100.0, 100.0),
                delta: ScrollDelta { x: 0.0, y: delta },
            },
            |_, _| true,
        );
        assert!(outcome.handled && outcome.default_prevented && outcome.propagation_stopped);
        assert_eq!(state.borrow().zoom > 1.0, enlarged);
    }
}

#[test]
fn drag_captures_pointer_pans_at_fit_and_releases_outside_diagram() {
    let (mut tree, state, viewport_node) = mounted();
    let outcome = route_input(
        &mut tree,
        &InputEvent::PointerDown(pointer(100.0, 100.0, 1), PointerButton::Primary),
        |_, _| true,
    );
    assert!(outcome.handled && outcome.default_prevented);
    assert_eq!(tree.pointer_capture(0), Some(viewport_node));
    let outcome = route_input(
        &mut tree,
        &InputEvent::PointerMove(pointer(450.0, 350.0, 1)),
        |_, _| true,
    );
    assert!(outcome.handled && outcome.default_prevented);
    let destination = state
        .borrow_mut()
        .destination(viewport(), &diagram(400.0, 200.0), 1.0);
    assert_eq!((destination.x, destination.y), (360.0, 270.0));
    route_input(
        &mut tree,
        &InputEvent::PointerUp(pointer(450.0, 350.0, 0), PointerButton::Primary),
        |_, _| true,
    );
    assert_eq!(tree.pointer_capture(0), None);
    assert_eq!(state.borrow().drag, None);
    assert!(!state.borrow_mut().drag_to(Point::new(500.0, 400.0)));
}

#[test]
fn the_viewport_describes_the_parsed_nodes_and_links() {
    use aqua_text::{SharedFonts, SharedRuns};
    let diagram = Arc::new(
        crate::ui::markdown::diagram::render(
            "flowchart LR\nA[Start] -->|next| B[Review]\nB --> C[Ship]",
        )
        .unwrap(),
    );
    let state = Rc::new(RefCell::new(DiagramState::new()));
    let invalidate: Rc<dyn Fn()> = Rc::new(|| {});
    let element = view(
        &ArtisanTheme::resolved(),
        "diagram",
        &diagram,
        &state,
        &invalidate,
    );
    let mut app = aqua::UiApp::new(SharedFonts::new(), SharedRuns::new());
    app.mount(move |_| element).unwrap();
    app.set_viewport(600.0, 600.0, 1.0);
    app.build_frame().unwrap();
    let description = app
        .tree()
        .emit_semantics()
        .into_iter()
        .find(|node| node.label == "Mermaid diagram")
        .and_then(|node| node.description)
        .unwrap();
    assert!(
        description
            .starts_with("Nodes: Start, Review, Ship. Links: Start → Review: next; Review → Ship."),
        "{description}"
    );
}

#[test]
fn the_viewport_shows_a_focus_ring_when_tab_reaches_it() {
    use aqua_core::{InputEvent, Key, KeyEvent, Modifiers, NamedKey};
    use aqua_text::{SharedFonts, SharedRuns};
    let diagram = Arc::new(crate::ui::markdown::diagram::render("flowchart LR\nA --> B").unwrap());
    let state = Rc::new(RefCell::new(DiagramState::new()));
    let invalidate: Rc<dyn Fn()> = Rc::new(|| {});
    let element = view(
        &ArtisanTheme::resolved(),
        "diagram",
        &diagram,
        &state,
        &invalidate,
    );
    let mut app = aqua::UiApp::new(SharedFonts::new(), SharedRuns::new());
    app.mount(move |_| element).unwrap();
    app.set_viewport(600.0, 600.0, 1.0);
    app.build_frame().unwrap();
    let viewport = app
        .tree()
        .sole_node_with_key(hash_key(aqua_ui::element::Key::from("diagram-viewport")))
        .unwrap();
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

fn painted(theme: &ArtisanTheme, diagram: &Arc<DiagramSvg>) -> (Arc<str>, Rect, Rect) {
    use aqua_text::{SharedFonts, SharedRuns};
    let state = Rc::new(RefCell::new(DiagramState::new()));
    let invalidate: Rc<dyn Fn()> = Rc::new(|| {});
    let element = view(theme, "diagram", diagram, &state, &invalidate);
    let mut app = aqua::UiApp::new(SharedFonts::new(), SharedRuns::new());
    app.mount(move |_| element).unwrap();
    app.set_viewport(1200.0, 1200.0, 1.0);
    app.build_frame().unwrap();
    let viewport = app
        .tree()
        .sole_node_with_key(hash_key(aqua_ui::element::Key::from("diagram-viewport")))
        .and_then(|node| app.tree().rect(node))
        .unwrap();
    let (source, bounds) = app
        .tree_mut()
        .emit_scene()
        .into_iter()
        .find_map(|(_, item)| {
            let DisplayItem::GpuiSvg { source, bounds, .. } = item else {
                return None;
            };
            Some((source, bounds))
        })
        .expect("the diagram paints its SVG");
    (source, viewport, bounds)
}

#[test]
fn high_contrast_paints_the_diagram_in_high_contrast_ink() {
    let diagram = Arc::new(crate::ui::markdown::diagram::render("flowchart LR\nA --> B").unwrap());
    for mode in [ThemeMode::Dark, ThemeMode::Light] {
        let (standard, ..) = painted(
            ArtisanTheme::for_mode_and_contrast(mode, Contrast::Standard),
            &diagram,
        );
        let (high, ..) = painted(
            ArtisanTheme::for_mode_and_contrast(mode, Contrast::High),
            &diagram,
        );
        assert_ne!(
            standard, high,
            "{mode:?} high contrast paints the standard diagram"
        );
    }
}

#[test]
fn a_larger_text_size_draws_the_diagram_larger() {
    let diagram = Arc::new(crate::ui::markdown::diagram::render("flowchart LR\nA --> B").unwrap());
    let standard = ArtisanTheme::resolved();
    let mut larger = standard;
    larger.typography = larger.typography.scaled(2.25);
    let (_, standard_viewport, standard_drawn) = painted(&standard, &diagram);
    let (_, larger_viewport, larger_drawn) = painted(&larger, &diagram);
    assert!(
        larger_drawn.w > standard_drawn.w * 1.5,
        "the diagram grew from {standard_drawn:?} to only {larger_drawn:?}"
    );
    assert!(
        larger_viewport.h > standard_viewport.h,
        "the viewport stayed {standard_viewport:?} at 225%"
    );
}
