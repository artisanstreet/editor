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
