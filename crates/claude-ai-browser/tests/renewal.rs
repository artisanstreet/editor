use std::cell::Cell;
use std::rc::Rc;
use std::time::{Duration, Instant};

use super::*;

struct Task(Rc<Cell<bool>>);

impl Drop for Task {
    fn drop(&mut self) {
        self.0.set(true);
    }
}

#[test]
fn signing_out_while_the_browser_captures_stops_it_and_discards_the_session() {
    let mut renewal = Renewal::default();
    let stopped = Rc::new(Cell::new(false));
    let now = Instant::now();
    let Step::Started(ticket) = renewal.observe(true, now, |_| Task(Rc::clone(&stopped))) else {
        panic!("a renewal the Forge asks for starts");
    };
    assert_eq!(
        renewal.observe(false, now, |_| unreachable!()),
        Step::Cancelled
    );
    assert!(stopped.get(), "the capture kept running after sign-out");
    let handed = renewal.captured(ticket, || {
        unreachable!("a session captured before sign-out was handed over")
    });
    assert!(!handed);
}

#[test]
fn a_session_from_an_earlier_renewal_is_not_handed_over() {
    let mut renewal = Renewal::default();
    let flag = Rc::new(Cell::new(false));
    let start = Instant::now();
    let Step::Started(earlier) = renewal.observe(true, start, |_| Task(Rc::clone(&flag))) else {
        panic!("the first renewal starts");
    };
    renewal.observe(false, start, |_| unreachable!());
    let later = start + Duration::from_mins(11);
    let Step::Started(current) = renewal.observe(true, later, |_| Task(Rc::clone(&flag))) else {
        panic!("a later renewal starts");
    };
    assert!(!renewal.captured(earlier, || unreachable!(
        "the earlier capture was handed over"
    )));
    assert!(renewal.captured(current, || Task(Rc::clone(&flag))));
}
