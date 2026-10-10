//! Service command loop tests: batch forwarding, connection holds carried
//! by admitted commands through their handlers, and the link-preview lane.

#![forbid(unsafe_code)]

use super::{
    CommandSendError, FrameFactory, NativeTransportCommand, NativeTransportEvent,
    NativeTransportService, PREVIEW_CAPACITY, PreviewLane, PrivateDelivery, QueuedCommand,
    ServiceFailure, ServiceRuntime, command_loop_with_delivery,
};
use artisan_domain::{
    ConversationCursor, ConversationLifecycle, ConversationPatch, PatchBatch, PatchId,
    PatchSequence, RequestId, Revision, ThreadId, TurnId, TurnOrdinal, UnixMillis,
};
use std::{
    sync::{
        Arc,
        mpsc::{Receiver, SyncSender, sync_channel},
    },
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
        let (delivery_tx, delivery_rx) = tokio::sync::mpsc::channel::<PrivateDelivery>(8);
        let (event_tx, event_rx) = sync_channel::<NativeTransportEvent>(16);
        service.deliveries.attach(delivery_rx, event_tx.clone());
        let mut frames = FrameFactory::new();
        let join = tokio::spawn(async move {
            let outcome = command_loop_with_delivery(
                &mut command_rx,
                &PreviewLane::default(),
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
    previews: Arc<PreviewLane>,
    events: SyncSender<NativeTransportEvent>,
) -> std::thread::JoinHandle<Result<(), ServiceFailure>> {
    std::thread::spawn(move || {
        test_runtime().block_on(async move {
            let mut commands = commands;
            let (delivery_tx, delivery_rx) = tokio::sync::mpsc::channel(1);
            let mut runtime = ServiceRuntime::new_for_batch_tests();
            runtime.deliveries.attach(delivery_rx, events.clone());
            let mut frames = FrameFactory::new();
            let outcome = command_loop_with_delivery(
                &mut commands,
                &previews,
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
    NativeTransportCommand::SubmitComposerDraft(Box::new(artisan_domain::SubmitComposerDraft {
        request_id: RequestId::parse(request).expect("request"),
        scope: artisan_domain::ComposerDraftScope::Thread(
            ThreadId::parse("unknown-hold-thread").expect("thread"),
        ),
        draft_revision: artisan_domain::ComposerDraftRevision::new(1).expect("revision"),
        selection: None,
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
    let service_loop = run_loop(commands, Arc::clone(&service.previews), event_tx);

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
    assert!(matches!(reply, NativeTransportEvent::MessageFailed { .. }));
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

#[test]
fn a_parked_first_batch_is_forwarded_once_its_subscription_started() {
    let mut service = ServiceRuntime::new_for_batch_tests();
    let thread = ThreadId::parse("parked-thread").expect("thread");
    // A fresh subscribe has no baseline until its response is read.
    service.custody.on_subscribe(thread.clone(), None);
    let (_delivery_tx, delivery_rx) = tokio::sync::mpsc::channel::<PrivateDelivery>(1);
    let (event_tx, event_rx) = sync_channel::<NativeTransportEvent>(4);
    service.deliveries.attach(delivery_rx, event_tx);
    service
        .deliveries
        .parked
        .push_back(PrivateDelivery::Batch(batch(&thread, 5, 6)));
    assert!(
        service
            .deliveries
            .forward_parked_if_ready(&mut service.custody)
    );
    assert!(
        event_rx.try_recv().is_err(),
        "no baseline yet: it stays parked"
    );
    assert!(!service.deliveries.parked.is_empty());

    // The response is read: the subscription starts at the snapshot cursor.
    service
        .custody
        .on_subscribe(thread.clone(), Some(ConversationCursor::new(5)));
    assert!(
        service
            .deliveries
            .forward_parked_if_ready(&mut service.custody)
    );
    assert!(matches!(
        event_rx.try_recv(),
        Ok(NativeTransportEvent::PatchBatch(forwarded)) if forwarded.to_cursor() == ConversationCursor::new(6)
    ));
    assert!(service.deliveries.parked.is_empty());
    assert_eq!(
        service.custody.received_cursor(),
        Some(ConversationCursor::new(6))
    );
}

fn preview(url: &str) -> NativeTransportCommand {
    NativeTransportCommand::ResolveRichLink {
        url: url.to_owned(),
    }
}

/// The URL of the next event, which must be a rich-link failure: the test
/// runtime has no session and cannot reconnect, so every preview fails.
fn next_preview_failure(events: &Receiver<NativeTransportEvent>) -> String {
    match events.recv_timeout(Duration::from_secs(5)) {
        Ok(NativeTransportEvent::RichLinkFailed { requested_url, .. }) => requested_url,
        other => panic!("expected a rich-link failure, got {other:?}"),
    }
}

#[test]
fn previews_wait_in_their_own_lane_not_the_command_queue() {
    let (service, mut commands, _finished) = NativeTransportService::pending_for_test();
    for index in 0..PREVIEW_CAPACITY {
        service
            .submit(preview(&format!("https://example.com/{index}")))
            .expect("the lane admits a full burst");
    }
    assert_eq!(
        service.submit(preview("https://example.com/overflow")),
        Err(CommandSendError::Busy),
        "a full lane pushes back like the command queue"
    );
    service
        .submit(NativeTransportCommand::ListRegisteredProfiles)
        .expect("a full lane leaves the command queue free");
    let queued = commands.try_recv().expect("the command was queued");
    assert!(matches!(
        queued.command(),
        NativeTransportCommand::ListRegisteredProfiles
    ));
    assert!(
        commands.try_recv().is_err(),
        "no preview took a command slot"
    );
    assert_eq!(service.previews.len(), PREVIEW_CAPACITY);
}

#[test]
fn a_command_waits_only_for_the_preview_already_running() {
    let (service, commands, _finished) = NativeTransportService::pending_for_test();
    for index in 1..=5 {
        service
            .submit(preview(&format!("https://example.com/{index}")))
            .expect("admitted");
    }
    // Rendezvous bridge: the running preview is parked in `publish` until
    // its result is received.
    let (event_tx, event_rx) = sync_channel(0);
    let service_loop = run_loop(commands, Arc::clone(&service.previews), event_tx);
    wait_until(|| service.previews.len() == 4);
    service
        .submit(NativeTransportCommand::ListRegisteredProfiles)
        .expect("admitted");

    assert_eq!(next_preview_failure(&event_rx), "https://example.com/1");
    assert!(
        matches!(
            event_rx.recv_timeout(Duration::from_secs(5)),
            Ok(NativeTransportEvent::RegisteredProfilesFailed(_))
        ),
        "the command runs before the four queued previews"
    );
    for index in 2..=5 {
        assert_eq!(
            next_preview_failure(&event_rx),
            format!("https://example.com/{index}"),
            "queued previews then run in admission order"
        );
    }
    service.request_shutdown().expect("shutdown");
    assert!(service_loop.join().expect("loop thread").is_ok());
}

#[test]
fn a_failed_preview_publishes_its_failure_and_the_lane_keeps_serving() {
    let (service, commands, _finished) = NativeTransportService::pending_for_test();
    // Refused before any request, then lost to the absent session.
    service.submit(preview("not a link")).expect("admitted");
    service
        .submit(preview("https://example.com/after"))
        .expect("admitted");
    let (event_tx, event_rx) = sync_channel(0);
    let service_loop = run_loop(commands, Arc::clone(&service.previews), event_tx);

    assert_eq!(next_preview_failure(&event_rx), "not a link");
    assert_eq!(next_preview_failure(&event_rx), "https://example.com/after");
    service
        .submit(preview("https://example.com/later"))
        .expect("the lane still admits");
    assert_eq!(next_preview_failure(&event_rx), "https://example.com/later");
    service
        .submit(NativeTransportCommand::ListRegisteredProfiles)
        .expect("admitted");
    assert!(matches!(
        event_rx.recv_timeout(Duration::from_secs(5)),
        Ok(NativeTransportEvent::RegisteredProfilesFailed(_))
    ));
    service.request_shutdown().expect("shutdown");
    assert!(
        service_loop.join().expect("loop thread").is_ok(),
        "preview failures never end the service"
    );
}

#[test]
fn discarding_drops_only_the_previews_that_have_not_started() {
    let (service, commands, _finished) = NativeTransportService::pending_for_test();
    for index in 1..=5 {
        service
            .submit(preview(&format!("https://example.com/{index}")))
            .expect("admitted");
    }
    let (event_tx, event_rx) = sync_channel(0);
    let service_loop = run_loop(commands, Arc::clone(&service.previews), event_tx);
    wait_until(|| service.previews.len() == 4);

    assert_eq!(service.discard_queued_previews(), 4);
    assert_eq!(
        next_preview_failure(&event_rx),
        "https://example.com/1",
        "the running preview still publishes"
    );
    service
        .submit(NativeTransportCommand::ListRegisteredProfiles)
        .expect("admitted");
    assert!(
        matches!(
            event_rx.recv_timeout(Duration::from_secs(5)),
            Ok(NativeTransportEvent::RegisteredProfilesFailed(_))
        ),
        "nothing discarded runs"
    );
    service.request_shutdown().expect("shutdown");
    assert!(service_loop.join().expect("loop thread").is_ok());
}

#[test]
fn shutdown_does_not_wait_for_queued_previews() {
    let (service, commands, _finished) = NativeTransportService::pending_for_test();
    for index in 0..50 {
        service
            .submit(preview(&format!("https://example.com/{index}")))
            .expect("admitted");
    }
    service.request_shutdown().expect("shutdown");
    let (event_tx, event_rx) = sync_channel(64);
    let service_loop = run_loop(commands, Arc::clone(&service.previews), event_tx);
    assert!(service_loop.join().expect("loop thread").is_ok());
    assert!(
        event_rx.try_recv().is_err(),
        "the loop stopped before any queued preview ran"
    );
    assert_eq!(
        service.submit(preview("https://example.com/late")),
        Err(CommandSendError::Stopped)
    );
}
