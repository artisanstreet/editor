use editor_ui::theme::{ArtisanTheme, Oklch, ThemeMode};

fn luminance(color: Oklch) -> f64 {
    let srgb = color.to_srgb();
    let linear = |channel: f32| {
        let channel = f64::from(channel);
        match channel <= 0.04045 {
            true => channel / 12.92,
            false => ((channel + 0.055) / 1.055).powf(2.4),
        }
    };
    0.2126 * linear(srgb.r) + 0.7152 * linear(srgb.g) + 0.0722 * linear(srgb.b)
}

fn ratio(foreground: Oklch, background: Oklch) -> f64 {
    let (a, b) = (luminance(foreground), luminance(background));
    (a.max(b) + 0.05) / (a.min(b) + 0.05)
}

fn failures(text_floor: f64, ring_floor: f64) -> Vec<String> {
    let mut failed = Vec::new();
    for mode in [ThemeMode::Light, ThemeMode::Dark] {
        let colors = ArtisanTheme::for_mode(mode).colors;
        let grounds = [
            ("background", colors.background),
            ("surface", colors.surface),
            ("raised", colors.raised),
        ];
        let inks = [
            ("text", colors.text, text_floor),
            ("text_muted", colors.text_muted, text_floor),
            ("text_faint", colors.text_faint, text_floor),
            ("link", colors.link, text_floor),
            ("ring", colors.ring, ring_floor),
        ];
        for (ink_name, ink, floor) in inks {
            for (ground_name, ground) in grounds {
                let measured = ratio(ink, ground);
                if measured < floor {
                    failed.push(format!(
                        "{mode:?} {ink_name} on {ground_name}: {measured:.2} < {floor}"
                    ));
                }
            }
        }
    }
    failed
}

#[test]
fn text_and_focus_tokens_meet_wcag_aa_on_every_surface() {
    assert_eq!(failures(4.5, 3.0), Vec::<String>::new());
}
