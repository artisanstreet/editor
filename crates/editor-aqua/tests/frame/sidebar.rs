use artisan_api::{ProjectId, ThreadId};
use editor_aqua::frame::sidebar::{Event, Sidebar};
use std::time::Instant;

#[test]
fn switching_tabs_preserves_manually_opened_and_closed_project_groups() {
    let mut sidebar = Sidebar::default();
    let now = Instant::now();
    let project = ProjectId::new();
    let other_project = ProjectId::new();
    let first = ThreadId::new();
    let second = ThreadId::new();
    let working = [first.clone(), second.clone()];
    sidebar.follow(&working, Some(&first));
    assert_eq!(sidebar.openness(&project, now), 0.0);
    assert_eq!(sidebar.openness(&other_project, now), 0.0);
    (sidebar.emit())(Event::Project(project.clone()));
    sidebar.take_events(now, true);
    sidebar.follow(&working, Some(&second));
    assert_eq!(sidebar.openness(&project, now), 1.0);
    assert_eq!(sidebar.openness(&other_project, now), 0.0);
    (sidebar.emit())(Event::Project(project.clone()));
    sidebar.take_events(now, true);
    sidebar.tick(now, true);
    sidebar.follow(&working, Some(&first));
    assert_eq!(sidebar.openness(&project, now), 0.0);
    assert_eq!(sidebar.tabs().order(), working);
    sidebar.follow(&working, None);
    assert_eq!(sidebar.openness(&project, now), 0.0);
}

#[test]
fn narrow_windows_force_collapse_and_restore_the_preferred_open_state() {
    use editor_aqua::frame::sidebar::{COLLAPSED_WIDTH, OPEN_WIDTH};
    let mut sidebar = Sidebar::default();
    let now = Instant::now();
    sidebar.fit(1200.0, now, true);
    assert!(sidebar.open());
    sidebar.fit(900.0, now, true);
    assert!(!sidebar.open());
    assert_eq!(sidebar.width(now), COLLAPSED_WIDTH);
    (sidebar.emit())(Event::Toggle);
    sidebar.take_events(now, true);
    assert!(!sidebar.open());
    sidebar.fit(980.0, now, true);
    assert!(!sidebar.open());
    sidebar.fit(1000.0, now, true);
    assert!(sidebar.open());
    assert_eq!(sidebar.width(now), OPEN_WIDTH);
    (sidebar.emit())(Event::Toggle);
    sidebar.take_events(now, true);
    sidebar.fit(720.0, now, true);
    sidebar.fit(1200.0, now, true);
    assert!(!sidebar.open());
    assert_eq!(sidebar.width(now), COLLAPSED_WIDTH);
}

#[test]
fn responsive_collapse_keeps_room_for_the_thread_during_interrupted_motion() {
    use editor_aqua::frame::sidebar::{COLLAPSED_WIDTH, OPEN_WIDTH, RESIZE};
    use std::time::Duration;
    let mut sidebar = Sidebar::default();
    let now = Instant::now();
    sidebar.fit(720.0, now, false);
    assert!(!sidebar.open());
    assert!(sidebar.animating());
    assert!(720.0 - sidebar.width(now) - 12.0 >= 600.0);
    let halfway = now + RESIZE.duration / 2;
    let interrupted_width = sidebar.width(halfway);
    sidebar.fit(1200.0, halfway, false);
    assert!(sidebar.open());
    assert_eq!(sidebar.width(halfway), interrupted_width);
    sidebar.fit(720.0, halfway + Duration::from_millis(10), false);
    assert!(!sidebar.open());
    assert!(720.0 - sidebar.width(halfway) - 12.0 >= 600.0);
    let settled = now + Duration::from_secs(2);
    sidebar.tick(settled, false);
    assert_eq!(sidebar.width(settled), COLLAPSED_WIDTH);
    sidebar.fit(1200.0, settled, true);
    assert_eq!(sidebar.width(settled), OPEN_WIDTH);
    assert!(!sidebar.animating());
    sidebar.fit(720.0, settled, false);
    sidebar.fit(720.0, settled, true);
    assert_eq!(sidebar.width(settled), COLLAPSED_WIDTH);
    assert!(!sidebar.animating());
}
