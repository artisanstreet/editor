use editor_aqua::composer::sliding_tab::SlidingTab;
use std::time::{Duration, Instant};

const HEIGHT: f32 = 52.0;

fn at(start: Instant, millis: u64) -> Instant {
    start + Duration::from_millis(millis)
}

#[test]
fn the_tab_keeps_its_height_and_eases_forward_until_it_lands() {
    let start = Instant::now();
    let mut tab = SlidingTab::new(0.0, HEIGHT);
    tab.aim(156.0, start);
    let mut previous = 0.0;
    for millis in (16..250).step_by(16) {
        tab.advance(at(start, millis));
        let (top, bottom) = tab.span();
        assert!((bottom - top - HEIGHT).abs() < 0.001);
        assert!(top >= previous && top < 156.0, "got {top} after {previous}");
        previous = top;
    }
    tab.advance(at(start, 250));
    assert_eq!(tab.span(), (156.0, 208.0));
    assert!(tab.settled());
}

#[test]
fn the_slide_decelerates_into_its_target() {
    let start = Instant::now();
    let mut tab = SlidingTab::new(0.0, HEIGHT);
    tab.aim(100.0, start);
    tab.advance(at(start, 75));
    let early = tab.span().0;
    tab.advance(at(start, 200));
    let late = tab.span().0;
    assert!(early > 50.0, "most of the distance is covered early, got {early}");
    assert!(late - early < early, "the end is slower than the start");
}

#[test]
fn choosing_another_tab_mid_slide_continues_from_where_it_is() {
    let start = Instant::now();
    let mut tab = SlidingTab::new(0.0, HEIGHT);
    tab.aim(156.0, start);
    tab.advance(at(start, 60));
    let reached = tab.span().0;
    tab.aim(52.0, at(start, 60));
    tab.advance(at(start, 60));
    assert_eq!(tab.span().0, reached);
    tab.advance(at(start, 310));
    assert_eq!(tab.span().0, 52.0);
}

#[test]
fn aiming_at_the_same_tab_every_frame_does_not_restart_the_slide() {
    let start = Instant::now();
    let mut tab = SlidingTab::new(0.0, HEIGHT);
    tab.aim(52.0, start);
    tab.aim(52.0, at(start, 200));
    tab.advance(at(start, 250));
    assert!(tab.settled());
    assert_eq!(tab.span().0, 52.0);
}
