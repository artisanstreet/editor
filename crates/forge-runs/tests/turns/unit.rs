use super::*;
use artisan_api::{EngineId, Run, RunOutcome, RunState, Usage};
use std::time::Duration;
use tokio_util::sync::CancellationToken;

fn thread() -> ThreadId {
    ThreadId("thread-1".to_owned())
}

fn live(id: &RunId, retries: u32) -> LiveRun {
    let (inbox, _receiver) = tokio::sync::mpsc::channel(1);
    LiveRun {
        id: id.clone(),
        inbox,
        cancel: CancellationToken::new(),
        engine: None,
        launched: None,
        live: std::collections::BTreeMap::new(),
        foreground_bytes: 0,
        terminal_bytes: 0,
        phase: Phase::Working,
        stop: StopSignal::default(),
        retries,
    }
}

fn claimed(turns: &mut Turns) -> RunId {
    let id = RunId::new();
    turns.claim(thread(), |retries| live(&id, retries));
    id
}

fn settlement(run: &RunId, outcome: RunOutcome) -> Box<Settlement> {
    let mut ended = Run {
        config: None,
        id: run.clone(),
        thread_id: thread(),
        engine: EngineId::Claude,
        model: None,
        input_cutoff: 0,
        state: RunState::Running,
        usage: Usage::default(),
        session_id: Some("session".to_owned()),
        seq: 0,
        started_at: 0,
        continuation: None,
    };
    ended.finish(outcome, None, 1).unwrap();
    Box::new(Settlement {
        run: ended,
        open: Vec::new(),
        attempted: Vec::new(),
        park: Some(forge_store::Park {
            continuation: artisan_api::Continuation::Overloaded,
            due_at: 60,
        }),
        note: None,
    })
}

fn disk_full(run: &RunId) -> Arc<SettleRunError> {
    Arc::new(SettleRunError {
        run: run.clone(),
        thread: thread(),
        cause: Box::new(forge_store::WriteFailure::ThreadNotFound { thread: thread() }),
    })
}

fn unsaved(turns: &mut Turns, outcome: RunOutcome) -> RunId {
    let run = claimed(turns);
    turns.end(
        &thread(),
        &run,
        Ended::Unsaved {
            settlement: settlement(&run, outcome),
            error: disk_full(&run),
        },
        Instant::now(),
    );
    run
}

#[tokio::test(start_paused = true)]
async fn a_start_that_keeps_failing_waits_twice_as_long_each_time_up_to_a_minute() {
    let mut turns = Turns::default();
    let mut delays = Vec::new();
    for _ in 0..9 {
        let run = claimed(&mut turns);
        let failed_at = Instant::now();
        turns.end(&thread(), &run, Ended::Deferred, failed_at);
        let due = turns
            .next_due(failed_at)
            .expect("a failed start waits before it is retried");
        assert_eq!(
            turns.waiting(failed_at).collect::<Vec<_>>(),
            [&thread()],
            "the thread is not started again before its wait is over"
        );
        delays.push(due - failed_at);
        tokio::time::advance(due - failed_at).await;
        assert_eq!(
            turns.waiting(Instant::now()).count(),
            0,
            "the thread may start once its wait is over"
        );
        assert_eq!(
            turns.next_due(Instant::now()),
            None,
            "a wait that is over does not wake the dispatcher again"
        );
    }

    let seconds: Vec<_> = delays.iter().map(Duration::as_secs).collect();
    assert_eq!(seconds, [1, 2, 4, 8, 16, 32, 60, 60, 60]);
}

#[tokio::test(start_paused = true)]
async fn a_new_message_or_a_continue_starts_a_failing_thread_without_its_wait() {
    let mut turns = Turns::default();
    for _ in 0..3 {
        let run = claimed(&mut turns);
        turns.end(&thread(), &run, Ended::Deferred, Instant::now());
    }
    turns.undefer(&thread());
    let run = claimed(&mut turns);
    turns.end(&thread(), &run, Ended::Deferred, Instant::now());
    assert_eq!(
        turns
            .next_due(Instant::now())
            .map(|due| due - Instant::now()),
        Some(Duration::from_secs(1)),
        "a new message starts the backoff over"
    );

    assert_eq!(turns.carry_on(&thread()), Continued::Idle);
    assert_eq!(
        turns.waiting(Instant::now()).count(),
        1,
        "asking whether a Continue applies leaves the wait until the Continue is accepted"
    );
    turns.undefer(&thread());
    assert_eq!(turns.waiting(Instant::now()).count(), 0);
    assert!(!turns.holds(&thread()));
}

#[tokio::test]
async fn a_run_whose_end_could_not_be_saved_holds_its_thread_until_it_is() {
    let mut turns = Turns::default();
    let run = claimed(&mut turns);
    let Stopped::Live(stop) = turns.stop(&thread(), 7) else {
        panic!("a live run is stopped live");
    };
    turns.end(
        &thread(),
        &run,
        Ended::Unsaved {
            settlement: settlement(&run, RunOutcome::Cancelled),
            error: disk_full(&run),
        },
        Instant::now(),
    );

    stop.saved()
        .await
        .expect_err("the Stop learns its save failed");
    assert!(
        turns.holds(&thread()),
        "no new run starts on an unsaved end"
    );
    assert!(!turns.contains_key(&thread()), "the run itself is over");
    assert_eq!(
        turns
            .unsettled()
            .iter()
            .map(|settlement| settlement.run.id.clone())
            .collect::<Vec<_>>(),
        [run]
    );
    turns.settled(&thread());
    assert!(!turns.holds(&thread()));
    assert!(turns.unsettled().is_empty());
}

#[tokio::test]
async fn a_stop_is_acknowledged_by_the_first_save_of_the_stop() {
    let mut turns = Turns::default();
    let run = claimed(&mut turns);
    let Stopped::Live(stop) = turns.stop(&thread(), 7) else {
        panic!("a live run is stopped live");
    };
    let signal = turns.get(&thread()).unwrap().stop.clone();
    assert!(signal.is_requested());
    signal.resolve(Ok(()));
    turns.end(
        &thread(),
        &run,
        Ended::Unsaved {
            settlement: settlement(&run, RunOutcome::Cancelled),
            error: disk_full(&run),
        },
        Instant::now(),
    );

    stop.saved()
        .await
        .expect("the stop was saved before the run's later write failed");
}

#[tokio::test]
async fn a_stop_waits_for_the_run_to_end_when_nothing_announced_it() {
    let mut turns = Turns::default();
    let run = claimed(&mut turns);
    let Stopped::Live(stop) = turns.stop(&thread(), 7) else {
        panic!("a live run is stopped live");
    };
    let mut waiting = Box::pin(stop.saved());
    assert!(
        futures::poll!(&mut waiting).is_pending(),
        "Stop does not return before anything is saved"
    );
    turns.end(&thread(), &run, Ended::Settled, Instant::now());
    waiting.await.expect("the run's end saves the stop");
}

#[test]
fn stop_and_continue_take_turns_on_a_stopping_run() {
    let mut turns = Turns::default();
    let run = claimed(&mut turns);
    assert_eq!(turns.carry_on(&thread()), Continued::Busy);

    assert!(matches!(turns.stop(&thread(), 3), Stopped::Live(_)));
    assert_eq!(turns.carry_on(&thread()), Continued::Record);
    turns.continued(&thread());
    assert_eq!(
        turns.phase(&thread(), &run),
        Phase::Stopping {
            through: 3,
            then: AfterStop::Continue
        }
    );
    assert_eq!(
        turns.carry_on(&thread()),
        Continued::Recorded,
        "pressing Continue twice records it once"
    );

    assert!(matches!(turns.stop(&thread(), 5), Stopped::Live(_)));
    assert_eq!(
        turns.phase(&thread(), &run),
        Phase::Stopping {
            through: 5,
            then: AfterStop::Settle
        },
        "the last Stop wins over the Continue"
    );
}

#[test]
fn continue_on_an_unsaved_end_depends_on_whether_it_was_a_stop() {
    let mut turns = Turns::default();
    unsaved(&mut turns, RunOutcome::Cancelled);
    assert_eq!(turns.carry_on(&thread()), Continued::Record);
    turns.continued(&thread());
    assert!(turns.holds(&thread()));
    turns.settled(&thread());

    unsaved(&mut turns, RunOutcome::Done);
    assert_eq!(turns.carry_on(&thread()), Continued::NothingStopped);
}

#[test]
fn a_stop_on_an_unsaved_failed_run_drops_its_automatic_retry() {
    let mut turns = Turns::default();
    unsaved(&mut turns, RunOutcome::Failed);

    let Stopped::Unsettled(stopped) = turns.stop(&thread(), 9) else {
        panic!("an unsaved end is stopped by saving it");
    };

    assert_eq!(stopped.park, None);
    assert_eq!(
        turns
            .unsettled()
            .iter()
            .map(|settlement| settlement.park)
            .collect::<Vec<_>>(),
        [None],
        "the background save no longer parks the stopped turn either"
    );
}
