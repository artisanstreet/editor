use editor_aqua::frame::menu::{Effect, Event, Menu, REST_SCALE, Spot};
use std::time::{Duration, Instant};

fn settled(menu: &mut Menu, now: Instant) -> Instant {
    let later = now + Duration::from_secs(1);
    menu.tick(later);
    later
}

#[test]
fn theme_flips_without_closing_the_menu() {
    let now = Instant::now();
    let mut menu = Menu::default();
    menu.apply(Event::Pick(Spot::Trigger), now, false);
    assert!(menu.open());
    assert_eq!(
        menu.apply(Event::Pick(Spot::Theme), now, false),
        Some(Effect::FlipTheme)
    );
    assert!(menu.open());
}

#[test]
fn leaving_actions_close_the_menu_and_report_their_effect() {
    let now = Instant::now();
    for (spot, effect) in [
        (Spot::Settings, Some(Effect::OpenSettings)),
        (Spot::SignOut, Some(Effect::SignOut)),
        (Spot::Help, None),
        (Spot::Upgrade, None),
    ] {
        let mut menu = Menu::default();
        menu.apply(Event::Toggle, now, false);
        assert_eq!(menu.apply(Event::Pick(spot), now, false), effect);
        assert!(!menu.open(), "{spot:?} should close the menu");
    }
}

#[test]
fn the_trigger_stays_lit_while_open_and_entries_unlight_on_close() {
    let now = Instant::now();
    let mut menu = Menu::default();
    menu.apply(Event::Toggle, now, false);
    menu.apply(Event::Hover(Spot::Settings, true), now, false);
    let now = settled(&mut menu, now);
    assert!(menu.highlight(Spot::Trigger).lit());
    assert_eq!(menu.highlight(Spot::Settings).opacity(now), 1.0);
    assert_eq!(menu.highlight(Spot::Settings).scale(now), 1.0);
    menu.apply(Event::Dismissed, now, false);
    let now = settled(&mut menu, now);
    assert!(!menu.highlight(Spot::Trigger).lit());
    assert_eq!(menu.highlight(Spot::Settings).opacity(now), 0.0);
    assert_eq!(menu.highlight(Spot::Settings).scale(now), REST_SCALE);
}

#[test]
fn closing_keeps_the_trigger_lit_while_the_pointer_is_on_it() {
    let now = Instant::now();
    let mut menu = Menu::default();
    menu.apply(Event::Hover(Spot::Trigger, true), now, false);
    menu.apply(Event::Toggle, now, false);
    menu.apply(Event::Toggle, now, false);
    assert!(menu.highlight(Spot::Trigger).lit());
}

#[test]
fn highlights_grow_slower_than_they_shrink() {
    let start = Instant::now();
    let mut menu = Menu::default();
    menu.apply(Event::Hover(Spot::Help, true), start, false);
    let quick = start + Duration::from_millis(150);
    assert_eq!(menu.highlight(Spot::Help).opacity(quick), 1.0);
    assert!(menu.highlight(Spot::Help).scale(quick) < 1.0);
    let now = settled(&mut menu, start);
    menu.apply(Event::Hover(Spot::Help, false), now, false);
    let quick = now + Duration::from_millis(150);
    assert!((menu.highlight(Spot::Help).scale(quick) - REST_SCALE).abs() < 1e-6);
}

#[test]
fn reduced_motion_jumps_straight_to_the_end_state() {
    let now = Instant::now();
    let mut menu = Menu::default();
    menu.apply(Event::Toggle, now, true);
    menu.apply(Event::Hover(Spot::Theme, true), now, true);
    assert!(!menu.animating());
    assert_eq!(menu.highlight(Spot::Theme).opacity(now), 1.0);
}
