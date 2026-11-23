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
        ..SystemAccess::default()
    };
    let followed = preferences(system).theme();
    assert_eq!(followed.colors.contrast, Contrast::High);
    assert_eq!(followed.interaction.motion, Motion::Reduced);
    assert!((followed.typography.scale - 1.5).abs() < f32::EPSILON);
    let overridden = Preferences {
        reduced_motion: Some(false),
        high_contrast: Some(false),
        text_scale: Some(1.25),
        ..preferences(system)
    }
    .theme();
    assert_eq!(overridden.colors.contrast, Contrast::Standard);
    assert_eq!(overridden.interaction.motion, Motion::Full);
    assert!((overridden.typography.scale - 1.25).abs() < f32::EPSILON);
}

#[test]
fn windows_text_size_and_high_contrast_reach_the_theme() {
    use aqua::prelude::{OsPreferences, PrefState, PreferenceSource, TextScale};
    let os = OsPreferences {
        reduce_motion: PrefState::On,
        reduce_transparency: PrefState::Unknown,
        high_contrast: PrefState::On,
        text_scale: TextScale::from_percent(150),
        theme: aqua_protocol::shell::ColorScheme::Unknown,
        source: PreferenceSource::WindowsSystem,
    };
    let theme = preferences(SystemAccess::from_os(&os)).theme();
    assert_eq!(theme.colors.contrast, Contrast::High);
    assert_eq!(theme.interaction.motion, Motion::Reduced);
    assert!((theme.typography.scale - 1.5).abs() < f32::EPSILON);
}

fn text_height(element: aqua_ui::element::Element, text: &str) -> f32 {
    use aqua_text::{SharedFonts, SharedRuns};
    use aqua_ui::element::Box as UiBox;
    use aqua_ui::tree::NodeKind;
    let mut app = aqua::UiApp::new(SharedFonts::new(), SharedRuns::new());
    app.mount(|_| UiBox::new().width(160.0).child(element).build())
        .expect("the element mounts");
    app.set_viewport(400.0, 400.0, 1.0);
    app.build_frame().expect("the element draws");
    let tree = app.tree();
    let node = tree
        .traverse()
        .into_iter()
        .find(|node| {
            matches!(&tree.get(*node).expect("a traversed node exists").kind, NodeKind::Text(shown) if shown.text() == text)
        })
        .unwrap_or_else(|| panic!("{text:?} is shown"));
    tree.rect(node).expect("a shown text has a rect").h
}

fn label(theme: &ArtisanTheme) -> aqua_ui::element::Element {
    TextRole::Label
        .text(
            theme,
            "Larger text wraps onto more lines in the same column",
        )
        .wrap(true)
        .build()
}

fn account_menu(theme: &ArtisanTheme) -> aqua_ui::element::Element {
    use editor_ui::chrome::account::{Account, Identity, Sheet, menu};
    use editor_ui::chrome::limits::{EngineLimits, Gauge, Limit};
    let account = Account {
        identity: Identity::of("Ada Lovelace", "ada@example.com", None),
        avatar: None,
        limits: vec![EngineLimits {
            engine: artisan_api::EngineId::Claude,
            gauge: Gauge::Meters(vec![Limit {
                label: "Week".into(),
                cadence: artisan_api::UsageCadence::Week,
                scope: None,
                used_percent: 40.0,
                resets_at: None,
            }]),
        }],
    };
    menu(
        theme,
        &Sheet {
            account: &account,
            menu: &editor_ui::chrome::menu::Menu::default(),
            width: 160.0,
            now: std::time::Instant::now(),
            emit: std::rc::Rc::new(|_| {}),
        },
    )
}

fn suggested_prompt(theme: &ArtisanTheme) -> aqua_ui::element::Element {
    use editor_ui::text::mount::EditorController;
    use editor_ui::text::{Editor, Mode};
    let controller = EditorController::new(Editor::new("", Mode::Prompt), |_| {});
    controller.set_suggestion(Some("Summarise".into()));
    controller.element(Mode::Prompt, theme, "")
}

#[test]
fn a_larger_text_size_reflows_labels() {
    let standard = preferences(SystemAccess::default()).theme();
    let larger = preferences(SystemAccess {
        text_scale: 1.75,
        ..SystemAccess::default()
    })
    .theme();
    let grows = |build: fn(&ArtisanTheme) -> aqua_ui::element::Element, text: &str| {
        let (larger, standard) = (
            text_height(build(&larger), text),
            text_height(build(&standard), text),
        );
        assert!(
            larger > standard * 1.5,
            "{text:?} grew from {standard} to only {larger}"
        );
    };
    grows(
        label,
        "Larger text wraps onto more lines in the same column",
    );
    grows(account_menu, "Ada Lovelace");
    grows(account_menu, "Week");
    grows(suggested_prompt, "Tab");
}
