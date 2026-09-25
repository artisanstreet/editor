use super::*;
use std::time::Duration;

fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("hold test runtime")
}

/// Deterministic permutation so the release order is arbitrary but reproducible.
fn shuffled(len: usize, mut seed: u64) -> Vec<usize> {
    let mut order: Vec<usize> = (0..len).collect();
    for index in (1..len).rev() {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        let other = usize::try_from(seed % (index as u64 + 1)).expect("index fits");
        order.swap(index, other);
    }
    order
}

#[test]
fn many_holds_released_in_any_order_are_idle_exactly_at_the_last_drop() {
    for seed in [1, 7, 0x9E37_79B9_7F4A_7C15] {
        let holds = ConnectionHolds::new();
        let mut live: Vec<Option<Hold>> = (0..32)
            .map(|index| holds.try_hold(HoldKind::ALL[index % HoldKind::ALL.len()]))
            .collect();
        assert!(live.iter().all(Option::is_some));
        assert_eq!(holds.status().count, 32);
        let order = shuffled(live.len(), seed);
        for (released, index) in order.iter().enumerate() {
            assert!(!holds.status().is_idle(), "busy before the last drop");
            drop(live[*index].take());
            assert_eq!(holds.status().count, 31 - released);
        }
        let state = holds.status();
        assert!(state.is_idle());
        assert!(HoldKind::ALL.iter().all(|kind| state.count_of(*kind) == 0));
    }
}

#[test]
fn idle_resolves_immediately_without_holds() {
    let holds = ConnectionHolds::new();
    runtime().block_on(async {
        tokio::time::timeout(Duration::ZERO, holds.idle())
            .await
            .expect("idle at zero resolves on its first poll");
    });
}

#[test]
fn idle_waits_for_the_last_hold() {
    let holds = ConnectionHolds::new();
    let first = holds.try_hold(HoldKind::Message).expect("unsealed");
    let second = holds.try_hold(HoldKind::Answer).expect("unsealed");
    runtime().block_on(async {
        assert!(
            tokio::time::timeout(Duration::from_millis(10), holds.idle())
                .await
                .is_err()
        );
        let waiter = holds.idle();
        drop(second);
        drop(first);
        tokio::time::timeout(Duration::from_secs(1), waiter)
            .await
            .expect("idle after the last drop");
    });
}

#[test]
fn seal_refuses_new_holds_while_existing_holds_complete() {
    let holds = ConnectionHolds::new();
    let existing = holds.try_hold(HoldKind::EngineSettings).expect("unsealed");
    holds.seal();
    assert!(holds.try_hold(HoldKind::Message).is_none());
    let state = holds.status();
    assert!(state.sealed);
    assert_eq!(state.count, 1);
    assert_eq!(state.count_of(HoldKind::EngineSettings), 1);
    drop(existing);
    assert!(holds.status().is_idle());
    assert!(holds.status().sealed, "draining never unseals");
    holds.unseal();
    assert!(!holds.status().sealed);
    let restored = holds.try_hold(HoldKind::Message).expect("unsealed again");
    assert_eq!(restored.kind(), HoldKind::Message);
}

#[test]
fn panics_and_cancellation_release_their_holds() {
    let holds = ConnectionHolds::new();
    let panicking = Arc::clone(&holds);
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
        let _hold = panicking.try_hold(HoldKind::Message).expect("unsealed");
        panic!("handler failed");
    }));
    assert!(outcome.is_err());
    assert!(holds.status().is_idle());

    let hold = holds.try_hold(HoldKind::StopRequest).expect("unsealed");
    runtime().block_on(async {
        let cancelled = tokio::time::timeout(Duration::from_millis(5), async move {
            let _hold = hold;
            std::future::pending::<()>().await;
        })
        .await;
        assert!(cancelled.is_err());
    });
    assert!(holds.status().is_idle());
}

#[test]
fn status_is_observable_synchronously_and_by_subscription() {
    let holds = ConnectionHolds::new();
    let mut changes = holds.subscribe();
    let hold = holds.try_hold(HoldKind::Message).expect("unsealed");
    assert!(changes.has_changed().expect("sender alive"));
    assert_eq!(changes.borrow_and_update().count, 1);
    assert_eq!(holds.status().count, 1);
    holds.seal();
    holds.seal();
    assert!(changes.has_changed().expect("sender alive"));
    assert!(changes.borrow_and_update().sealed);
    drop(hold);
    assert_eq!(changes.borrow_and_update().count, 0);
}

#[test]
fn summaries_name_each_kind_with_its_count() {
    let holds = ConnectionHolds::new();
    assert_eq!(holds.status().summary(), None);
    let message = holds.try_hold(HoldKind::Message);
    assert_eq!(holds.status().summary().as_deref(), Some("1 message"));
    let second = holds.try_hold(HoldKind::Message);
    let setting = holds.try_hold(HoldKind::EngineSettings);
    assert_eq!(
        holds.status().summary().as_deref(),
        Some("2 messages and 1 model setting")
    );
    let answer = holds.try_hold(HoldKind::Answer);
    assert_eq!(
        holds.status().summary().as_deref(),
        Some("2 messages, 1 answer and 1 model setting")
    );
    drop((message, second, setting, answer));
}

#[test]
fn only_mutations_take_holds() {
    let thread = artisan_domain::ThreadId::parse("hold-thread").expect("thread");
    assert_eq!(
        NativeTransportCommand::StopRun(artisan_domain::StopRun::new(
            artisan_domain::RequestId::parse("hold-request").expect("request"),
            thread.clone(),
            artisan_domain::RunId::parse("hold-run").expect("run"),
        ))
        .hold_kind(),
        Some(HoldKind::StopRequest)
    );
    assert_eq!(
        NativeTransportCommand::BeginProjectIntake.hold_kind(),
        Some(HoldKind::ProjectIntake)
    );
    for read in [
        NativeTransportCommand::RequestSnapshot(thread.clone()),
        NativeTransportCommand::Unsubscribe {
            thread_id: thread.clone(),
        },
        NativeTransportCommand::AcknowledgePatch {
            thread_id: thread,
            cursor: artisan_domain::ConversationCursor::new(1),
        },
        NativeTransportCommand::ListRegisteredProfiles,
        NativeTransportCommand::Shutdown,
    ] {
        assert_eq!(read.hold_kind(), None, "{read:?} must not hold");
    }
}
