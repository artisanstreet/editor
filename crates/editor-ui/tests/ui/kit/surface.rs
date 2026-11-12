use editor_ui::kit::{Media, Radius, Space, Surface, Tone, Tooltip};

const ALL: [Surface; 14] = [
    Surface::Menu,
    Surface::Tooltip(Tooltip::Brief),
    Surface::Tooltip(Tooltip::Card),
    Surface::Dialog,
    Surface::Card,
    Surface::Inset,
    Surface::Composer,
    Surface::Field,
    Surface::Media(Media::Frame),
    Surface::Media(Media::Thumb),
    Surface::Callout(Tone::Neutral),
    Surface::Callout(Tone::Danger),
    Surface::Pill(24.0),
    Surface::Bubble,
];

#[test]
fn containers_hug_their_children_with_concentric_corners() {
    let expected = [
        (Surface::Menu, Radius::Row),
        (Surface::Inset, Radius::Row),
        (Surface::Card, Radius::Panel),
        (Surface::Dialog, Radius::Panel),
        (Surface::Composer, Radius::Field),
    ];
    for (surface, child) in expected {
        assert_eq!(
            surface.child_radius(),
            Some(child),
            "{surface:?} must nest {child:?}"
        );
    }
}

#[test]
fn the_menu_reproduces_the_account_menu() {
    assert!(
        (Surface::Menu.radius_px() - 10.4).abs() < 1e-4,
        "menu radius"
    );
    assert_eq!(Surface::Menu.padding(), Some(Space::S6), "menu padding");
}

#[test]
fn every_padding_is_a_single_step_on_the_spacing_scale() {
    for surface in ALL {
        let padding = surface.padding_px();
        assert!(
            padding == 0.0 || Space::of(padding).is_some(),
            "{surface:?} pads by {padding}, which is off the scale"
        );
    }
}

#[test]
fn text_only_surfaces_keep_text_off_the_corner_curve() {
    for surface in ALL {
        if surface.child_radius().is_none() && surface.padding().is_some() {
            let curve = surface.radius_px() * (1.0 - std::f32::consts::FRAC_1_SQRT_2);
            assert!(
                surface.padding_px() >= curve,
                "{surface:?} lets text clip the corner: padding {} < {curve}",
                surface.padding_px()
            );
        }
    }
}

#[test]
fn pills_are_fully_round_at_their_height() {
    assert_eq!(Surface::Pill(24.0).radius_px(), 12.0, "half of the height");
    assert_eq!(
        Surface::Pill(24.0).child_radius(),
        None,
        "pills don't nest corners"
    );
}

#[test]
fn a_banded_card_keeps_the_card_padding_outside_and_none_at_its_seams() {
    use editor_ui::kit::surface::band::Band;
    let card = Surface::Card.padding_px();
    let stack = [
        Band::of(true, false),
        Band::of(false, false),
        Band::of(false, true),
    ];
    assert_eq!(stack, [Band::First, Band::Middle, Band::Last], "position picks the band");
    let outside = stack[0].padding().top + stack[2].padding().bottom;
    assert_eq!(outside, 2.0 * card, "the stack's ends carry the card padding");
    let seams = stack
        .windows(2)
        .map(|pair| pair[0].padding().bottom + pair[1].padding().top)
        .sum::<f32>();
    assert_eq!(seams, 0.0, "bands meet without padding between them");
    assert!(
        stack.iter().all(|band| band.padding().left == card && band.padding().right == card),
        "every band keeps the card's side padding"
    );
    assert_eq!(
        Band::of(true, true).padding(),
        Band::Only.inset(card),
        "a lone band is a whole card"
    );
    let corners = Band::Middle.corners(Surface::Card.radius_px());
    assert_eq!(corners.top_left + corners.bottom_right, 0.0, "a middle band is square");
}
