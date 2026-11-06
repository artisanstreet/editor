use std::sync::Arc;
use std::time::{Duration, Instant};

use editor_aqua::ui::markdown::reveal::PreparedReveal;

#[test]
fn streaming_tail_waits_for_whitespace_and_preserves_unicode_boundaries() {
    let reveal = PreparedReveal::default();
    let now = Instant::now();
    reveal.begin_frame(now, true, true, false, false);
    reveal.set_tail(Some(7));
    let text: Arc<str> = Arc::from("éclair waiting");
    let fades = reveal.fades(7, text.clone());
    assert_eq!(reveal.visible_end(7, &text), "éclair ".len());
    assert!(fades.iter().all(
        |fade| text.is_char_boundary(fade.range.start) && text.is_char_boundary(fade.range.end)
    ));
    assert_eq!(fades.last().unwrap().opacity, 0.0);
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
    assert!(reveal.is_animating());
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
    assert_eq!(reveal.block_entrance(3), Some((0.0, 4.0)));
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
        assert_eq!(fades.last().unwrap().opacity, 0.0);
        reveal.begin_frame(now + Duration::from_millis(200), true, true, false, swift);
        let later = reveal.fades(1, text);
        assert!(later.iter().any(|fade| fade.opacity > 0.0));
    }
}
