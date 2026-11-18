use editor_ui::kit::TextRole;
use editor_ui::preferences::{Preferences, StreamingMode, SystemAccess};
use editor_ui::theme::{ArtisanTheme, Contrast, FontSet, Motion, ThemeMode};

fn preferences(system: SystemAccess) -> Preferences {
    Preferences {
        theme: ThemeMode::Light,
        fonts: FontSet::Spline,
        outline_text: true,
        reduced_motion: None,
        high_contrast: None,
        text_scale: None,
        streaming: StreamingMode::Smooth,
        lower_frame_rate: false,
        vsync: true,
        system,
    }
}

#[test]
fn windows_accessibility_settings_reach_the_theme_unless_overridden() {
    let system = SystemAccess {
        reduced_motion: true,
        high_contrast: true,
        text_scale: 1.5,
    };
    let followed = preferences(system).theme();
    assert_eq!(followed.colors.contrast, Contrast::High);
    assert_eq!(followed.interaction.motion, Motion::Reduced);
    assert_eq!(followed.typography.scale, 1.5);
    let overridden = Preferences {
        reduced_motion: Some(false),
        high_contrast: Some(false),
        text_scale: Some(1.25),
        ..preferences(system)
    }
    .theme();
    assert_eq!(overridden.colors.contrast, Contrast::Standard);
    assert_eq!(overridden.interaction.motion, Motion::Full);
    assert_eq!(overridden.typography.scale, 1.25);
}

fn label_height(theme: &ArtisanTheme) -> f32 {
    use aqua_text::{SharedFonts, SharedRuns};
    use aqua_ui::element::Box as UiBox;
    let text = TextRole::Label
        .text(
            theme,
            "Larger text wraps onto more lines in the same column",
        )
        .wrap(true)
        .build()
        .with_key("label");
    let mut app = aqua::UiApp::new(SharedFonts::new(), SharedRuns::new());
    app.mount(|_| UiBox::new().width(160.0).child(text).build())
        .unwrap();
    app.set_viewport(400.0, 400.0, 1.0);
    app.build_frame().unwrap();
    let node = app
        .tree()
        .sole_node_with_key(aqua_ui::tree::hash_key(aqua_ui::element::Key::from(
            "label",
        )))
        .unwrap();
    app.tree().rect(node).unwrap().h
}

#[test]
fn a_larger_text_size_reflows_labels() {
    let standard = preferences(SystemAccess::default()).theme();
    let larger = preferences(SystemAccess {
        text_scale: 1.75,
        ..SystemAccess::default()
    })
    .theme();
    assert!(label_height(&larger) > label_height(&standard) * 1.5);
}
