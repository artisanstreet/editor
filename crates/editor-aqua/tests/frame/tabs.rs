use artisan_api::ThreadId;
use editor_aqua::frame::tabs::Tabs;

#[test]
fn tabs_keep_their_place_and_new_work_joins_at_the_end() {
    let [a, b, c] = [ThreadId::new(), ThreadId::new(), ThreadId::new()];
    let mut tabs = Tabs::default();
    assert!(tabs.sync(&[a.clone(), b.clone()], None));
    assert_eq!(tabs.order(), [a.clone(), b.clone()]);
    assert!(tabs.sync(&[c.clone(), b.clone(), a.clone()], None));
    assert_eq!(tabs.order(), [a.clone(), b.clone(), c.clone()]);
    assert!(!tabs.sync(&[b.clone(), c.clone(), a.clone()], None));
}

#[test]
fn a_settled_thread_keeps_its_tab_only_while_it_is_open() {
    let [a, b] = [ThreadId::new(), ThreadId::new()];
    let mut tabs = Tabs::default();
    tabs.sync(&[a.clone(), b.clone()], Some(&a));
    tabs.sync(std::slice::from_ref(&b), Some(&a));
    assert_eq!(tabs.order(), [a.clone(), b.clone()]);
    tabs.sync(std::slice::from_ref(&b), Some(&b));
    assert_eq!(tabs.order(), std::slice::from_ref(&b));
}

#[test]
fn closing_a_tab_moves_to_its_right_neighbour_then_its_left() {
    let [a, b, c] = [ThreadId::new(), ThreadId::new(), ThreadId::new()];
    let mut tabs = Tabs::default();
    tabs.sync(&[a.clone(), b.clone(), c.clone()], None);
    assert_eq!(tabs.successor(&b), Some(c.clone()));
    assert_eq!(tabs.successor(&c), Some(b.clone()));
    tabs.sync(std::slice::from_ref(&a), None);
    assert_eq!(tabs.successor(&a), None);
}
