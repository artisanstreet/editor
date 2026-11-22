use editor_ui::theme::{ArtisanTheme, Contrast, Oklch, ThemeMode};

fn luminance(color: Oklch) -> f64 {
    let srgb = color.to_srgb();
    let linear = |channel: f32| {
        let channel = f64::from(channel);
        if channel <= 0.04045 {
            channel / 12.92
        } else {
            ((channel + 0.055) / 1.055).powf(2.4)
        }
    };
    0.0722f64.mul_add(
        linear(srgb.b),
        0.7152f64.mul_add(linear(srgb.g), 0.2126 * linear(srgb.r)),
    )
}

fn ratio(foreground: Oklch, background: Oklch) -> f64 {
    let (a, b) = (luminance(foreground), luminance(background));
    (a.max(b) + 0.05) / (a.min(b) + 0.05)
}

fn failures(contrast: Contrast, text_floor: f64, ring_floor: f64) -> Vec<String> {
    let mut failed = Vec::new();
    for mode in [ThemeMode::Light, ThemeMode::Dark] {
        let colors = ArtisanTheme::for_mode_and_contrast(mode, contrast).colors;
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
    assert_eq!(failures(Contrast::Standard, 4.5, 3.0), Vec::<String>::new());
}

#[test]
fn the_high_contrast_palette_meets_wcag_aaa() {
    assert_eq!(failures(Contrast::High, 7.0, 4.5), Vec::<String>::new());
}
