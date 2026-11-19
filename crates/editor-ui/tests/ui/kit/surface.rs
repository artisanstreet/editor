use super::close;
use editor_ui::kit::{Media, Picker, Surface, Tone, Tooltip};

const ALL: [Surface; 17] = [
    Surface::Menu,
    Surface::Picker(Picker::List),
    Surface::Picker(Picker::Sheet),
    Surface::Tooltip(Tooltip::Brief),
    Surface::Tooltip(Tooltip::Card),
    Surface::Dialog,
    Surface::Card,
    Surface::Inset,
    Surface::Code,
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
fn a_banded_card_keeps_the_card_padding_outside_and_none_at_its_seams() {
    use editor_ui::kit::surface::band::Band;
    let card = Surface::Card.padding_px();
    let stack = [
        Band::of(true, false),
        Band::of(false, false),
        Band::of(false, true),
    ];
    assert_eq!(
        stack,
        [Band::First, Band::Middle, Band::Last],
        "position picks the band"
    );
    let outside = stack[0].padding().top + stack[2].padding().bottom;
    assert!(
        close(outside, 2.0 * card),
        "the stack's ends carry the card padding"
    );
    let seams = stack
        .windows(2)
        .map(|pair| pair[0].padding().bottom + pair[1].padding().top)
        .sum::<f32>();
    assert!(close(seams, 0.0), "bands meet without padding between them");
    assert!(
        stack
            .iter()
            .all(|band| close(band.padding().left, card) && close(band.padding().right, card)),
        "every band keeps the card's side padding"
    );
    assert_eq!(
        Band::of(true, true).padding(),
        Band::Only.inset(card),
        "a lone band is a whole card"
    );
    let corners = Band::Middle.corners(Surface::Card.radius_px());
    assert!(
        close(corners.top_left + corners.bottom_right, 0.0),
        "a middle band is square"
    );
}
