//! Service command loop tests: batch forwarding and connection holds carried
//! by admitted commands through their handlers.

#![forbid(unsafe_code)]

use super::{
    CommandSendError, FrameFactory, NativeTransportCommand, NativeTransportEvent,
    NativeTransportService, PrivateDelivery, QueuedCommand, ServiceFailure, ServiceRuntime,
    command_loop_with_delivery,
};
use artisan_domain::{
    ConversationCursor, ConversationLifecycle, ConversationPatch, MessageBody, PatchBatch, PatchId,
    PatchSequence, QueueFirstMessage, RequestId, Revision, ThreadId, TurnId, TurnOrdinal,
    UnixMillis,
};
use std::{
    sync::mpsc::{Receiver, SyncSender, sync_channel},
    time::{Duration, Instant},
};

fn batch(thread: &ThreadId, from: u64, to: u64) -> PatchBatch {
    let turn = artisan_domain::ConversationTurn {
        turn_id: TurnId::parse("turn-a").expect("turn"),
        ordinal: TurnOrdinal::new(0),
        revision: Revision::new(0),
        lifecycle: ConversationLifecycle::Pending,
        created_at: UnixMillis::EPOCH,
        updated_at: UnixMillis::EPOCH,
    };
    let patch = ConversationPatch::TurnUpsert {
        patch_id: PatchId::parse("patch-a").expect("patch"),
        sequence: PatchSequence::new(to).expect("sequence"),
        turn,
    };
    PatchBatch::new(
        thread.clone(),
        ConversationCursor::new(from),
        ConversationCursor::new(to),
        vec![patch],
    )
    .expect("batch")
}

async fn next_patch(events: &Receiver<NativeTransportEvent>) -> PatchBatch {
    for _ in 0..10_000 {
        match events.try_recv() {
            Ok(NativeTransportEvent::PatchBatch(batch)) => return batch,
            Ok(_) | Err(std::sync::mpsc::TryRecvError::Empty) => tokio::task::yield_now().await,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                panic!("event bridge closed before both batches published");
            }
        }
    }
    panic!("second contiguous batch was not published before any UI ack");
}

fn test_runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("loop test runtime")
}

#[test]
fn two_contiguous_batches_publish_before_delayed_ack_without_reconnect() {
    test_runtime().block_on(async {
        let mut service = ServiceRuntime::new_for_batch_tests();
        let thread = ThreadId::parse("loop-thread").expect("thread");
        service
            .custody
            .on_subscribe(thread.clone(), Some(ConversationCursor::new(5)));
        let (command_tx, mut command_rx) = tokio::sync::mpsc::channel::<QueuedCommand>(8);
        let (delivery_tx, mut delivery_rx) = tokio::sync::mpsc::channel::<PrivateDelivery>(8);
        let (event_tx, event_rx) = sync_channel::<NativeTransportEvent>(16);
        let mut frames = FrameFactory::new();
        let join = tokio::spawn(async move {
            let outcome = command_loop_with_delivery(
                &mut command_rx,
                &mut delivery_rx,
                &mut service,
                &mut frames,
                &event_tx,
            )
            .await;
            (outcome, service.custody)
        });
        delivery_tx
            .send(PrivateDelivery::Batch(batch(&thread, 5, 6)))
            .await
            .expect("first batch");
        let first = next_patch(&event_rx).await;
        assert_eq!(first.from_cursor(), ConversationCursor::new(5));
        assert_eq!(first.to_cursor(), ConversationCursor::new(6));
        delivery_tx
            .send(PrivateDelivery::Batch(batch(&thread, 6, 7)))
            .await
            .expect("second batch");
        let second = next_patch(&event_rx).await;
        assert_eq!(second.from_cursor(), ConversationCursor::new(6));
        assert_eq!(second.to_cursor(), ConversationCursor::new(7));
        command_tx
            .send(NativeTransportCommand::Shutdown.into())
            .await
            .expect("shutdown");
        let (outcome, custody) = join.await.expect("loop join");
        assert!(
            outcome.is_ok(),
            "two contiguous batches must publish without reconnecting"
        );
        assert_eq!(custody.received_cursor(), Some(ConversationCursor::new(7)));
        assert_eq!(
            custody.last_accepted_cursor(),
            None,
            "forwarding must not mark application acceptance"
        );
    });
}

/// Runs the real command loop for a pending test service on its own thread.
/// The rendezvous event bridge blocks each handler in `publish` until the
/// test receives the event, so "the handler has not returned" is observable.
fn run_loop(
    commands: tokio::sync::mpsc::Receiver<QueuedCommand>,
    events: SyncSender<NativeTransportEvent>,
) -> std::thread::JoinHandle<Result<(), ServiceFailure>> {
    std::thread::spawn(move || {
        test_runtime().block_on(async move {
            let mut commands = commands;
            let (delivery_tx, mut delivery_rx) = tokio::sync::mpsc::channel(1);
            let mut runtime = ServiceRuntime::new_for_batch_tests();
            let mut frames = FrameFactory::new();
            let outcome = command_loop_with_delivery(
                &mut commands,
                &mut delivery_rx,
                &mut runtime,
                &mut frames,
                &events,
            )
            .await;
            drop(delivery_tx);
            outcome
        })
    })
}

fn unknown_thread_message(request: &str) -> NativeTransportCommand {
    NativeTransportCommand::QueueFirstMessage(Box::new(QueueFirstMessage {
        request_id: RequestId::parse(request).expect("request"),
        thread_id: ThreadId::parse("unknown-hold-thread").expect("thread"),
        body: MessageBody::parse("held message").expect("body"),
    }))
}

fn wait_until(mut condition: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !condition() {
        assert!(Instant::now() < deadline, "condition was not reached");
        std::thread::sleep(Duration::from_millis(1));
    }
}

#[test]
fn held_command_keeps_the_connection_busy_until_its_handler_returns() {
    let (service, commands, _finished) = NativeTransportService::pending_for_test();
    let holds = std::sync::Arc::clone(service.holds());
    let (event_tx, event_rx) = sync_channel(0);
    let service_loop = run_loop(commands, event_tx);

    service
        .submit(unknown_thread_message("held-first-message"))
        .expect("admitted");
    assert_eq!(holds.status().count, 1, "held from admission");
    // The handler is parked in `publish` until the reply is received.
    std::thread::sleep(Duration::from_millis(20));
    assert_eq!(holds.status().count, 1, "busy while the handler runs");
    let reply = event_rx
        .recv_timeout(Duration::from_secs(5))
        .expect("handler reply");
    assert!(matches!(
        reply,
        NativeTransportEvent::FirstMessageFailed { .. }
    ));
    wait_until(|| holds.status().is_idle());

    holds.seal();
    assert_eq!(
        service.submit(unknown_thread_message("sealed-first-message")),
        Err(CommandSendError::Busy),
        "a sealed connection refuses new mutations"
    );
    assert!(holds.status().is_idle());
    service.request_shutdown().expect("shutdown after drain");
    assert!(
        service_loop.join().expect("loop thread").is_ok(),
        "the drained loop stops cleanly"
    );
    assert_eq!(
        service.submit(NativeTransportCommand::ListRegisteredProfiles),
        Err(CommandSendError::Stopped)
    );
}

#[test]
fn sealing_refuses_only_mutations_and_unsealing_restores_admission() {
    let (service, mut commands, _finished) = NativeTransportService::pending_for_test();
    let holds = service.holds();
    holds.seal();
    assert_eq!(
        service.submit(unknown_thread_message("sealed-message")),
        Err(CommandSendError::Busy)
    );
    service
        .submit(NativeTransportCommand::ListRegisteredProfiles)
        .expect("reads stay admitted while sealed");
    let read = commands.try_recv().expect("queued read");
    assert!(read.hold().is_none());
    holds.unseal();
    service
        .submit(unknown_thread_message("unsealed-message"))
        .expect("admitted after unseal");
    let queued = commands.try_recv().expect("queued mutation");
    assert_eq!(
        queued.hold().map(super::Hold::kind),
        Some(super::HoldKind::Message)
    );
    assert_eq!(holds.status().count, 1);
    drop(queued);
    assert!(
        holds.status().is_idle(),
        "a dropped command releases its hold"
    );
}

#[test]
fn refused_admission_releases_the_hold() {
    let (service, commands, _finished) = NativeTransportService::pending_for_test();
    drop(commands);
    assert_eq!(
        service.submit(unknown_thread_message("closed-message")),
        Err(CommandSendError::Stopped)
    );
    assert!(service.holds().status().is_idle());
}
