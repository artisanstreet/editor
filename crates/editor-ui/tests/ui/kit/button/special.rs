use editor_ui::kit::button::special::{SEGMENTS, svg, wedges};

#[test]
fn wedges_tile_the_full_turn_without_gaps() {
    let fan = wedges(0.0, SEGMENTS);
    assert_eq!(fan.len(), SEGMENTS, "one wedge per segment");
    assert_eq!(fan[0].from_degrees, 0.0, "starts at the top");
    assert!(
        (fan[SEGMENTS - 1].to_degrees - 360.0).abs() < 1e-3,
        "ends a full turn later"
    );
    for pair in fan.windows(2) {
        assert!(
            (pair[0].to_degrees - pair[1].from_degrees).abs() < 1e-3,
            "contiguous"
        );
    }
}

#[test]
fn orbiting_rotates_the_fan_but_keeps_its_colours() {
    let rest = wedges(0.0, SEGMENTS);
    let turned = wedges(90.0, SEGMENTS);
    for (a, b) in rest.iter().zip(&turned) {
        assert_eq!(a.color, b.color, "colours ride with their wedge");
        assert!(
            (b.from_degrees - a.from_degrees - 90.0).abs() < 1e-3,
            "rotated by the orbit"
        );
    }
}

#[test]
fn the_svg_clips_the_fan_to_a_one_pixel_ring() {
    let source = svg(120.0, 32.0, 6.4, 0.0);
    assert_eq!(
        source.matches("<path d=\"M60 16L").count(),
        SEGMENTS,
        "every wedge is drawn"
    );
    assert!(
        source.contains("clip-rule=\"evenodd\""),
        "the ring is outer minus inner"
    );
    assert!(
        source.contains("radialGradient"),
        "the body keeps its radial glow"
    );
    let tree = usvg::Tree::from_str(&source, &usvg::Options::default());
    assert!(
        tree.is_ok(),
        "the renderer accepts the rim: {:?}",
        tree.err()
    );
}
