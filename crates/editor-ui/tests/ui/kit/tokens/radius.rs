use super::close;
use editor_ui::kit::{Corner, Radius, concentric};

#[test]
fn every_radius_is_its_parent_minus_the_inset() {
    for radius in Radius::ALL {
        if let Some((outer, inset)) = radius.parent() {
            assert!(
                close(radius.px(), outer.px() - inset.px()),
                "{radius:?} must be {outer:?} − {inset:?}"
            );
            assert_eq!(
                outer.inset(inset),
                Some(radius),
                "{outer:?} inset by {inset:?}"
            );
        }
    }
}

#[test]
fn corners_resolve_against_height() {
    assert!(
        close(Corner::Full.resolve(28.0), 14.0),
        "full corners are half the height"
    );
    assert!(
        close(
            Corner::Fixed(Radius::Field).resolve(28.0),
            Radius::Field.px()
        ),
        "fixed ignores height"
    );
    assert!(
        close(Corner::Nested(3.0).resolve(28.0), 3.0),
        "nested keeps its value"
    );
    assert!(
        close(concentric(2.0, 6.0), 0.0),
        "insets deeper than the radius clamp to square"
    );
}
