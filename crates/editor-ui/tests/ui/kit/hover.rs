use editor_ui::kit::Highlight;
use std::time::{Duration, Instant};

#[test]
fn lighting_pops_to_full_and_fades_back_to_rest() {
    let start = Instant::now();
    let lit = Highlight::default().light(true, start, false);
    assert!(lit.lit(), "lighting targets the lit state");
    assert!(lit.running(), "the pop animates");
    let settled = lit.settle(start + Duration::from_secs(1));
    assert_eq!(
        settled.opacity(start + Duration::from_secs(1)),
        1.0,
        "settles fully opaque"
    );
    assert_eq!(
        settled.scale(start + Duration::from_secs(1)),
        1.0,
        "settles at full scale"
    );
    let dark = settled.light(false, start + Duration::from_secs(1), false);
    let later = start + Duration::from_secs(2);
    assert_eq!(dark.settle(later).opacity(later), 0.0, "fades out");
    assert!(
        dark.settle(later).scale(later) < 1.0,
        "shrinks back to its rest scale"
    );
}

#[test]
fn reduced_motion_jumps_without_running() {
    let now = Instant::now();
    let lit = Highlight::default().light(true, now, true);
    assert!(!lit.running(), "reduced motion never animates");
    assert_eq!(lit.opacity(now), 1.0, "and lands immediately");
}

#[test]
fn relighting_an_already_lit_highlight_keeps_its_timeline() {
    let now = Instant::now();
    let lit = Highlight::default().light(true, now, false);
    assert_eq!(
        lit.light(true, now + Duration::from_millis(40), false),
        lit,
        "no restart"
    );
}
