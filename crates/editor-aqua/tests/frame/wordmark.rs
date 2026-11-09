use editor_aqua::frame::scramble::Rows;
use editor_aqua::frame::wordmark::{Face, LEAVE_DELAY, Wordmark};
use std::time::{Duration, Instant};

fn text(rows: &Rows) -> Vec<String> {
    rows.iter()
        .map(|row| row.iter().flatten().collect())
        .collect()
}

fn after(start: Instant, ms: u64) -> Instant {
    start + Duration::from_millis(ms)
}

#[test]
fn hovering_spells_toggle_and_leaving_spells_artisan_again() {
    let start = Instant::now();
    let mut wordmark = Wordmark::default();
    assert_eq!(text(&wordmark.shown(start)), ["AR", "TIS", "AN"]);

    wordmark.enter(start, false);
    assert_eq!(wordmark.face(), Face::Swap);
    wordmark.advance(after(start, 300), false);
    assert!(!wordmark.scrambling());
    assert_eq!(text(&wordmark.shown(after(start, 300))), ["TO", "GG", "LE"]);

    wordmark.leave(after(start, 300));
    wordmark.advance(after(start, 300) + LEAVE_DELAY - Duration::from_millis(1), false);
    assert_eq!(wordmark.face(), Face::Swap);
    wordmark.advance(after(start, 300) + LEAVE_DELAY, false);
    assert_eq!(wordmark.face(), Face::Rest);
    assert!(wordmark.scrambling());
    wordmark.advance(after(start, 800), false);
    assert_eq!(text(&wordmark.shown(after(start, 800))), ["AR", "TIS", "AN"]);
}

#[test]
fn returning_within_the_leave_delay_keeps_toggle_without_replaying() {
    let start = Instant::now();
    let mut wordmark = Wordmark::default();
    wordmark.enter(start, false);
    wordmark.advance(after(start, 300), false);
    wordmark.leave(after(start, 300));
    wordmark.enter(after(start, 350), false);
    wordmark.advance(after(start, 600), false);
    assert_eq!(wordmark.face(), Face::Swap);
    assert!(!wordmark.scrambling());
    assert_eq!(wordmark.deadline(), None);
}

#[test]
fn reduced_motion_swaps_the_letters_at_once() {
    let start = Instant::now();
    let mut wordmark = Wordmark::default();
    wordmark.enter(start, true);
    assert!(!wordmark.scrambling());
    assert_eq!(text(&wordmark.shown(start)), ["TO", "GG", "LE"]);
}
