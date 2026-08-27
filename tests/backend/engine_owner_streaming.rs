use super::framing::{SseEof, SseFramer, SseFramerError};
use super::observation::{
    DeliveryError, EngineObservation, TerminalObservation, TerminalState, TextDelta, chunk_text,
    deliver_observation,
};
use artisan_transport::CancelHandle;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::time::Instant;

fn run_id(value: &str) -> artisan_domain::RunId {
    artisan_domain::RunId::parse(value).expect("valid run id")
}

// ---------------------------------------------------------------------------
// Framing: fragmentation including CR and multi-byte UTF-8
// ---------------------------------------------------------------------------

#[test]
fn framer_fragments_data_line_across_chunks() {
    let mut framer = SseFramer::new(1024, 4096).unwrap();
    let a = framer.feed(b"data: hel").unwrap();
    assert!(a.is_empty());
    let b = framer.feed(b"lo\n\n").unwrap();
    assert_eq!(b.len(), 1);
    assert_eq!(b[0].data(), "hello");
    assert_eq!(b[0].event(), "message");
}

#[test]
fn framer_cr_split_across_chunks() {
    let mut framer = SseFramer::new(1024, 4096).unwrap();
    let a = framer.feed(b"data: hello\r").unwrap();
    assert!(a.is_empty());
    let b = framer.feed(b"\n\n").unwrap();
    assert_eq!(b.len(), 1);
    assert_eq!(b[0].data(), "hello");
}

#[test]
fn framer_cr_inside_single_chunk_trimmed() {
    let mut framer = SseFramer::new(1024, 4096).unwrap();
    let events = framer.feed(b"data: hi\r\n\r\n").unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].data(), "hi");
}

#[test]
fn framer_multibyte_utf8_split_across_chunks() {
    // "café" where é is 2 bytes: 0xC3 0xA9
    let mut framer = SseFramer::new(1024, 4096).unwrap();
    let line = "data: café\n\n";
    let bytes = line.as_bytes();
    // Split inside é: "data: caf" + [0xC3] then [0xA9 + "\n\n"]
    let split = "data: caf".len() + 1; // after first byte of é
    let first = &bytes[..split];
    let second = &bytes[split..];
    assert_eq!(first, &line.as_bytes()[..split]);
    let a = framer.feed(first).unwrap();
    assert!(a.is_empty(), "incomplete UTF-8 line must not dispatch");
    let b = framer.feed(second).unwrap();
    assert_eq!(b.len(), 1);
    assert_eq!(b[0].data(), "café");
}

#[test]
fn framer_three_byte_utf8_split_across_chunks() {
    // "€" is 3 bytes: E2 82 AC
    let mut framer = SseFramer::new(1024, 4096).unwrap();
    let payload = "€";
    let mut line_bytes = Vec::new();
    line_bytes.extend_from_slice(b"data: ");
    line_bytes.extend_from_slice(payload.as_bytes());
    line_bytes.extend_from_slice(b"\n\n");
    // Split in middle of €
    let first = &line_bytes[..7]; // "data: " (6) + 0xE2
    let second = &line_bytes[7..];
    let a = framer.feed(first).unwrap();
    assert!(a.is_empty());
    let b = framer.feed(second).unwrap();
    assert_eq!(b.len(), 1);
    assert_eq!(b[0].data(), "€");
}

// ---------------------------------------------------------------------------
// Framing: multiple data lines, event names, IDs, comments, unknown fields
// ---------------------------------------------------------------------------

#[test]
fn framer_multiple_data_lines_joined_with_newline() {
    let mut framer = SseFramer::new(1024, 4096).unwrap();
    let events = framer
        .feed(b"data: first\ndata: second\ndata: third\n\n")
        .unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].data(), "first\nsecond\nthird");
}

#[test]
fn framer_explicit_and_default_event_names() {
    let mut framer = SseFramer::new(1024, 4096).unwrap();
    let events = framer
        .feed(b"event: custom\ndata: hello\n\n data: ignored blank? \n")
        .unwrap();
    // Actually first event has custom, second not yet dispatched (no blank). Let's do two events.
    let mut framer2 = SseFramer::new(1024, 4096).unwrap();
    let e = framer2
        .feed(b"event: custom\ndata: hello\n\ndata: world\n\n")
        .unwrap();
    assert_eq!(e.len(), 2);
    assert_eq!(e[0].event(), "custom");
    assert_eq!(e[0].data(), "hello");
    assert_eq!(e[1].event(), "message");
    assert_eq!(e[1].data(), "world");
    // first framer had only one dispatched
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].event(), "custom");

    let mut empty_event = SseFramer::new(1024, 4096).unwrap();
    let empty = empty_event.feed(b"event:\ndata: fallback\n\n").unwrap();
    assert_eq!(empty[0].event(), "message");
}

#[test]
fn framer_ids_preserved_and_reset_between_events() {
    let mut framer = SseFramer::new(1024, 4096).unwrap();
    let events = framer
        .feed(b"id: 42\ndata: first\n\nid: 99\ndata: second\n\ndata: third\n\n")
        .unwrap();
    assert_eq!(events.len(), 3);
    assert_eq!(events[0].id(), Some("42"));
    assert_eq!(events[1].id(), Some("99"));
    // Third event has no id; should be None because per-event reset.
    assert_eq!(events[2].id(), None);
}

#[test]
fn framer_comments_and_unknown_fields_ignored() {
    let mut framer = SseFramer::new(1024, 4096).unwrap();
    let events = framer
        .feed(b": this is comment\nretry: 1000\nunknown: foo\ndata: hello\n\n")
        .unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].data(), "hello");
    assert_eq!(events[0].event(), "message");
}

#[test]
fn framer_colon_space_trimming() {
    let mut framer = SseFramer::new(1024, 4096).unwrap();
    let events = framer
        .feed(b"data:  hello with one space trimmed\n\n")
        .unwrap();
    assert_eq!(events[0].data(), " hello with one space trimmed");
    let mut framer2 = SseFramer::new(1024, 4096).unwrap();
    let events2 = framer2.feed(b"data:hello-no-space\n\n").unwrap();
    assert_eq!(events2[0].data(), "hello-no-space");
    let mut framer3 = SseFramer::new(1024, 4096).unwrap();
    let events3 = framer3.feed(b"data:\n\n").unwrap();
    assert_eq!(events3[0].data(), "");
}

#[test]
fn framer_field_without_colon_treated_as_empty_value() {
    let mut framer = SseFramer::new(1024, 4096).unwrap();
    let events = framer.feed(b"data\n\n").unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].data(), "");
}

#[test]
fn framer_blank_line_without_data_does_not_dispatch() {
    let mut framer = SseFramer::new(1024, 4096).unwrap();
    let events = framer.feed(b"event: foo\n\n").unwrap();
    assert!(events.is_empty(), "event without data must not dispatch");
    let events2 = framer.feed(b"data: hi\n\n").unwrap();
    assert_eq!(events2.len(), 1);
    assert_eq!(events2[0].event(), "message");
    assert_eq!(events2[0].data(), "hi");
}

// ---------------------------------------------------------------------------
// Framing: bounds, UTF-8, arithmetic, EOF
// ---------------------------------------------------------------------------

#[test]
fn framer_exact_line_cap_accepted() {
    // "data: 1234" is exactly 10 bytes before LF.
    let mut framer = SseFramer::new(10, 100).unwrap();
    let events = framer.feed(b"data: 1234\n\n").unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].data(), "1234");
}

#[test]
fn framer_one_byte_over_line_rejected() {
    let mut framer = SseFramer::new(10, 100).unwrap();
    // "data: 12345" is 11 bytes before LF -> one over
    let err = framer.feed(b"data: 12345\n").unwrap_err();
    assert_eq!(err, SseFramerError::LineTooLong);
    // Poisoned thereafter
    assert_eq!(
        framer.feed(b"data: hi\n\n").unwrap_err(),
        SseFramerError::Poisoned
    );
}

#[test]
fn framer_exact_event_cap_accepted() {
    // max_event=5, data "hello" is 5 -> should be ok
    let mut framer = SseFramer::new(100, 5).unwrap();
    let events = framer.feed(b"data: hello\n\n").unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].data(), "hello");
}

#[test]
fn framer_event_overflow_rejected() {
    let mut framer = SseFramer::new(100, 5).unwrap();
    // first data 3 bytes "abc", second data 3 bytes "def" joined => "abc\ndef" =7 >5
    let first = framer.feed(b"data: abc\n").unwrap();
    assert!(first.is_empty());
    let err = framer.feed(b"data: def\n\n").unwrap_err();
    assert_eq!(err, SseFramerError::EventTooLarge);
    assert_eq!(
        framer.feed(b"data: hi\n\n").unwrap_err(),
        SseFramerError::Poisoned
    );
}

#[test]
fn framer_invalid_utf8_rejected_payload_safe() {
    let secret = "hunter2-super-secret-xyz-12345";
    let mut framer = SseFramer::new(1024, 4096).unwrap();
    let mut line = Vec::new();
    line.extend_from_slice(b"data: ");
    line.extend_from_slice(secret.as_bytes());
    line.push(0xFF);
    line.push(b'\n');
    let err = framer.feed(&line).unwrap_err();
    assert_eq!(err, SseFramerError::InvalidUtf8);
    let display = format!("{err}");
    let debug = format!("{err:?}");
    assert!(!display.contains(secret));
    assert!(!debug.contains(secret));
    assert!(!display.contains("hunter2"));
    assert!(!debug.contains("hunter2"));
}

#[test]
fn framer_arithmetic_extreme_bounds() {
    let err = SseFramer::new(usize::MAX, 1024).unwrap_err();
    assert_eq!(err, SseFramerError::UnrepresentableCap);
    let err2 = SseFramer::new(1024, usize::MAX).unwrap_err();
    assert_eq!(err2, SseFramerError::UnrepresentableCap);
    let err3 = SseFramer::new(0, 1024).unwrap_err();
    assert_eq!(err3, SseFramerError::UnrepresentableCap);
}

#[test]
fn framer_eof_explicit_and_not_terminal() {
    let mut framer = SseFramer::new(1024, 4096).unwrap();
    let events = framer.feed(b"data: hello\n").unwrap();
    assert!(events.is_empty(), "no blank line -> no dispatch");
    let eof = framer.finish().unwrap();
    assert_eq!(eof, SseEof::Clean);
    // EOF does not produce an event; it is not a terminal observation
    // Subsequent feed is poisoned
    assert_eq!(
        framer.feed(b"data: hi\n\n").unwrap_err(),
        SseFramerError::Poisoned
    );
    // finish again is poisoned
    assert_eq!(framer.finish().unwrap_err(), SseFramerError::Poisoned);
}

#[test]
fn framer_eof_with_pending_data_does_not_dispatch() {
    let mut framer = SseFramer::new(1024, 4096).unwrap();
    framer.feed(b"data: pending").unwrap();
    // No LF yet, pending holds incomplete line
    let eof = framer.finish().unwrap();
    assert_eq!(eof, SseEof::Clean);
}

#[test]
fn framer_eof_clean_after_complete_events() {
    let mut framer = SseFramer::new(1024, 4096).unwrap();
    framer.feed(b"data: hi\n\n").unwrap();
    let eof = framer.finish().unwrap();
    assert_eq!(eof, SseEof::Clean);
}

#[test]
fn framer_error_debug_payload_safe() {
    let secret = "Bearer supercalifragilistic-secret-token-999";
    let mut framer = SseFramer::new(5, 100).unwrap();
    // Build a line longer than 5 containing secret
    let mut line = Vec::new();
    line.extend_from_slice(format!("data: {secret}").as_bytes());
    line.push(b'\n');
    let err = framer.feed(&line).unwrap_err();
    assert_eq!(err, SseFramerError::LineTooLong);
    let dbg = format!("{err:?}");
    let disp = format!("{err}");
    assert!(!dbg.contains(secret));
    assert!(!disp.contains(secret));
    assert!(!dbg.contains("supercalifragilistic"));
}

#[test]
fn framer_deterministic_poisoned_after_event_overflow() {
    let mut framer = SseFramer::new(100, 3).unwrap();
    framer.feed(b"data: ab\n").unwrap();
    let err = framer.feed(b"data: cd\n\n").unwrap_err();
    assert_eq!(err, SseFramerError::EventTooLarge);
    for _ in 0..3 {
        assert_eq!(
            framer.feed(b"data: x\n\n").unwrap_err(),
            SseFramerError::Poisoned
        );
    }
}

// ---------------------------------------------------------------------------
// Observation: chunking, IDs, sequences, terminals
// ---------------------------------------------------------------------------

#[test]
fn chunking_ascii_4096_one_chunk() {
    let text = "a".repeat(4096);
    let deltas = chunk_text(&run_id("run-aaaaaaaaaa"), 1, "native1", &text);
    assert_eq!(deltas.len(), 1);
    assert_eq!(deltas[0].delta().len(), 4096);
    assert_eq!(deltas[0].delta(), text);
    assert!(deltas[0].delta().len() <= 4096);
    assert_eq!(deltas[0].chunk_id(), "native1:1:0");
    assert_eq!(deltas[0].sequence(), 1);
}

#[test]
fn chunking_ascii_4097_two_chunks() {
    let text = "a".repeat(4097);
    let deltas = chunk_text(&run_id("run-bbbbbbbbbb"), 2, "native2", &text);
    assert_eq!(deltas.len(), 2);
    assert_eq!(deltas[0].delta().len(), 4096);
    assert_eq!(deltas[1].delta().len(), 1);
    assert_eq!(deltas[0].chunk_id(), "native2:2:0");
    assert_eq!(deltas[1].chunk_id(), "native2:2:1");
    let concat: String = deltas.iter().map(TextDelta::delta).collect();
    assert_eq!(concat, text);
}

#[test]
fn chunking_beyond_8192_three_chunks() {
    let text = "b".repeat(8193);
    let deltas = chunk_text(&run_id("run-cccccccccc"), 5, "nat", &text);
    assert_eq!(deltas.len(), 3);
    assert_eq!(deltas[0].delta().len(), 4096);
    assert_eq!(deltas[1].delta().len(), 4096);
    assert_eq!(deltas[2].delta().len(), 1);
    for d in &deltas {
        assert!(d.delta().len() <= 4096);
        assert_eq!(d.sequence(), 5);
    }
    let concat: String = deltas.iter().map(TextDelta::delta).collect();
    assert_eq!(concat, text);
    assert_eq!(deltas[0].chunk_id(), "nat:5:0");
    assert_eq!(deltas[1].chunk_id(), "nat:5:1");
    assert_eq!(deltas[2].chunk_id(), "nat:5:2");
}

#[test]
fn chunking_unicode_split_near_boundary() {
    // Build 4095 'a' + '€' (3 bytes) + 'b' * 10
    let mut text = "a".repeat(4095);
    text.push('€');
    text.push_str(&"b".repeat(10));
    assert!(text.len() > 4096);
    let deltas = chunk_text(&run_id("run-unicode111"), 7, "uid", &text);
    for d in &deltas {
        assert!(
            d.delta().len() <= 4096,
            "chunk too large: {}",
            d.delta().len()
        );
        // each chunk must be valid UTF-8 (String guarantees)
        assert!(std::str::from_utf8(d.delta().as_bytes()).is_ok());
    }
    let concat: String = deltas.iter().map(TextDelta::delta).collect();
    assert_eq!(concat, text, "concatenation must reproduce input exactly");
    // First chunk should be 4095 bytes (only 'a's), second contains €+b's
    assert_eq!(deltas[0].delta().len(), 4095);
    assert_eq!(deltas[1].delta(), format!("€{}", "b".repeat(10)));
}

#[test]
fn chunking_unicode_exact_boundary_no_split_inside_codepoint() {
    // Fill exactly 4096 bytes with multi-byte chars where last char would straddle boundary
    // Use 1365 * '€' (3 bytes) = 4095, plus 'a' (1) = 4096
    let mut text = "€".repeat(1365);
    text.push('a');
    assert_eq!(text.len(), 4096);
    let deltas = chunk_text(&run_id("run-unicode222"), 10, "u2", &text);
    assert_eq!(deltas.len(), 1);
    assert_eq!(deltas[0].delta().len(), 4096);
    // Add one more char -> split
    text.push('€');
    let deltas2 = chunk_text(&run_id("run-unicode222"), 10, "u2", &text);
    assert_eq!(deltas2.len(), 2);
    assert_eq!(deltas2[0].delta().len(), 4096);
    assert_eq!(deltas2[1].delta(), "€");
    let concat: String = deltas2.iter().map(TextDelta::delta).collect();
    assert_eq!(concat, text);
}

#[test]
fn chunking_empty_produces_zero_deltas() {
    let deltas = chunk_text(&run_id("run-empty0000"), 3, "nat", "");
    assert!(deltas.is_empty());
}

#[test]
fn chunking_exact_concatenation_with_interior_newlines_and_empty_parts() {
    let text = "hello\n\nworld\n";
    let deltas = chunk_text(&run_id("run-concat1111"), 42, "nid", text);
    let concat: String = deltas.iter().map(TextDelta::delta).collect();
    assert_eq!(concat, text);
    // Interior content preserved: double newline must be in concatenation
    assert!(concat.contains("\n\n"));
}

#[test]
fn chunking_stable_ids_and_preserved_sequence() {
    let text = "x".repeat(5000);
    let seq = 12345;
    let deltas = chunk_text(&run_id("run-stable1234"), seq, "myNative", &text);
    assert_eq!(deltas.len(), 2);
    assert_eq!(deltas[0].sequence(), seq);
    assert_eq!(deltas[1].sequence(), seq);
    assert_eq!(deltas[0].chunk_id(), "myNative:12345:0");
    assert_eq!(deltas[1].chunk_id(), "myNative:12345:1");
    assert_eq!(deltas[0].run_id().as_str(), "run-stable1234");
}

#[test]
fn chunking_no_ellipsis_or_truncation() {
    let text = "a".repeat(10000);
    let deltas = chunk_text(&run_id("run-no-trunc00"), 1, "n", &text);
    let concat: String = deltas.iter().map(TextDelta::delta).collect();
    assert_eq!(concat.len(), 10000);
    assert_eq!(concat, text);
    for d in &deltas {
        assert!(!d.delta().contains('…'));
        assert!(!d.delta().contains("[truncated]"));
    }
}

#[test]
fn terminal_states_distinguishable() {
    let states = [
        TerminalState::Completed,
        TerminalState::Failed,
        TerminalState::Cancelled,
        TerminalState::Interrupted,
    ];
    for (i, a) in states.iter().enumerate() {
        for (j, b) in states.iter().enumerate() {
            if i == j {
                assert_eq!(a, b);
            } else {
                assert_ne!(a, b, "{a:?} should not equal {b:?}");
            }
        }
    }
    // Ensure Interrupted != Cancelled explicitly
    assert_ne!(TerminalState::Interrupted, TerminalState::Cancelled);
}

#[test]
fn chunking_preserves_multibyte_exactly() {
    let text = "héllo 🌍 world — test 𝄞 end";
    let deltas = chunk_text(&run_id("run-multi-byte"), 99, "mid", text);
    let concat: String = deltas.iter().map(TextDelta::delta).collect();
    assert_eq!(concat, text);
    for d in &deltas {
        assert!(d.delta().len() <= 4096);
        assert!(std::str::from_utf8(d.delta().as_bytes()).is_ok());
    }
}

// ---------------------------------------------------------------------------
// Sink: wakeable bounded delivery
// ---------------------------------------------------------------------------

fn make_delta(text: &str) -> EngineObservation {
    let deltas = chunk_text(&run_id("run-sink-test1"), 42, "native", text);
    assert_eq!(deltas.len(), 1);
    EngineObservation::TextDelta(deltas.into_iter().next().unwrap())
}

fn make_terminal(
    state: TerminalState,
    seq: u64,
    reason: Option<&str>,
    error_ref: Option<&str>,
) -> EngineObservation {
    EngineObservation::Terminal(TerminalObservation::new(
        run_id("run-sink-term1"),
        seq,
        state,
        reason.map(std::borrow::ToOwned::to_owned),
        error_ref.map(std::borrow::ToOwned::to_owned),
    ))
}

#[tokio::test(flavor = "current_thread")]
async fn sink_immediate_capacity_delivers_exactly_once() {
    let (tx, mut rx) = mpsc::channel::<EngineObservation>(2);
    let shutdown = CancelHandle::new();
    let cancel = CancelHandle::new();
    let deadline = Instant::now() + Duration::from_secs(5);
    let obs = make_delta("hello sink");
    let obs_clone = obs.clone();

    let res = deliver_observation(obs, tx.clone(), &shutdown, &cancel, deadline).await;
    assert_eq!(res, Ok(()));

    let got = rx.recv().await.expect("one item");
    assert_eq!(got, obs_clone);
    assert!(rx.try_recv().is_err(), "exactly once");
}

#[tokio::test(flavor = "current_thread")]
async fn sink_full_capacity_stays_pending_then_wakes_on_receive() {
    let (tx, mut rx) = mpsc::channel::<EngineObservation>(1);
    let shutdown = CancelHandle::new();
    let cancel = CancelHandle::new();

    // Fill capacity.
    let first = make_delta("first");
    tx.send(first).await.unwrap();
    assert_eq!(tx.capacity(), 0);

    let pending = make_delta("pending-secret-xyz");
    let pending_clone = pending.clone();
    let tx2 = tx.clone();
    let deadline = Instant::now() + Duration::from_secs(5);

    let handle = tokio::spawn(async move {
        deliver_observation(pending, tx2, &shutdown, &cancel, deadline).await
    });

    // Let the reserve register.
    tokio::task::yield_now().await;
    assert!(!handle.is_finished(), "should stay pending while full");

    // Free one slot.
    let got_first = rx.recv().await.unwrap();
    assert_eq!(got_first, make_delta("first"));

    let res = tokio::time::timeout(Duration::from_secs(1), handle)
        .await
        .expect("wakes")
        .unwrap();
    assert_eq!(res, Ok(()));

    let got_pending = rx.recv().await.unwrap();
    assert_eq!(got_pending, pending_clone);
    assert!(rx.try_recv().is_err());
}

#[tokio::test(flavor = "current_thread")]
async fn sink_receiver_close_while_pending_returns_sink_closed() {
    let (tx, rx) = mpsc::channel::<EngineObservation>(1);
    let shutdown = CancelHandle::new();
    let cancel = CancelHandle::new();

    // Fill.
    tx.send(make_delta("fill")).await.unwrap();

    let pending = make_delta("closed-pending");
    let deadline = Instant::now() + Duration::from_secs(5);
    let tx2 = tx.clone();
    let handle = tokio::spawn(async move {
        deliver_observation(pending, tx2, &shutdown, &cancel, deadline).await
    });
    tokio::task::yield_now().await;
    assert!(!handle.is_finished());

    drop(rx);
    // Dropping the receiver still leaves sender capacity but reserve will see closed when all receivers gone?
    // With 1 capacity channel, dropping receiver closes channel; reserve should return SinkClosed.
    // Need to also drop the original tx's receiver side is gone; but tx still exists.
    // Close the channel explicitly by dropping remaining receiver already done; give time.
    let res = tokio::time::timeout(Duration::from_secs(1), handle)
        .await
        .expect("wakes on close")
        .unwrap();
    assert_eq!(res, Err(DeliveryError::SinkClosed));
}

#[tokio::test(flavor = "current_thread")]
async fn sink_pre_cancel_returns_cancelled_without_delivery() {
    let (tx, mut rx) = mpsc::channel::<EngineObservation>(1);
    let shutdown = CancelHandle::new();
    let cancel = CancelHandle::new();
    cancel.cancel();
    let deadline = Instant::now() + Duration::from_secs(5);
    let obs = make_delta("pre-cancel-secret-hunter2");
    let res = deliver_observation(obs, tx.clone(), &shutdown, &cancel, deadline).await;
    assert_eq!(res, Err(DeliveryError::Cancelled));
    assert!(rx.try_recv().is_err());
}

#[tokio::test(flavor = "current_thread")]
async fn sink_pre_shutdown_returns_shutdown_without_delivery() {
    let (tx, mut rx) = mpsc::channel::<EngineObservation>(1);
    let shutdown = CancelHandle::new();
    let cancel = CancelHandle::new();
    shutdown.cancel();
    let deadline = Instant::now() + Duration::from_secs(5);
    let obs = make_delta("pre-shutdown");
    let res = deliver_observation(obs, tx.clone(), &shutdown, &cancel, deadline).await;
    assert_eq!(res, Err(DeliveryError::Shutdown));
    assert!(rx.try_recv().is_err());
}

#[tokio::test(flavor = "current_thread")]
async fn sink_expired_deadline_returns_deadline_without_delivery() {
    let (tx, mut rx) = mpsc::channel::<EngineObservation>(1);
    let shutdown = CancelHandle::new();
    let cancel = CancelHandle::new();
    let deadline = Instant::now() - Duration::from_millis(10);
    let obs = make_delta("expired");
    let res = deliver_observation(obs, tx.clone(), &shutdown, &cancel, deadline).await;
    assert_eq!(res, Err(DeliveryError::Deadline));
    assert!(rx.try_recv().is_err());
}

#[tokio::test(flavor = "current_thread")]
async fn sink_shutdown_wins_over_cancel_when_both_pre_signalled() {
    let (tx, mut rx) = mpsc::channel::<EngineObservation>(1);
    let shutdown = CancelHandle::new();
    let cancel = CancelHandle::new();
    shutdown.cancel();
    cancel.cancel();
    let deadline = Instant::now() + Duration::from_secs(5);
    let obs = make_delta("both");
    let res = deliver_observation(obs, tx.clone(), &shutdown, &cancel, deadline).await;
    assert_eq!(res, Err(DeliveryError::Shutdown));
    assert!(rx.try_recv().is_err());
}

#[tokio::test(flavor = "current_thread")]
async fn sink_cancel_wins_over_deadline_when_both_ready() {
    let (tx, mut rx) = mpsc::channel::<EngineObservation>(1);
    let shutdown = CancelHandle::new();
    let cancel = CancelHandle::new();
    cancel.cancel();
    let deadline = Instant::now() - Duration::from_millis(10);
    let obs = make_delta("cancel-vs-deadline");
    let res = deliver_observation(obs, tx.clone(), &shutdown, &cancel, deadline).await;
    // Preflight: shutdown not set, cancel set => Cancelled wins even though deadline expired.
    assert_eq!(res, Err(DeliveryError::Cancelled));
    assert!(rx.try_recv().is_err());
}

#[tokio::test(flavor = "current_thread")]
async fn sink_later_shutdown_wins_while_full() {
    let (tx, mut rx) = mpsc::channel::<EngineObservation>(1);
    tx.send(make_delta("fill")).await.unwrap();
    let shutdown = std::sync::Arc::new(CancelHandle::new());
    let cancel = std::sync::Arc::new(CancelHandle::new());
    let deadline = Instant::now() + Duration::from_secs(5);
    let pending = make_delta("late-shutdown");
    let txc = tx.clone();
    let s = std::sync::Arc::clone(&shutdown);
    let c = std::sync::Arc::clone(&cancel);
    let h = tokio::spawn(async move { deliver_observation(pending, txc, &s, &c, deadline).await });
    tokio::task::yield_now().await;
    assert!(!h.is_finished());
    shutdown.cancel();
    let res = tokio::time::timeout(Duration::from_secs(1), h)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(res, Err(DeliveryError::Shutdown));
    // No pending delivered.
    let first = rx.recv().await.unwrap();
    assert_eq!(first, make_delta("fill"));
    assert!(rx.try_recv().is_err());
}

#[tokio::test(flavor = "current_thread")]
async fn sink_later_cancel_wins_while_full() {
    let (tx, mut rx) = mpsc::channel::<EngineObservation>(1);
    tx.send(make_delta("fill")).await.unwrap();
    let shutdown = std::sync::Arc::new(CancelHandle::new());
    let cancel = std::sync::Arc::new(CancelHandle::new());
    let deadline = Instant::now() + Duration::from_secs(5);
    let pending = make_delta("late-cancel");
    let txc = tx.clone();
    let s = std::sync::Arc::clone(&shutdown);
    let c = std::sync::Arc::clone(&cancel);
    let h = tokio::spawn(async move { deliver_observation(pending, txc, &s, &c, deadline).await });
    tokio::task::yield_now().await;
    assert!(!h.is_finished());
    cancel.cancel();
    let res = tokio::time::timeout(Duration::from_secs(1), h)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(res, Err(DeliveryError::Cancelled));
    let first = rx.recv().await.unwrap();
    assert_eq!(first, make_delta("fill"));
    assert!(rx.try_recv().is_err());
}

#[tokio::test(flavor = "current_thread")]
async fn sink_later_deadline_wins_while_full() {
    let (tx, mut rx) = mpsc::channel::<EngineObservation>(1);
    tx.send(make_delta("fill")).await.unwrap();
    let shutdown = CancelHandle::new();
    let cancel = CancelHandle::new();
    let deadline = Instant::now() + Duration::from_millis(30);
    let pending = make_delta("late-deadline");
    let txc = tx.clone();
    let h = tokio::spawn(async move {
        deliver_observation(pending, txc, &shutdown, &cancel, deadline).await
    });
    // Do not free capacity; deadline should fire.
    let res = tokio::time::timeout(Duration::from_secs(1), h)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(res, Err(DeliveryError::Deadline));
    let first = rx.recv().await.unwrap();
    assert_eq!(first, make_delta("fill"));
    assert!(rx.try_recv().is_err());
}

#[tokio::test(flavor = "current_thread")]
async fn sink_frees_capacity_before_deadline_sends_successfully() {
    let (tx, mut rx) = mpsc::channel::<EngineObservation>(1);
    tx.send(make_delta("fill")).await.unwrap();
    let shutdown = CancelHandle::new();
    let cancel = CancelHandle::new();
    let deadline = Instant::now() + Duration::from_millis(200);
    let pending = make_delta("before-deadline");
    let pending_clone = pending.clone();
    let txc = tx.clone();
    let h = tokio::spawn(async move {
        deliver_observation(pending, txc, &shutdown, &cancel, deadline).await
    });
    tokio::task::yield_now().await;
    // Free after 20ms, before deadline.
    tokio::time::sleep(Duration::from_millis(20)).await;
    let first = rx.recv().await.unwrap();
    assert_eq!(first, make_delta("fill"));
    let res = tokio::time::timeout(Duration::from_secs(1), h)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(res, Ok(()));
    let got = rx.recv().await.unwrap();
    assert_eq!(got, pending_clone);
}

#[test]
fn terminal_observation_preserves_all_four_states_and_fields() {
    let seq = 987;
    let run = run_id("run-term-preserve");
    for state in [
        TerminalState::Completed,
        TerminalState::Failed,
        TerminalState::Cancelled,
        TerminalState::Interrupted,
    ] {
        let obs = TerminalObservation::new(
            run.clone(),
            seq,
            state,
            Some("reason-text".to_owned()),
            Some("error-ref-123".to_owned()),
        );
        assert_eq!(obs.run_id().as_str(), run.as_str());
        assert_eq!(obs.sequence(), seq);
        assert_eq!(obs.state(), state);
        assert_eq!(obs.reason(), Some("reason-text"));
        assert_eq!(obs.error_ref(), Some("error-ref-123"));
        // EngineObservation wrapper preserves.
        let wrapped = EngineObservation::Terminal(obs.clone());
        match wrapped {
            EngineObservation::Terminal(t) => {
                assert_eq!(t.state(), state);
                assert_eq!(t.sequence(), seq);
                assert_eq!(t.run_id().as_str(), run.as_str());
                assert_eq!(t.reason(), Some("reason-text"));
                assert_eq!(t.error_ref(), Some("error-ref-123"));
            }
            EngineObservation::TextDelta(_) => panic!("expected terminal"),
        }
    }
    // Reason/error None preserved.
    let no_ref = TerminalObservation::new(run.clone(), 1, TerminalState::Completed, None, None);
    assert_eq!(no_ref.reason(), None);
    assert_eq!(no_ref.error_ref(), None);
    // Distinction: Interrupted != Cancelled
    assert_ne!(TerminalState::Interrupted, TerminalState::Cancelled);
}

#[test]
fn terminal_states_all_distinct_in_observation() {
    let run = run_id("run-distinct");
    let a = TerminalObservation::new(run.clone(), 1, TerminalState::Interrupted, None, None);
    let b = TerminalObservation::new(run.clone(), 1, TerminalState::Cancelled, None, None);
    assert_ne!(
        EngineObservation::Terminal(a),
        EngineObservation::Terminal(b)
    );
}

#[tokio::test(flavor = "current_thread")]
async fn delivery_errors_are_payload_free() {
    let secret = "hunter2-super-secret-xyz-12345-Bearer-token-999";
    let obs = make_delta(secret);
    // Force SinkClosed via closed channel to get error that could leak.
    let (tx, rx) = mpsc::channel::<EngineObservation>(1);
    drop(rx);
    let shutdown = CancelHandle::new();
    let cancel = CancelHandle::new();
    let deadline = Instant::now() + Duration::from_secs(5);
    let res = deliver_observation(obs, tx, &shutdown, &cancel, deadline).await;
    assert_eq!(res, Err(DeliveryError::SinkClosed));
    let err = res.unwrap_err();
    let display = format!("{err}");
    let debug = format!("{err:?}");
    assert!(!display.contains(secret));
    assert!(!debug.contains(secret));
    assert!(!display.contains("hunter2"));
    assert!(!debug.contains("hunter2"));

    // Also check other variants payload-free.
    for e in [
        DeliveryError::Shutdown,
        DeliveryError::Cancelled,
        DeliveryError::Deadline,
        DeliveryError::SinkClosed,
    ] {
        let d = format!("{e}");
        let dbg = format!("{e:?}");
        assert!(!d.contains(secret));
        assert!(!dbg.contains(secret));
        assert!(!d.contains("hunter2"));
        assert!(!dbg.contains("hunter2"));
    }
}

#[tokio::test(flavor = "current_thread")]
async fn deliver_terminal_observation_exact_payload() {
    let (tx, mut rx) = mpsc::channel::<EngineObservation>(2);
    let shutdown = CancelHandle::new();
    let cancel = CancelHandle::new();
    let deadline = Instant::now() + Duration::from_secs(5);
    let term = make_terminal(TerminalState::Failed, 42, Some("oops"), Some("err-ref"));
    let term_clone = term.clone();
    let res = deliver_observation(term, tx.clone(), &shutdown, &cancel, deadline).await;
    assert_eq!(res, Ok(()));
    let got = rx.recv().await.unwrap();
    assert_eq!(got, term_clone);
}
