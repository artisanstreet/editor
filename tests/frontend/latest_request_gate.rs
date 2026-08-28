//! Focused coverage for the native latest-request generation fence.

use std::sync::{Arc, Barrier};
use std::thread;

use artisan_frontend::latest_request_gate::{LatestRequestGate, LatestRequestGateError};

fn assert_send_sync<T: Send + Sync>() {}

#[test]
fn a_new_gate_starts_with_generation_zero_as_current() {
    let gate = LatestRequestGate::new();

    assert!(gate.is_current(0));
    assert!(!gate.is_current(1));
}

#[test]
fn successive_begins_return_newer_generations() {
    let gate = LatestRequestGate::new();

    let first = gate.begin().expect("first request begins");
    let second = gate.begin().expect("second request begins");
    let third = gate.begin().expect("third request begins");

    assert_eq!((first, second, third), (1, 2, 3));
    assert!(gate.is_current(third));
}

#[test]
fn only_the_latest_generation_is_current_after_out_of_order_completions() {
    let gate = LatestRequestGate::new();
    let first = gate.begin().expect("first request begins");
    let second = gate.begin().expect("second request begins");

    // A completion for the older request must be discarded even when it
    // arrives after the newer request has already completed its work.
    assert!(!gate.is_current(first));
    assert!(gate.is_current(second));
    assert!(!gate.is_current(0));
}

#[test]
fn clones_share_one_generation_counter() {
    let gate = LatestRequestGate::new();
    let clone = gate.clone();

    let first = clone.begin().expect("clone begins first request");
    let second = gate.begin().expect("original begins second request");

    assert_eq!((first, second), (1, 2));
    assert!(!clone.is_current(first));
    assert!(gate.is_current(second));
}

#[test]
fn concurrent_begins_are_unique_and_monotonic() {
    const WORKERS: usize = 8;
    const BEGINS_PER_WORKER: usize = 64;

    let gate = LatestRequestGate::new();
    let start = Arc::new(Barrier::new(WORKERS));
    let mut workers = Vec::with_capacity(WORKERS);

    for _ in 0..WORKERS {
        let gate = gate.clone();
        let start = Arc::clone(&start);
        workers.push(thread::spawn(move || {
            let _ = start.wait();
            (0..BEGINS_PER_WORKER)
                .map(|_| gate.begin().expect("concurrent begin succeeds"))
                .collect::<Vec<_>>()
        }));
    }

    let mut generations = workers
        .into_iter()
        .flat_map(|worker| worker.join().expect("worker joins"))
        .collect::<Vec<_>>();
    generations.sort_unstable();

    let total = u64::try_from(WORKERS * BEGINS_PER_WORKER).expect("test count fits in u64");
    let expected = (1..=total).collect::<Vec<_>>();
    assert_eq!(generations, expected);
    assert!(gate.is_current(total));
}

#[test]
fn overflow_refuses_begin_without_wrapping_to_an_old_generation() {
    let gate = LatestRequestGate::from_current_generation(u64::MAX - 1);

    assert_eq!(gate.begin(), Ok(u64::MAX));
    assert!(gate.is_current(u64::MAX));

    assert_eq!(
        gate.begin(),
        Err(LatestRequestGateError::GenerationExhausted)
    );
    assert!(gate.is_current(u64::MAX));
    assert!(!gate.is_current(u64::MAX - 1));
    assert!(!gate.is_current(0));
}

#[test]
fn an_already_exhausted_gate_stays_exhausted() {
    let gate = LatestRequestGate::from_current_generation(u64::MAX);

    assert_eq!(
        gate.begin(),
        Err(LatestRequestGateError::GenerationExhausted)
    );
    assert!(gate.is_current(u64::MAX));
}

#[test]
fn gate_is_send_and_sync_for_shared_request_access() {
    assert_send_sync::<LatestRequestGate>();
    assert_send_sync::<LatestRequestGateError>();
}
