use super::*;

const LIFT: f32 = 24.0;

fn centered(now: Instant) -> ComposerGlide {
    let mut glide = ComposerGlide::default();
    glide.new_thread(Posting::Idle, LIFT, now, false);
    glide.observe_centered(500.0, 900.0);
    glide
}

fn thread(id: &str) -> ThreadId {
    ThreadId(id.into())
}

fn sending() -> Posting {
    Posting::Sending(thread("created"))
}

#[test]
fn sending_starts_the_composer_where_it_was_centered() {
    let now = Instant::now();
    let mut glide = centered(now);
    glide.new_thread(sending(), LIFT, now, false);
    assert!(glide.launched());
    assert_eq!(glide.shift(now), -376.0);
    assert!(glide.shift(now + Duration::from_millis(200)) > -376.0);
    assert_eq!(glide.shift(now + Duration::from_millis(400)), 0.0);
}

#[test]
fn a_send_launches_once() {
    let now = Instant::now();
    let mut glide = centered(now);
    glide.new_thread(sending(), LIFT, now, false);
    glide.new_thread(sending(), LIFT, now + Duration::from_millis(100), false);
    assert_eq!(glide.shift(now), -376.0);
    assert!(!glide.awaits(&sending()));
}

#[test]
fn an_accepted_send_keeps_docking_until_its_thread_opens() {
    let now = Instant::now();
    let mut glide = centered(now);
    glide.new_thread(sending(), LIFT, now, false);
    let accepted = Posting::Accepted(thread("created"));
    assert!(!glide.awaits(&accepted));
    glide.new_thread(accepted, LIFT, now, false);
    assert!(glide.launched());
    assert_eq!(glide.shift(now), -376.0);
}

#[test]
fn a_send_that_vanishes_glides_back_to_the_center() {
    let now = Instant::now();
    let mut glide = centered(now);
    glide.new_thread(sending(), LIFT, now, false);
    let docked = now + Duration::from_millis(400);
    assert!(glide.awaits(&Posting::Idle));
    glide.new_thread(Posting::Idle, LIFT, docked, false);
    assert!(!glide.launched());
    assert_eq!(glide.shift(docked), 376.0);
}

#[test]
fn the_sky_sets_while_the_composer_docks_into_the_new_thread() {
    let now = Instant::now();
    let mut glide = centered(now);
    assert!(glide.sky_shown(now));
    glide.new_thread(sending(), LIFT, now, false);
    glide.leave(Some(&thread("created")));
    glide.leave(Some(&thread("created")));
    assert_eq!(glide.sky_opacity(now), 1.0);
    let midway = now + Duration::from_millis(450);
    assert!(glide.sky_opacity(midway) > 0.0 && glide.sky_opacity(midway) < 1.0);
    assert!(glide.sky_setting(midway));
    assert!(glide.shift(now + Duration::from_millis(200)) < 0.0);
    assert!(!glide.sky_shown(now + Duration::from_millis(900)));
}

#[test]
fn opening_another_thread_while_sending_keeps_that_thread_still() {
    let now = Instant::now();
    let mut glide = centered(now);
    glide.new_thread(sending(), LIFT, now, false);
    glide.leave(Some(&thread("existing")));
    let later = now + Duration::from_millis(100);
    assert_eq!(glide.shift(later), 0.0);
    assert!(!glide.sky_setting(later));
}

#[test]
fn opening_another_thread_mid_dock_stops_the_motion() {
    let now = Instant::now();
    let mut glide = centered(now);
    glide.new_thread(sending(), LIFT, now, false);
    glide.leave(Some(&thread("created")));
    glide.leave(Some(&thread("other")));
    let later = now + Duration::from_millis(100);
    assert_eq!(glide.shift(later), 0.0);
    assert!(!glide.sky_setting(later));
}

#[test]
fn opening_an_existing_thread_does_not_move_the_composer() {
    let now = Instant::now();
    let mut glide = centered(now);
    glide.leave(Some(&thread("existing")));
    assert_eq!(glide.shift(now), 0.0);
    assert!(!glide.launched());
    assert!(!glide.sky_setting(now));
}

#[test]
fn a_failed_send_glides_back_to_the_center_and_brings_the_sky_back() {
    let now = Instant::now();
    let mut glide = centered(now);
    glide.new_thread(sending(), LIFT, now, false);
    let docked = now + Duration::from_millis(400);
    assert!(glide.awaits(&Posting::Failed));
    glide.new_thread(Posting::Failed, LIFT, docked, false);
    assert!(!glide.launched());
    assert_eq!(glide.shift(docked), 376.0);
    assert_eq!(glide.shift(docked + Duration::from_millis(400)), 0.0);
    assert_eq!(glide.sky_opacity(docked + Duration::from_millis(900)), 1.0);
}

#[test]
fn retrying_while_the_composer_returns_continues_from_where_it_is() {
    let now = Instant::now();
    let mut glide = centered(now);
    glide.new_thread(sending(), LIFT, now, false);
    let failed = now + Duration::from_millis(400);
    glide.new_thread(Posting::Failed, LIFT, failed, false);
    glide.observe_centered(500.0, 900.0);
    let retry = failed + Duration::from_millis(100);
    let returning = glide.shift(retry);
    assert!(returning > 0.0 && returning < 376.0);
    glide.new_thread(sending(), LIFT, retry, false);
    assert_eq!(glide.shift(retry), returning - 376.0);
}

#[test]
fn reduced_motion_docks_in_place() {
    let now = Instant::now();
    let mut glide = centered(now);
    glide.new_thread(sending(), LIFT, now, true);
    assert_eq!(glide.shift(now), 0.0);
    assert!(!glide.sky_shown(now));
}

#[test]
fn a_send_accepted_before_any_frame_saw_it_still_docks_from_the_center() {
    let now = Instant::now();
    let mut glide = centered(now);
    let created = thread("created");
    glide.follow(
        Route::Thread {
            thread: &created,
            sent: Some(created.clone()),
        },
        LIFT,
        now,
        false,
    );
    assert_eq!(glide.shift(now), -376.0);
    assert!(glide.sky_setting(now + Duration::from_millis(100)));
}

#[test]
fn a_send_for_another_thread_does_not_move_the_opened_one() {
    let now = Instant::now();
    let mut glide = centered(now);
    let existing = thread("existing");
    glide.follow(
        Route::Thread {
            thread: &existing,
            sent: Some(thread("created")),
        },
        LIFT,
        now,
        false,
    );
    assert_eq!(glide.shift(now), 0.0);
    assert!(!glide.sky_setting(now));
}

#[test]
fn a_send_without_a_measured_center_docks_in_place_without_a_sky() {
    let now = Instant::now();
    let mut glide = ComposerGlide::default();
    glide.new_thread(Posting::Idle, LIFT, now, false);
    glide.new_thread(sending(), LIFT, now, false);
    assert!(glide.launched());
    assert_eq!(glide.shift(now), 0.0);
    assert!(!glide.sky_shown(now));
}
