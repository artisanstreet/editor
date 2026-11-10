use editor_aqua::ui::scroll_edge::{EdgeFade, target};
use std::time::{Duration, Instant};

#[test]
fn an_edge_shows_in_proportion_to_the_content_hidden_past_it() {
    assert_eq!(target(0.0), 0.0);
    assert_eq!(target(8.0), 0.5);
    assert_eq!(target(16.0), 1.0);
    assert_eq!(target(10_000.0), 1.0);
    assert_eq!(target(-5.0), 0.0);
}

#[test]
fn a_fade_eases_toward_its_target_and_settles_exactly() {
    let start = Instant::now();
    let (mut fade, mut moving) = EdgeFade::default().step(1.0, false, start);
    assert!(moving);
    assert_eq!(fade.shown(), 0.0);
    let mut now = start;
    let mut previous = 0.0;
    for _ in 0..200 {
        now += Duration::from_millis(16);
        (fade, moving) = fade.step(1.0, false, now);
        assert!(fade.shown() >= previous);
        previous = fade.shown();
        if !moving {
            break;
        }
    }
    assert!(!moving);
    assert_eq!(fade.shown(), 1.0);
}

#[test]
fn a_fade_can_reverse_midway_without_jumping() {
    let start = Instant::now();
    let (fade, _) = EdgeFade::default().step(1.0, false, start);
    let (fade, _) = fade.step(1.0, false, start + Duration::from_millis(100));
    let before = fade.shown();
    let (fade, moving) = fade.step(0.0, false, start + Duration::from_millis(116));
    assert!(moving);
    assert!(fade.shown() < before + 0.5);
    assert!(fade.shown() > 0.0);
}

#[test]
fn reduced_motion_jumps_straight_to_the_target() {
    let (fade, moving) = EdgeFade::default().step(0.75, true, Instant::now());
    assert!(!moving);
    assert_eq!(fade.shown(), 0.75);
}

#[test]
fn reopening_at_a_saved_scroll_offset_reveals_both_edges_from_clear() {
    use aqua::UiApp;
    use aqua_text::{SharedFonts, SharedRuns};
    use aqua_ui::element::Box as UiBox;
    use editor_aqua::ui::{
        scroll_edge::{Edge, EdgePaint, ScrollEdges, keyed},
        theme::ArtisanTheme,
    };
    let edges = ScrollEdges::default();
    let color = ArtisanTheme::resolved().colors.background;
    let render = || {
        UiBox::new()
            .width(200.0)
            .height(100.0)
            .child(edges.inset_band("top", Edge::Top, color, 0.0))
            .child(edges.inset_band("bottom", Edge::Bottom, color, 0.0))
            .build()
    };
    let mut app = UiApp::new(SharedFonts::new(), SharedRuns::new());
    app.mount(|_| render()).unwrap();
    app.set_viewport(200.0, 100.0, 1.0);
    app.build_frame().unwrap();
    let start = Instant::now();
    for opening in 0..2 {
        edges.reveal();
        app.refresh(vec![render()]).unwrap();
        app.build_frame().unwrap();
        let start = start + Duration::from_secs(opening);
        for (millis, expected_moving) in [(0, true), (80, true), (160, true), (250, false)] {
            let mut alphas = Vec::new();
            for (edge, key) in [(Edge::Top, "top"), (Edge::Bottom, "bottom")] {
                let node = keyed(&app, key).unwrap();
                assert_eq!(
                    edges.advance(
                        &mut app,
                        edge,
                        EdgePaint::InsetShadow,
                        node,
                        color,
                        100.0,
                        false,
                        start + Duration::from_millis(millis)
                    ),
                    expected_moving
                );
                alphas.push(app.tree().paint(node).unwrap().gpui_shadows[0].color.a);
            }
            assert_eq!(alphas[0], alphas[1]);
            match millis {
                0 => assert_eq!(alphas[0], 0.0),
                80 => assert!(alphas[0] > 0.0 && alphas[0] < 0.5),
                160 => assert!(alphas[0] > 0.5 && alphas[0] < 1.0),
                _ => assert_eq!(alphas[0], 1.0),
            }
        }
    }
    edges.reveal();
    for (edge, key) in [(Edge::Top, "top"), (Edge::Bottom, "bottom")] {
        let node = keyed(&app, key).unwrap();
        assert!(!edges.advance(
            &mut app,
            edge,
            EdgePaint::InsetShadow,
            node,
            color,
            100.0,
            true,
            start
        ));
        assert_eq!(app.tree().paint(node).unwrap().gpui_shadows[0].color.a, 1.0);
    }
}
