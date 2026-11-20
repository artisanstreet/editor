use artisan_api::{SettingValueError, setting_decl};
use serde_json::json;

#[test]
fn the_font_set_takes_only_the_fonts_the_editor_can_draw() {
    let decl = setting_decl("font_set").expect("font_set is a setting");
    decl.check(&json!("spline")).expect("spline");
    decl.check(&json!("system")).expect("system");
    assert!(matches!(
        decl.check(&json!("serif")),
        Err(SettingValueError::NotAnOption {
            key: "font_set",
            ..
        })
    ));
    decl.check(&json!(3)).expect_err("a number is no font");
}

#[test]
fn a_switch_holding_neither_on_nor_off_follows_its_declared_default_everywhere() {
    use artisan_api::{SETTINGS, SettingKind, Switch, switch_decl};
    for decl in SETTINGS
        .iter()
        .filter(|decl| matches!(decl.kind, SettingKind::Bool))
    {
        let declared = decl
            .default_value()
            .as_bool()
            .expect("a switch declares on or off");
        let switch = switch_decl(decl.key);
        assert_eq!(switch.switch(None), Switch::Unset(declared), "{}", decl.key);
        assert_eq!(
            switch.switch(Some(&json!(!declared))),
            Switch::Stored(!declared),
            "{}",
            decl.key
        );
        assert_eq!(
            switch.switch(Some(&json!("yes"))),
            Switch::Damaged(declared),
            "{}",
            decl.key
        );
        assert_eq!(
            switch.switch(Some(&json!(null))),
            Switch::Damaged(declared),
            "{}",
            decl.key
        );
    }
}
