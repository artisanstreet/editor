use artisan_api::{EngineId, Run, RunId, RunStatus, ThreadId, Usage};
use editor_aqua::core::host::stopping::{Held, Stopping};

fn run(id: &str, status: RunStatus) -> Run {
    Run {
        config: None,
        id: RunId(id.into()),
        thread_id: ThreadId("thread".into()),
        engine: EngineId::Claude,
        model: None,
        input_cutoff: 0,
        status,
        usage: Usage::default(),
        session_id: None,
        error: None,
        seq: 1,
        started_at: 0,
        ended_at: None,
        continuation: None,
    }
}

#[test]
fn stopping_settles_a_live_run_at_once() {
    let mut live = run("a", RunStatus::Running);
    assert!(Stopping::begin(&mut live, 50).is_some());
    assert_eq!(live.status, RunStatus::Cancelled);
    assert_eq!(live.ended_at, Some(50));
    let mut done = run("a", RunStatus::Done);
    assert!(Stopping::begin(&mut done, 50).is_none());
    assert_eq!(done.status, RunStatus::Done);
}

#[test]
fn late_live_updates_of_the_stopped_run_stay_stopped() {
    let stopping = Stopping::begin(&mut run("a", RunStatus::Running), 50).unwrap();
    let mut update = run("a", RunStatus::Running);
    update.seq = 9;
    let Held::Stopping(held) = stopping.hold(update) else {
        panic!("a late live update must not restart the spinner");
    };
    assert_eq!((held.status, held.ended_at, held.seq), (RunStatus::Cancelled, Some(50), 9));
}

#[test]
fn forge_confirming_the_stop_or_a_new_run_ends_the_override() {
    let stopping = Stopping::begin(&mut run("a", RunStatus::Running), 50).unwrap();
    let mut confirmed = run("a", RunStatus::Cancelled);
    confirmed.ended_at = Some(80);
    assert_eq!(stopping.hold(confirmed.clone()), Held::Settled(confirmed));
    let next = run("b", RunStatus::Running);
    assert_eq!(stopping.hold(next.clone()), Held::Other(next));
}

#[test]
fn a_refused_stop_puts_the_run_back_to_work() {
    let mut live = run("a", RunStatus::Running);
    let stopping = Stopping::begin(&mut live, 50).unwrap();
    stopping.revert(&mut live);
    assert_eq!((live.status, live.ended_at), (RunStatus::Running, None));
    let mut settled = run("a", RunStatus::Cancelled);
    settled.ended_at = Some(80);
    stopping.revert(&mut settled);
    assert_eq!(settled.status, RunStatus::Cancelled);
}
