use artisan_api::Switch;
use editor_core::{Browser, Fonts, SetSettingError, StoredSettingError, Streaming};
use serde_json::json;

#[test]
fn stored_editor_settings_read_as_typed_values_with_their_declared_defaults() {
    let directory = tempfile::tempdir().expect("temporary directory");
    std::fs::write(
        directory.path().join("settings.json"),
        br#"{
            "notifications": "yes",
            "lower_refresh_rate": true,
            "text_scale": 110,
            "high_contrast": "bright",
            "reduced_motion": false,
            "streaming_mode": "swift",
            "browser": "  Helium ",
            "fonts": "system"
        }"#,
    )
    .expect("seed settings.json");
    let core = test_support::editor::core::test_core(directory.path());
    let preferences = core.settings().preferences();

    assert_eq!(preferences.notifications, Switch::Damaged(true));
    assert_eq!(preferences.lower_refresh_rate, Switch::Stored(true));
    assert_eq!(preferences.graphics_vsync, Switch::Unset(true));
    assert_eq!(
        preferences.text_percent, None,
        "a text size the Editor doesn't offer follows the system"
    );
    assert_eq!(preferences.high_contrast, None);
    assert_eq!(preferences.reduced_motion, Some(false));
    assert_eq!(preferences.streaming, Streaming::Swift);
    assert_eq!(preferences.browser, Browser::Named("Helium".to_owned()));
    assert_eq!(
        preferences.fonts,
        Fonts::Spline,
        "fonts is no setting, so it doesn't choose the font set"
    );
    assert!(matches!(
        core.settings().rejected("fonts").map(|error| &**error),
        Some(StoredSettingError::Undeclared { .. })
    ));
}

#[test]
fn the_editor_refuses_a_setting_it_does_not_declare_or_a_value_its_declaration_does_not_take() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let mut core = test_support::editor::core::test_core(directory.path());

    assert!(matches!(
        core.set_setting("text_size", json!(125)),
        Err(SetSettingError::Invalid {
            source: StoredSettingError::Undeclared { .. },
            ..
        })
    ));
    assert!(matches!(
        core.set_setting("high_contrast", json!("bright")),
        Err(SetSettingError::Invalid {
            source: StoredSettingError::Invalid { .. },
            ..
        })
    ));
    assert!(matches!(
        core.set_setting("text_scale", json!(110)),
        Err(SetSettingError::Invalid { .. })
    ));

    core.set_setting("text_scale", json!(150))
        .expect("an offered text size is saved");
    core.set_setting("high_contrast", json!(true))
        .expect("high contrast takes on");
    core.set_setting("reduced_motion", json!(null))
        .expect("reduced motion can follow the system again");
    core.set_setting("browser", json!(""))
        .expect("an empty browser is the system default");
    let preferences = core.settings().preferences();
    assert_eq!(preferences.text_percent, Some(150));
    assert_eq!(preferences.high_contrast, Some(true));
    assert_eq!(preferences.reduced_motion, None);
    assert_eq!(preferences.browser, Browser::SystemDefault);
}
