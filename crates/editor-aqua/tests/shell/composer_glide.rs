use super::*;

#[test]
fn docking_starts_the_composer_where_it_was_centered() {
    let now = Instant::now();
    let mut glide = ComposerGlide::default();
    glide.observe_centered(500.0, 900.0);
    glide.dock(24.0, now, false);
    assert_eq!(glide.shift(now), -376.0);
    assert!(glide.shift(now + Duration::from_millis(200)) > -376.0);
    assert_eq!(glide.shift(now + Duration::from_millis(400)), 0.0);
}

#[test]
fn docking_happens_once_per_centered_composer() {
    let now = Instant::now();
    let mut glide = ComposerGlide::default();
    glide.observe_centered(500.0, 900.0);
    glide.dock(24.0, now, false);
    glide.dock(24.0, now + Duration::from_millis(100), false);
    assert_eq!(glide.shift(now), -376.0);
}

#[test]
fn an_open_thread_without_a_centered_composer_does_not_move() {
    let now = Instant::now();
    let mut glide = ComposerGlide::default();
    glide.dock(24.0, now, false);
    assert_eq!(glide.shift(now), 0.0);
    glide.observe_centered(500.0, 900.0);
    glide.forget();
    glide.dock(24.0, now, false);
    assert_eq!(glide.shift(now), 0.0);
}

#[test]
fn reduced_motion_docks_in_place() {
    let now = Instant::now();
    let mut glide = ComposerGlide::default();
    glide.observe_centered(500.0, 900.0);
    glide.dock(24.0, now, true);
    assert_eq!(glide.shift(now), 0.0);
}
