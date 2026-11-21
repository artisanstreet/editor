use editor_aqua::shell::root::reveal::{estimated_row, find_ignoring_case};

#[test]
fn the_match_is_found_in_rendered_text_whatever_its_case() {
    assert_eq!(
        find_ignoring_case("Raising the Scene Bound fixed it.", "scene bound"),
        Some(12..23)
    );
    assert_eq!(find_ignoring_case("Brød og smør", "SMØR"), Some(9..14));
    assert_eq!(find_ignoring_case("BRØD OG SMØR", "smør"), Some(9..14));
    assert_eq!(find_ignoring_case("Ærlig talt", "ærlig"), Some(0..6));
    assert_eq!(find_ignoring_case("nothing here", "scene"), None);
}

#[test]
fn a_hit_deep_in_a_long_message_aims_at_the_row_at_the_same_depth() {
    let rows = [10, 11, 12, 13, 14];
    assert_eq!(estimated_row(&rows, (0, 1000)), Some(10));
    assert_eq!(estimated_row(&rows, (500, 1000)), Some(12));
    assert_eq!(estimated_row(&rows, (990, 1000)), Some(14));
    assert_eq!(estimated_row(&rows, (5, 0)), Some(14));
    assert_eq!(estimated_row(&[], (5, 10)), None);
}

#[test]
fn the_match_folds_full_unicode_case_and_canonical_equivalence() {
    assert_eq!(find_ignoring_case("Sommer: été", "ÉTÉ"), Some(8..13));
    assert_eq!(
        find_ignoring_case("Die Straße ist lang", "STRASSE"),
        Some(4..11)
    );
    assert_eq!(find_ignoring_case("ΣΟΦΟΣ", "σοφος"), Some(0..10));
    assert_eq!(
        find_ignoring_case("cafe\u{301} au lait", "CAFÉ"),
        Some(0..6)
    );
    assert_eq!(
        find_ignoring_case("café au lait", "cafe\u{301}"),
        Some(0..5)
    );
    assert_eq!(find_ignoring_case("café", "cafe"), None);
    assert_eq!(find_ignoring_case("Straße", "s"), Some(0..1));
    assert_eq!(find_ignoring_case("Straße", ""), None);
}
