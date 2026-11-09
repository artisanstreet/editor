use std::sync::Arc;
use std::time::{Duration, Instant};

use editor_aqua::ui::markdown::reveal::PreparedReveal;

#[test]
fn smooth_streaming_accepts_each_arrival_without_a_second_batching_clock() {
    let reveal = PreparedReveal::default();
    let now = Instant::now();
    reveal.begin_frame(now, true, true, false, false);
    reveal.set_tail(Some(1));
    reveal.fades(1, Arc::from("first "));
    let text: Arc<str> = Arc::from("first second ");
    reveal.begin_frame(now + Duration::from_millis(16), true, true, false, false);
    reveal.fades(1, text.clone());
    assert_eq!(reveal.visible_end(1, &text), text.len());
}

#[test]
fn an_unbroken_unicode_token_is_visible_while_it_is_still_streaming() {
    for swift in [false, true] {
        let reveal = PreparedReveal::default();
        let now = Instant::now();
        reveal.begin_frame(now, true, true, false, swift);
        reveal.set_tail(Some(1));
        let text: Arc<str> = Arc::from("https://例子.test/一个很长的路径");
        reveal.fades(1, text.clone());
        assert_eq!(reveal.visible_end(1, &text), text.len());
    }
}

#[test]
fn swift_presents_received_text_and_blocks_without_decorative_delay() {
    let reveal = PreparedReveal::default();
    reveal.begin_frame(Instant::now(), true, true, false, true);
    reveal.set_tail(Some(1));
    let text: Arc<str> = Arc::from("ready now");
    assert!(reveal.fades(1, text.clone()).is_empty());
    assert_eq!(reveal.visible_end(1, &text), text.len());
    assert_eq!(reveal.block_entrance(1), None);
    assert_eq!(reveal.block_panel_height(1), 0.0);
    reveal.update_block_height(1, 100.0);
    assert_eq!(reveal.block_panel_height(1), 100.0);
    assert!(!reveal.is_animating());
}

#[test]
fn a_large_arrival_uses_one_fade_and_never_serializes_following_text() {
    let reveal = PreparedReveal::default();
    let now = Instant::now();
    let text: Arc<str> = Arc::from("word ".repeat(4_000));
    reveal.begin_frame(now, true, true, false, false);
    reveal.set_tail(Some(1));
    let fades = reveal.fades(1, text.clone());
    assert_eq!(fades.len(), 1);
    assert!(fades[0].opacity >= 0.35);
    assert!(!reveal.is_settling());
    reveal.begin_frame(now + Duration::from_millis(120), true, true, false, false);
    assert!(reveal.fades(1, text).is_empty());
    assert!(!reveal.is_animating());
}

#[test]
fn streaming_tail_preserves_unicode_without_waiting_for_whitespace() {
    let reveal = PreparedReveal::default();
    let now = Instant::now();
    reveal.begin_frame(now, true, true, false, false);
    reveal.set_tail(Some(7));
    let text: Arc<str> = Arc::from("éclair waiting");
    let fades = reveal.fades(7, text.clone());
    assert_eq!(reveal.visible_end(7, &text), text.len());
    assert!(fades.iter().all(
        |fade| text.is_char_boundary(fade.range.start) && text.is_char_boundary(fade.range.end)
    ));
    assert!(fades.iter().all(|fade| fade.opacity >= 0.35));
    reveal.begin_frame(now + Duration::from_millis(150), false, true, false, false);
    reveal.fades(7, text.clone());
    assert_eq!(reveal.visible_end(7, &text), text.len());
}

#[test]
fn pause_rush_and_reduced_motion_follow_source_ownership() {
    let reveal = PreparedReveal::default();
    let now = Instant::now();
    let text: Arc<str> = Arc::from("one two");
    reveal.begin_frame(now, false, true, false, false);
    reveal.set_paused(true);
    reveal.fades(1, text.clone());
    assert_eq!(reveal.visible_end(1, &text), 0);
    reveal.set_rushed(true);
    reveal.fades(1, text.clone());
    assert_eq!(reveal.visible_end(1, &text), text.len());
    assert!(!reveal.is_animating());
    reveal.begin_frame(now + Duration::from_secs(1), false, true, true, false);
    assert!(reveal.fades(1, text.clone()).is_empty());
    assert_eq!(reveal.visible_end(1, &text), text.len());
}

#[test]
fn geometry_retains_measured_block_and_clips_to_last_reported_line() {
    let reveal = PreparedReveal::default();
    let now = Instant::now();
    reveal.begin_frame(now, false, true, false, false);
    assert!(reveal.update_block_height(3, 120.0));
    let (opacity, offset) = reveal.block_entrance(3).unwrap();
    assert!(opacity > 0.0 && offset > 0.0);
    assert_eq!(reveal.block_panel_height(3), 0.0);
    reveal.report_frontier(Some(124.0));
    reveal.report_frontier(Some(115.0));
    assert!(reveal.update_visible_height(100.0, 80.0));
    assert_eq!(reveal.visible_height(), 24.0);
    reveal.begin_frame(now + Duration::from_millis(250), false, true, false, false);
    assert_eq!(reveal.block_panel_height(3), 120.0);
    reveal.report_frontier(Some(220.0));
    assert!(reveal.update_visible_height(100.0, 80.0));
    assert_eq!(reveal.visible_height(), 80.0);
}

#[test]
fn swift_and_smooth_fades_are_ordered_utf8_opacity_ranges() {
    for swift in [false, true] {
        let reveal = PreparedReveal::default();
        let now = Instant::now();
        let text: Arc<str> = Arc::from("α beta γamma unfinished");
        reveal.begin_frame(now, true, true, false, swift);
        reveal.set_tail(Some(1));
        let fades = reveal.fades(1, text.clone());
        assert!(
            fades
                .windows(2)
                .all(|pair| pair[0].range.end <= pair[1].range.start)
        );
        assert!(fades.iter().all(|fade| {
            fade.range.start < fade.range.end
                && text.is_char_boundary(fade.range.start)
                && text.is_char_boundary(fade.range.end)
                && (0.0..=1.0).contains(&fade.opacity)
        }));
        assert!(fades.iter().all(|fade| fade.opacity >= 0.35));
        reveal.begin_frame(now + Duration::from_millis(200), true, true, false, swift);
        let later = reveal.fades(1, text);
        assert!(later.is_empty());
    }
}
