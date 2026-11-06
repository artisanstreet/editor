use aqua_core::{Key, Modifiers, NamedKey, Point};
use editor_aqua::capture::script::{Button, Step, parse, parse_keystroke, types_text};
use std::time::Duration;

#[test]
fn script_lines_parse_like_the_gpui_adapter_and_skip_comments() {
    let steps = parse(
        "# header\n\nwait 250\n  move 10 20\nclick 1.5 2\nrclick 3 4\nscroll 5 6 -200\ntype hello world \nsettle\nsettle 500\ncapture out/a.png\nquit\n",
    )
    .unwrap();
    assert_eq!(steps[0], Step::Wait(Duration::from_millis(250)));
    assert_eq!(steps[1], Step::Move(Point::new(10.0, 20.0)));
    assert_eq!(steps[2], Step::Click(Point::new(1.5, 2.0), Button::Left));
    assert_eq!(steps[3], Step::Click(Point::new(3.0, 4.0), Button::Right));
    assert_eq!(steps[4], Step::Scroll(Point::new(5.0, 6.0), -200.0));
    assert_eq!(steps[5], Step::Type("hello world ".to_owned()));
    assert_eq!(steps[7], Step::Settle(Duration::from_millis(500)));
    assert_eq!(steps.len(), 10);
}

#[test]
fn malformed_steps_fail_with_their_line() {
    for bad in [
        "wait -1",
        "move 1",
        "scroll 1 2",
        "click a b",
        "capture",
        "dance",
        "key ctrl-",
    ] {
        assert!(parse(bad).is_err(), "{bad} must not parse");
    }
    assert!(format!("{:#}", parse("wait 1\nmove x y").unwrap_err()).contains("line 2"));
}

#[test]
fn keystrokes_split_modifiers_from_the_key() {
    let stroke = parse_keystroke("ctrl-alt-b").unwrap();
    assert_eq!(stroke.key, Key::Char('b'));
    assert!(
        stroke.modifiers.contains(Modifiers::CONTROL) && stroke.modifiers.contains(Modifiers::ALT)
    );
    assert_eq!(parse_keystroke("ctrl-,").unwrap().key, Key::Char(','));
    assert_eq!(
        parse_keystroke("shift-tab").unwrap().key,
        Key::Named(NamedKey::Tab)
    );
    assert_eq!(
        parse_keystroke("f12").unwrap().key,
        Key::Named(NamedKey::Function(12))
    );
    assert_eq!(types_text(&parse_keystroke("a").unwrap()), Some('a'));
    assert_eq!(types_text(&parse_keystroke("ctrl-k").unwrap()), None);
}
