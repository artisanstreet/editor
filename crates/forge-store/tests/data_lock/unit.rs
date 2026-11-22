use super::{DataLock, LockDataError, Patience, PatienceClock, WaitForDataError};
use std::cell::{Cell, RefCell};
use std::time::{Duration, Instant};

struct Pretended<'a> {
    started: Instant,
    passed: Cell<Duration>,
    pauses: Cell<u32>,
    on_pause: &'a dyn Fn(u32),
}

impl<'a> Pretended<'a> {
    fn new(on_pause: &'a dyn Fn(u32)) -> Self {
        Self {
            started: Instant::now(),
            passed: Cell::new(Duration::ZERO),
            pauses: Cell::new(0),
            on_pause,
        }
    }
}

impl PatienceClock for &Pretended<'_> {
    fn now(&self) -> Instant {
        self.started + self.passed.get()
    }

    fn pause(&self, pause: Duration) {
        self.passed.set(self.passed.get() + pause);
        self.pauses.set(self.pauses.get() + 1);
        (self.on_pause)(self.pauses.get());
    }
}

fn patience<'a>(clock: &'a Pretended<'a>) -> Patience<&'a Pretended<'a>> {
    Patience {
        deadline: Duration::from_secs(60),
        retry: Duration::from_secs(1),
        clock,
    }
}

#[test]
fn taking_the_data_back_waits_for_a_stopping_forge_to_let_go_of_it() {
    let dir = tempfile::tempdir().expect("create the data folder");
    let stopping = RefCell::new(Some(
        DataLock::acquire(dir.path()).expect("the stopping Forge holds its data"),
    ));
    assert!(matches!(
        DataLock::acquire(dir.path()),
        Err(LockDataError::Held { .. })
    ));
    let let_go = |pauses| {
        if pauses == 3 {
            drop(stopping.borrow_mut().take());
        }
    };
    let clock = Pretended::new(&let_go);
    let told = Cell::new(false);

    let taken = DataLock::acquire_waiting(dir.path(), &patience(&clock), || told.set(true));

    taken.expect("the data is taken once the stopping Forge lets go");
    assert!(told.get(), "the installer says it is waiting for the data");
    assert_eq!(clock.pauses.get(), 3);
}

#[test]
fn a_forge_that_never_lets_go_of_its_data_ends_the_wait_with_an_error_naming_the_folder() {
    let dir = tempfile::tempdir().expect("create the data folder");
    let _stuck = DataLock::acquire(dir.path()).expect("a stuck Forge holds its data");
    let ignore = |_| {};
    let clock = Pretended::new(&ignore);

    let taken = DataLock::acquire_waiting(dir.path(), &patience(&clock), || {});

    match taken {
        Err(WaitForDataError::StillHeld { dir: held, waited }) => {
            assert_eq!(held, dir.path());
            assert_eq!(waited, Duration::from_secs(60));
        }
        other => panic!("expected the wait to give up, got {other:?}"),
    }
    assert_eq!(clock.pauses.get(), 60);
}
