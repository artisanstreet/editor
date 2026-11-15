use super::*;

#[test]
fn changing_face_swaps_from_the_previous_icon() {
    let now = Instant::now();
    let mut swap = PrimarySwap::default();
    swap.observe(Primary::Send, now, false);
    assert_eq!(swap.progress(now), None, "the first face appears without a swap");
    swap.observe(Primary::Stop, now, false);
    assert_eq!(swap.progress(now), Some((Primary::Send, 0.0)));
    assert!(swap.progress(now + DURATION / 2).is_some_and(|(_, p)| p > 0.0 && p < 1.0));
    assert_eq!(swap.progress(now + DURATION), None);
}

#[test]
fn the_same_face_does_not_restart_the_swap() {
    let now = Instant::now();
    let mut swap = PrimarySwap::default();
    swap.observe(Primary::Send, now, false);
    swap.observe(Primary::Stop, now, false);
    swap.observe(Primary::Stop, now + DURATION / 2, false);
    assert!(swap.progress(now + DURATION / 2).is_some_and(|(_, p)| p > 0.4));
}

#[test]
fn reduced_motion_swaps_instantly() {
    let now = Instant::now();
    let mut swap = PrimarySwap::default();
    swap.observe(Primary::Send, now, true);
    swap.observe(Primary::Stop, now, true);
    assert_eq!(swap.progress(now), None);
}

#[test]
fn layers_cross_blur_and_scale() {
    assert_eq!(Layer::incoming(0.0), Layer::outgoing(1.0));
    let start = Layer::incoming(0.0);
    assert_eq!(start.opacity, 0.0);
    assert_eq!(start.blur, BLUR);
    assert!(start.scale < 1.0);
    let end = Layer::incoming(1.0);
    assert_eq!((end.opacity, end.scale, end.blur), (1.0, 1.0, 0.0));
}

#[test]
fn the_prompt_settles_back_in_after_a_send() {
    let now = Instant::now();
    assert_eq!(settle_progress(now, now), Some(0.0));
    assert!(settle_progress(now, now + SETTLE / 2).is_some_and(|p| p > 0.5 && p < 1.0));
    assert_eq!(settle_progress(now, now + SETTLE), None);
}
