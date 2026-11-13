use editor_ui::kit::{Face, TextRole};
use editor_ui::theme::{ArtisanTheme, ThemeMode};

#[test]
fn roles_resolve_to_one_line_height_per_size() {
    let mut seen: Vec<(f32, f32)> = Vec::new();
    for role in TextRole::ALL {
        let spec = role.spec();
        assert!(
            spec.line >= spec.size,
            "{role:?} line height shorter than its size"
        );
        if let Some((_, line)) = seen
            .iter()
            .find(|(size, _)| *size == spec.size && spec.face == Face::Sans)
        {
            assert_eq!(
                *line, spec.line,
                "{role:?} drifts from the line height of its size"
            );
        }
        if spec.face == Face::Sans {
            seen.push((spec.size, spec.line));
        }
    }
}

#[test]
fn roles_pick_the_family_from_the_theme() {
    let theme = *ArtisanTheme::for_mode(ThemeMode::Dark);
    let label = TextRole::Label.style(&theme);
    let code = TextRole::Code.style(&theme);
    assert_eq!(label.size_px, 14.0, "label size");
    assert_eq!(label.line_height_px, 20.0, "label line");
    assert_ne!(label.families, code.families, "code uses the mono family");
}
