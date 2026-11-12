use editor_ui::kit::{Corner, Inset, Radius, concentric};

fn close(a: f32, b: f32) -> bool {
    (a - b).abs() < 1e-4
}

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
fn the_scale_matches_the_account_menu() {
    let expected = [
        (Radius::Row, 4.4),
        (Radius::Field, 6.4),
        (Radius::Panel, 10.4),
        (Radius::Well, 14.4),
        (Radius::Shell, 18.4),
    ];
    for (radius, px) in expected {
        assert!(
            close(radius.px(), px),
            "{radius:?} resolves to {}",
            radius.px()
        );
    }
}

#[test]
fn only_the_shell_is_a_root() {
    let roots = Radius::ALL
        .into_iter()
        .filter(|radius| radius.parent().is_none())
        .collect::<Vec<_>>();
    assert_eq!(
        roots,
        vec![Radius::Shell],
        "a single root keeps the scale concentric"
    );
}

#[test]
fn insets_without_a_named_child_have_no_radius() {
    assert_eq!(
        Radius::Row.inset(Inset::Menu),
        None,
        "nothing nests inside a row"
    );
    assert_eq!(
        Radius::Panel.inset(Inset::Frame),
        None,
        "panel − 8 is not on the scale"
    );
}

#[test]
fn corners_resolve_against_height() {
    assert!(
        close(Corner::Full.resolve(28.0), 14.0),
        "full corners are half the height"
    );
    assert!(
        close(Corner::Fixed(Radius::Field).resolve(28.0), 6.4),
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
