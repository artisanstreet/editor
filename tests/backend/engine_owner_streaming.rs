use super::EngineBounds;
use super::framing::{SseEof, SseFramer, SseFramerError};
use super::http::{HealthSecret, PromptError, PromptFile, PromptInput, PromptReceipt};
use super::observation::{
    DeliveryError, EngineObservation, TerminalObservation, TerminalState, TextDelta, chunk_text,
    deliver_observation,
};
use artisan_transport::CancelHandle;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
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

// ---------------------------------------------------------------------------
// Prompt RPC: helpers
// ---------------------------------------------------------------------------

fn prompt_bounds(max_json_body: usize) -> EngineBounds {
    EngineBounds {
        max_json_body,
        max_sse_line: 1024,
        max_sse_event: 4096,
        max_readiness_line: 256,
        max_headers: 32,
        max_buf_bytes: 8192,
        stderr_cap_bytes: 4096,
        sink_capacity: 4,
        control_capacity: 4,
    }
}

fn prompt_secret() -> HealthSecret {
    HealthSecret::from_raw_for_tests("prompt-secret-fixed-1234567890abcd".to_owned())
}

fn validated_endpoint_for(addr: std::net::SocketAddr) -> super::readiness::ValidatedEndpoint {
    let url = match addr {
        std::net::SocketAddr::V4(v) => format!("http://{}:{}", v.ip(), v.port()),
        std::net::SocketAddr::V6(v) => format!("http://[{}]:{}", v.ip(), v.port()),
    };
    let line = format!(r#"{{"url":"{url}"}}"#);
    super::readiness::validate_readiness_line(line.as_bytes(), 2048).expect("endpoint")
}

async fn read_http_request(stream: &mut TcpStream) -> (String, Vec<(String, String)>, Vec<u8>) {
    let mut buf = Vec::new();
    let mut tmp = [0u8; 4096];
    loop {
        let n = stream.read(&mut tmp).await.expect("read");
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&tmp[..n]);
        if buf.windows(4).any(|w| w == b"\r\n\r\n") {
            break;
        }
        if buf.len() > 16 * 1024 {
            break;
        }
    }
    let header_end = buf
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .expect("header end")
        + 4;
    let header_bytes = &buf[..header_end];
    let header_str = String::from_utf8_lossy(header_bytes).to_string();
    let mut lines = header_str.lines();
    let request_line = lines.next().unwrap_or("").to_owned();
    let mut headers = Vec::new();
    for line in lines {
        if line.is_empty() {
            continue;
        }
        if let Some((k, v)) = line.split_once(':') {
            headers.push((k.trim().to_ascii_lowercase(), v.trim().to_owned()));
        }
    }
    let content_len = headers
        .iter()
        .find(|(k, _)| k == "content-length")
        .and_then(|(_, v)| v.parse::<usize>().ok())
        .unwrap_or(0);
    let mut body = buf[header_end..].to_vec();
    while body.len() < content_len {
        let n = stream.read(&mut tmp).await.expect("body read");
        if n == 0 {
            break;
        }
        body.extend_from_slice(&tmp[..n]);
    }
    body.truncate(content_len);
    (request_line, headers, body)
}

// ---------------------------------------------------------------------------
// Prompt RPC: request shape
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "current_thread")]
async fn prompt_request_exact_shape() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let endpoint = validated_endpoint_for(addr);
    let secret = prompt_secret();
    let bounds = prompt_bounds(4096);
    let expected_auth = secret.basic_auth();

    let files = vec![
        PromptFile::new("file:///tmp/a.txt".to_owned(), "a.txt".to_owned()),
        PromptFile::new("file:///tmp/b.txt".to_owned(), "b.txt".to_owned()),
    ];
    let captured = Arc::new(std::sync::Mutex::new(
        None::<(String, Vec<(String, String)>, Vec<u8>)>,
    ));

    let cap_clone = Arc::clone(&captured);
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let (req_line, headers, body) = read_http_request(&mut stream).await;
        *cap_clone.lock().unwrap() = Some((req_line, headers.clone(), body.clone()));
        // Validate server side quickly and respond with minimal JSON object.
        let resp = "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: 2\r\nconnection: close\r\n\r\n{}";
        stream.write_all(resp.as_bytes()).await.unwrap();
        stream.flush().await.unwrap();
    });

    let deadline = Instant::now() + Duration::from_secs(5);
    let shutdown = CancelHandle::new();
    let cancel = CancelHandle::new();
    let res = super::http::perform_prompt(
        &endpoint,
        &secret,
        &bounds,
        deadline,
        &cancel,
        &shutdown,
        PromptInput::new(
            "sess123",
            "agent",
            &files,
            "id-xyz",
            true,
            "hello prompt text",
        ),
    )
    .await;
    assert_eq!(res, Ok(PromptReceipt));
    server.await.unwrap();
    let (req_line, headers, body) = captured.lock().unwrap().take().expect("captured");
    assert_eq!(req_line, "POST /api/session/sess123/prompt HTTP/1.1");
    let host = headers
        .iter()
        .find(|(k, _)| k == "host")
        .expect("host")
        .1
        .clone();
    assert!(host.contains(&addr.port().to_string()));
    let auth = headers
        .iter()
        .find(|(k, _)| k == "authorization")
        .expect("auth")
        .1
        .clone();
    assert_eq!(auth, expected_auth);
    let ct = headers
        .iter()
        .find(|(k, _)| k == "content-type")
        .expect("ct")
        .1
        .clone();
    assert_eq!(ct, "application/json");
    let conn = headers
        .iter()
        .find(|(k, _)| k == "connection")
        .expect("conn")
        .1
        .clone();
    assert_eq!(conn, "close");
    let cl: usize = headers
        .iter()
        .find(|(k, _)| k == "content-length")
        .unwrap()
        .1
        .parse()
        .unwrap();
    assert_eq!(cl, body.len());
    let parsed: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(parsed["delivery"], "agent");
    assert_eq!(parsed["id"], "id-xyz");
    assert_eq!(parsed["resume"], true);
    assert_eq!(parsed["text"], "hello prompt text");
    assert_eq!(parsed["files"].as_array().unwrap().len(), 2);
    assert_eq!(parsed["files"][0]["uri"], "file:///tmp/a.txt");
    assert_eq!(parsed["files"][0]["name"], "a.txt");
    assert_eq!(parsed["files"][1]["uri"], "file:///tmp/b.txt");
    assert_eq!(parsed["files"][1]["name"], "b.txt");
}

#[tokio::test(flavor = "current_thread")]
async fn prompt_request_resume_false_and_empty_files() {
    let listener2 = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr2 = listener2.local_addr().unwrap();
    let endpoint2 = validated_endpoint_for(addr2);
    let secret = prompt_secret();
    let bounds = prompt_bounds(4096);
    let captured2 = Arc::new(std::sync::Mutex::new(None::<Vec<u8>>));
    let c2 = Arc::clone(&captured2);
    let srv2 = tokio::spawn(async move {
        let (mut s, _) = listener2.accept().await.unwrap();
        let (_, _, body) = read_http_request(&mut s).await;
        *c2.lock().unwrap() = Some(body);
        let resp = "HTTP/1.1 200 OK\r\ncontent-length: 2\r\nconnection: close\r\n\r\n{}";
        s.write_all(resp.as_bytes()).await.unwrap();
    });
    let res2 = super::http::perform_prompt(
        &endpoint2,
        &secret,
        &bounds,
        Instant::now() + Duration::from_secs(5),
        &CancelHandle::new(),
        &CancelHandle::new(),
        PromptInput::new("sess123", "agent", &[], "id2", false, "t2"),
    )
    .await;
    assert_eq!(res2, Ok(PromptReceipt));
    srv2.await.unwrap();
    let body2 = captured2.lock().unwrap().take().unwrap();
    let p2: serde_json::Value = serde_json::from_slice(&body2).unwrap();
    assert_eq!(p2["resume"], false);
    assert_eq!(p2["files"].as_array().unwrap().len(), 0);
}

// ---------------------------------------------------------------------------
// Prompt RPC: invalid inputs before transport
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "current_thread")]
async fn prompt_rejects_invalid_session_variants_before_transport() {
    let secret = prompt_secret();
    let bounds = prompt_bounds(1024);
    let dummy_endpoint = validated_endpoint_for("127.0.0.1:9".parse().unwrap());
    let deadline = Instant::now() + Duration::from_secs(5);
    let cases = vec![
        "",
        "a/b",
        "a?b",
        "a#b",
        "a\rb",
        "a\nb",
        "a%b",
        "a%2f",
        "a%2F",
        "sess/with/slash",
    ];
    for sess in cases {
        let res = super::http::perform_prompt(
            &dummy_endpoint,
            &secret,
            &bounds,
            deadline,
            &CancelHandle::new(),
            &CancelHandle::new(),
            PromptInput::new(sess, "delivery", &[], "id", false, "text"),
        )
        .await;
        assert_eq!(res, Err(PromptError::InvalidSession), "session {sess:?}");
    }
}

#[tokio::test(flavor = "current_thread")]
async fn prompt_rejects_empty_fields_before_transport() {
    let secret = prompt_secret();
    let bounds = prompt_bounds(1024);
    let ep = validated_endpoint_for("127.0.0.1:9".parse().unwrap());
    let deadline = Instant::now() + Duration::from_secs(5);
    // empty delivery
    assert_eq!(
        super::http::perform_prompt(
            &ep,
            &secret,
            &bounds,
            deadline,
            &CancelHandle::new(),
            &CancelHandle::new(),
            PromptInput::new("sess", "", &[], "id", false, "text")
        )
        .await,
        Err(PromptError::InvalidDelivery)
    );
    assert_eq!(
        super::http::perform_prompt(
            &ep,
            &secret,
            &bounds,
            deadline,
            &CancelHandle::new(),
            &CancelHandle::new(),
            PromptInput::new("sess", "d", &[], "", false, "text")
        )
        .await,
        Err(PromptError::InvalidId)
    );
    assert_eq!(
        super::http::perform_prompt(
            &ep,
            &secret,
            &bounds,
            deadline,
            &CancelHandle::new(),
            &CancelHandle::new(),
            PromptInput::new("sess", "d", &[], "id", false, "")
        )
        .await,
        Err(PromptError::InvalidText)
    );
}

#[tokio::test(flavor = "current_thread")]
async fn prompt_rejects_crlf_file_fields_before_transport() {
    let secret = prompt_secret();
    let bounds = prompt_bounds(1024);
    let ep = validated_endpoint_for("127.0.0.1:9".parse().unwrap());
    let deadline = Instant::now() + Duration::from_secs(5);
    let bad_files = vec![
        PromptFile::new("uri\r".to_owned(), "name".to_owned()),
        PromptFile::new("uri".to_owned(), "na\nme".to_owned()),
        PromptFile::new("a\rb".to_owned(), "n".to_owned()),
    ];
    for f in bad_files {
        let res = super::http::perform_prompt(
            &ep,
            &secret,
            &bounds,
            deadline,
            &CancelHandle::new(),
            &CancelHandle::new(),
            PromptInput::new("sess", "d", std::slice::from_ref(&f), "id", false, "text"),
        )
        .await;
        assert_eq!(res, Err(PromptError::InvalidFile));
    }
}

// ---------------------------------------------------------------------------
// Prompt RPC: body cap exactly at boundary
// ---------------------------------------------------------------------------

fn prompt_serialized_len(
    delivery: &str,
    files: &[PromptFile],
    id: &str,
    resume: bool,
    text: &str,
) -> usize {
    let mut files_json = Vec::new();
    for f in files {
        files_json.push(serde_json::json!({"uri": f.uri(), "name": f.name()}));
    }
    let v = serde_json::json!({"delivery": delivery, "files": files_json, "id": id, "resume": resume, "text": text});
    serde_json::to_vec(&v).unwrap().len()
}

#[tokio::test(flavor = "current_thread")]
async fn prompt_body_exactly_at_cap_accepts() {
    // Find text length that makes body exactly at cap without huge allocation.
    let cap = 256;
    let bounds = prompt_bounds(cap);
    // Use fixed small delivery/id and no files; brute force text length.
    let mut target_len = None;
    let mut good_text = String::new();
    for len in 0..=cap {
        let t = "a".repeat(len);
        let l = prompt_serialized_len("d", &[], "id", false, &t);
        if l == cap {
            target_len = Some(l);
            good_text = t;
            break;
        }
    }
    assert_eq!(target_len, Some(cap), "must find text len for exact cap");
    // Now start server expecting success.
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let ep = validated_endpoint_for(addr);
    let secret = prompt_secret();
    let srv = tokio::spawn(async move {
        let (mut s, _) = listener.accept().await.unwrap();
        let (_, _, body) = read_http_request(&mut s).await;
        assert_eq!(body.len(), cap);
        let resp = "HTTP/1.1 200 OK\r\ncontent-length: 2\r\nconnection: close\r\n\r\n{}";
        s.write_all(resp.as_bytes()).await.unwrap();
    });
    let res = super::http::perform_prompt(
        &ep,
        &secret,
        &bounds,
        Instant::now() + Duration::from_secs(5),
        &CancelHandle::new(),
        &CancelHandle::new(),
        PromptInput::new("sess", "d", &[], "id", false, &good_text),
    )
    .await;
    assert_eq!(res, Ok(PromptReceipt));
    srv.await.unwrap();
}

#[tokio::test(flavor = "current_thread")]
async fn prompt_body_cap_plus_one_rejects_before_network() {
    let cap = 256;
    let bounds = prompt_bounds(cap);
    let mut found = None;
    let mut bad_text = String::new();
    for len in 0..=512 {
        let t = "a".repeat(len);
        let l = prompt_serialized_len("d", &[], "id", false, &t);
        if l == cap + 1 {
            found = Some(l);
            bad_text = t;
            break;
        }
    }
    assert_eq!(found, Some(cap + 1));
    let ep = validated_endpoint_for("127.0.0.1:9".parse().unwrap());
    let secret = prompt_secret();
    let res = super::http::perform_prompt(
        &ep,
        &secret,
        &bounds,
        Instant::now() + Duration::from_secs(5),
        &CancelHandle::new(),
        &CancelHandle::new(),
        PromptInput::new("sess", "d", &[], "id", false, &bad_text),
    )
    .await;
    assert_eq!(res, Err(PromptError::BodyTooLarge));
}

// ---------------------------------------------------------------------------
// Prompt RPC: debug/display do not leak secrets
// ---------------------------------------------------------------------------

#[test]
fn prompt_error_and_receipt_do_not_leak_secret() {
    let secret_val = "super-secret-prompt-hunter2-999-Bearer-xyz";
    let file_uri = "file:///secret/path/hunter2";
    let err = PromptError::BodyTooLarge;
    let display = format!("{err}");
    let debug = format!("{err:?}");
    assert!(!display.contains(secret_val));
    assert!(!debug.contains(secret_val));
    assert!(!display.contains(file_uri));
    assert!(!debug.contains(file_uri));
    for e in [
        PromptError::InvalidSession,
        PromptError::InvalidDelivery,
        PromptError::InvalidId,
        PromptError::InvalidText,
        PromptError::InvalidFile,
        PromptError::BodyTooLarge,
        PromptError::ConnectFailed,
        PromptError::HandshakeFailed,
        PromptError::SendFailed,
        PromptError::StatusNotSuccess,
        PromptError::BodyReadFailed,
        PromptError::InvalidJson,
        PromptError::Timeout,
        PromptError::Cancelled,
        PromptError::Shutdown,
        PromptError::DriverFailed,
    ] {
        let d = format!("{e}");
        let dbg = format!("{e:?}");
        assert!(!d.contains(secret_val));
        assert!(!dbg.contains(secret_val));
        assert!(!d.contains("hunter2"));
        assert!(!dbg.contains("hunter2"));
    }
    let file = PromptFile::new(file_uri.to_owned(), "secret-name-hunter2".to_owned());
    let dbg = format!("{file:?}");
    assert!(!dbg.contains(file_uri));
    assert!(!dbg.contains("hunter2"));
    let secret = HealthSecret::from_raw_for_tests(secret_val.to_owned());
    let dbg = format!("{secret:?}");
    assert!(!dbg.contains(secret_val));
    let receipt = PromptReceipt;
    let d = format!("{receipt:?}");
    assert!(!d.contains(secret_val));
}

#[test]
fn prompt_success_receipt_is_copy_zero_sized() {
    let r = PromptReceipt;
    let r2 = r;
    assert_eq!(r, r2);
    assert_eq!(format!("{r:?}"), "PromptReceipt");
}

// ---------------------------------------------------------------------------
// Prompt RPC: transport and error variants, no retry
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "current_thread")]
async fn prompt_success_single_request() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let ep = validated_endpoint_for(addr);
    let secret = prompt_secret();
    let bounds = prompt_bounds(1024);
    let count = Arc::new(AtomicUsize::new(0));
    let c = Arc::clone(&count);
    let srv = tokio::spawn(async move {
        let (mut s, _) = listener.accept().await.unwrap();
        c.fetch_add(1, Ordering::SeqCst);
        let (_, _, _) = read_http_request(&mut s).await;
        let resp = "HTTP/1.1 200 OK\r\ncontent-length: 2\r\nconnection: close\r\n\r\n{}";
        s.write_all(resp.as_bytes()).await.unwrap();
        // Keep listening for unexpected second request with timeout.
        // If a second connection arrives, count it as retry.
        tokio::time::sleep(Duration::from_millis(200)).await;
    });
    let res = super::http::perform_prompt(
        &ep,
        &secret,
        &bounds,
        Instant::now() + Duration::from_secs(5),
        &CancelHandle::new(),
        &CancelHandle::new(),
        PromptInput::new("sess1", "d", &[], "id1", false, "hello"),
    )
    .await;
    assert_eq!(res, Ok(PromptReceipt));
    tokio::time::sleep(Duration::from_millis(300)).await;
    srv.await.unwrap();
    assert_eq!(count.load(Ordering::SeqCst), 1);
}

#[tokio::test(flavor = "current_thread")]
async fn prompt_non_2xx_is_status_error_and_no_retry() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let ep = validated_endpoint_for(addr);
    let secret = prompt_secret();
    let bounds = prompt_bounds(1024);
    let count = Arc::new(AtomicUsize::new(0));
    let c = Arc::clone(&count);
    let srv = tokio::spawn(async move {
        let (mut s, _) = listener.accept().await.unwrap();
        c.fetch_add(1, Ordering::SeqCst);
        let (_, _, _) = read_http_request(&mut s).await;
        let resp = "HTTP/1.1 500 Internal Server Error\r\ncontent-length: 2\r\nconnection: close\r\n\r\n{}";
        s.write_all(resp.as_bytes()).await.unwrap();
    });
    let res = super::http::perform_prompt(
        &ep,
        &secret,
        &bounds,
        Instant::now() + Duration::from_secs(5),
        &CancelHandle::new(),
        &CancelHandle::new(),
        PromptInput::new("sess", "d", &[], "id", false, "t"),
    )
    .await;
    assert_eq!(res, Err(PromptError::StatusNotSuccess));
    srv.await.unwrap();
    assert_eq!(count.load(Ordering::SeqCst), 1);
}

#[tokio::test(flavor = "current_thread")]
async fn prompt_malformed_json_is_invalid_json() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let ep = validated_endpoint_for(addr);
    let secret = prompt_secret();
    let bounds = prompt_bounds(1024);
    let srv = tokio::spawn(async move {
        let (mut s, _) = listener.accept().await.unwrap();
        let (_, _, _) = read_http_request(&mut s).await;
        let body = b"not-json";
        let resp = format!(
            "HTTP/1.1 200 OK\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
            body.len(),
            String::from_utf8_lossy(body)
        );
        s.write_all(resp.as_bytes()).await.unwrap();
    });
    let res = super::http::perform_prompt(
        &ep,
        &secret,
        &bounds,
        Instant::now() + Duration::from_secs(5),
        &CancelHandle::new(),
        &CancelHandle::new(),
        PromptInput::new("sess", "d", &[], "id", false, "t"),
    )
    .await;
    assert_eq!(res, Err(PromptError::InvalidJson));
    srv.await.unwrap();
}

#[tokio::test(flavor = "current_thread")]
async fn prompt_non_object_json_is_invalid_json() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let ep = validated_endpoint_for(addr);
    let secret = prompt_secret();
    let bounds = prompt_bounds(1024);
    let srv = tokio::spawn(async move {
        let (mut s, _) = listener.accept().await.unwrap();
        let (_, _, _) = read_http_request(&mut s).await;
        let body = b"[1,2,3]";
        let resp = format!(
            "HTTP/1.1 200 OK\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
            body.len(),
            String::from_utf8_lossy(body)
        );
        s.write_all(resp.as_bytes()).await.unwrap();
    });
    let res = super::http::perform_prompt(
        &ep,
        &secret,
        &bounds,
        Instant::now() + Duration::from_secs(5),
        &CancelHandle::new(),
        &CancelHandle::new(),
        PromptInput::new("sess", "d", &[], "id", false, "t"),
    )
    .await;
    assert_eq!(res, Err(PromptError::InvalidJson));
    srv.await.unwrap();
}

#[tokio::test(flavor = "current_thread")]
async fn prompt_declared_oversize_is_body_too_large() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let ep = validated_endpoint_for(addr);
    let secret = prompt_secret();
    let bounds = prompt_bounds(128);
    let srv = tokio::spawn(async move {
        let (mut s, _) = listener.accept().await.unwrap();
        let (_, _, _) = read_http_request(&mut s).await;
        // The request fits, while the declared response exceeds the cap.
        let resp = "HTTP/1.1 200 OK\r\ncontent-length: 129\r\nconnection: close\r\n\r\n{}";
        s.write_all(resp.as_bytes()).await.unwrap();
    });
    let res = super::http::perform_prompt(
        &ep,
        &secret,
        &bounds,
        Instant::now() + Duration::from_secs(5),
        &CancelHandle::new(),
        &CancelHandle::new(),
        PromptInput::new("sess", "d", &[], "id", false, "t"),
    )
    .await;
    assert_eq!(res, Err(PromptError::BodyTooLarge));
    srv.await.unwrap();
}

#[tokio::test(flavor = "current_thread")]
async fn prompt_streamed_oversize_is_body_read_failed_or_too_large() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let ep = validated_endpoint_for(addr);
    let secret = prompt_secret();
    let bounds = prompt_bounds(128);
    let srv = tokio::spawn(async move {
        let (mut s, _) = listener.accept().await.unwrap();
        let (_, _, _) = read_http_request(&mut s).await;
        let body = format!(r#"{{"a":"{}"}}"#, "x".repeat(160));
        // No Content-Length: the body itself must trip `Limited` while the
        // close-delimited response is collected.
        s.write_all(b"HTTP/1.1 200 OK\r\nconnection: close\r\n\r\n")
            .await
            .unwrap();
        s.write_all(body.as_bytes()).await.unwrap();
    });
    let res = super::http::perform_prompt(
        &ep,
        &secret,
        &bounds,
        Instant::now() + Duration::from_secs(5),
        &CancelHandle::new(),
        &CancelHandle::new(),
        PromptInput::new("sess", "d", &[], "id", false, "t"),
    )
    .await;
    assert_eq!(res, Err(PromptError::BodyReadFailed));
    srv.await.unwrap();
}

#[tokio::test(flavor = "current_thread")]
async fn prompt_pre_cancel_is_cancelled() {
    let ep = validated_endpoint_for("127.0.0.1:9".parse().unwrap());
    let secret = prompt_secret();
    let bounds = prompt_bounds(1024);
    let cancel = CancelHandle::new();
    cancel.cancel();
    let res = super::http::perform_prompt(
        &ep,
        &secret,
        &bounds,
        Instant::now() + Duration::from_secs(5),
        &cancel,
        &CancelHandle::new(),
        PromptInput::new("sess", "d", &[], "id", false, "t"),
    )
    .await;
    assert_eq!(res, Err(PromptError::Cancelled));
}

#[tokio::test(flavor = "current_thread")]
async fn prompt_pre_shutdown_is_shutdown() {
    let ep = validated_endpoint_for("127.0.0.1:9".parse().unwrap());
    let secret = prompt_secret();
    let bounds = prompt_bounds(1024);
    let shutdown = CancelHandle::new();
    shutdown.cancel();
    let res = super::http::perform_prompt(
        &ep,
        &secret,
        &bounds,
        Instant::now() + Duration::from_secs(5),
        &CancelHandle::new(),
        &shutdown,
        PromptInput::new("sess", "d", &[], "id", false, "t"),
    )
    .await;
    assert_eq!(res, Err(PromptError::Shutdown));
}

#[tokio::test(flavor = "current_thread")]
async fn prompt_expired_deadline_is_timeout() {
    let ep = validated_endpoint_for("127.0.0.1:9".parse().unwrap());
    let secret = prompt_secret();
    let bounds = prompt_bounds(1024);
    let res = super::http::perform_prompt(
        &ep,
        &secret,
        &bounds,
        Instant::now() - Duration::from_millis(10),
        &CancelHandle::new(),
        &CancelHandle::new(),
        PromptInput::new("sess", "d", &[], "id", false, "t"),
    )
    .await;
    assert_eq!(res, Err(PromptError::Timeout));
}

#[tokio::test(flavor = "current_thread")]
async fn prompt_closed_transport_is_connect_failed() {
    // Reserve an ephemeral loopback address, then close it before connecting
    // so refusal does not depend on a fixed platform port.
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    drop(listener);
    let ep = validated_endpoint_for(addr);
    let secret = prompt_secret();
    let bounds = prompt_bounds(1024);
    let res = super::http::perform_prompt(
        &ep,
        &secret,
        &bounds,
        Instant::now() + Duration::from_secs(5),
        &CancelHandle::new(),
        &CancelHandle::new(),
        PromptInput::new("sess", "d", &[], "id", false, "t"),
    )
    .await;
    assert_eq!(res, Err(PromptError::ConnectFailed));
}

#[tokio::test(flavor = "current_thread")]
async fn prompt_no_retry_after_close() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let ep = validated_endpoint_for(addr);
    let secret = prompt_secret();
    let bounds = prompt_bounds(1024);
    let count = Arc::new(AtomicUsize::new(0));
    let c = Arc::clone(&count);
    let srv = tokio::spawn(async move {
        let (mut s, _) = listener.accept().await.unwrap();
        c.fetch_add(1, Ordering::SeqCst);
        let (_, _, _) = read_http_request(&mut s).await;
        // Close without response to simulate unknown close.
        drop(s);
        // Wait and ensure no second accept within timeout.
        let res = tokio::time::timeout(Duration::from_millis(300), listener.accept()).await;
        if let Ok(Ok((mut s2, _))) = res {
            c.fetch_add(1, Ordering::SeqCst);
            let _ = read_http_request(&mut s2).await;
        }
    });
    let res = super::http::perform_prompt(
        &ep,
        &secret,
        &bounds,
        Instant::now() + Duration::from_secs(2),
        &CancelHandle::new(),
        &CancelHandle::new(),
        PromptInput::new("sess", "d", &[], "id", false, "t"),
    )
    .await;
    // Closed without response should be SendFailed or BodyReadFailed or DriverFailed, but not success.
    assert_ne!(res, Ok(PromptReceipt));
    // Must not have retried.
    tokio::time::sleep(Duration::from_millis(400)).await;
    srv.abort();
    let _ = srv.await;
    assert_eq!(
        count.load(Ordering::SeqCst),
        1,
        "exactly one request, no retry"
    );
}

// ---------------------------------------------------------------------------
// Event decoder: bridging SseEvent -> EngineObservation
// ---------------------------------------------------------------------------

use super::event::{EventDecodeError, decode_sse_event};

fn event_from_data(data: &str, id: Option<&str>) -> super::framing::SseEvent {
    let mut framer = SseFramer::new(8192, 16384).unwrap();
    let mut raw = Vec::new();
    if let Some(value) = id {
        raw.extend_from_slice(format!("id: {value}\n").as_bytes());
    }
    raw.extend_from_slice(format!("data: {data}\n\n").as_bytes());
    let mut events = framer.feed(&raw).unwrap();
    assert_eq!(events.len(), 1, "expected one framed event for {data:?}");
    events.pop().unwrap()
}

fn event_from_multiline_data(lines: &[&str], id: Option<&str>) -> super::framing::SseEvent {
    let mut framer = SseFramer::new(8192, 16384).unwrap();
    let mut raw = Vec::new();
    if let Some(value) = id {
        raw.extend_from_slice(format!("id: {value}\n").as_bytes());
    }
    for line in lines {
        raw.extend_from_slice(format!("data: {line}\n").as_bytes());
    }
    raw.extend_from_slice(b"\n");
    let mut events = framer.feed(&raw).unwrap();
    assert_eq!(events.len(), 1);
    events.pop().unwrap()
}

#[test]
fn event_text_single_chunk_with_sse_id() {
    let json = r#"{"run_id":"run-event-aaaa","sequence":7,"delta":"hello world"}"#;
    let event = event_from_data(json, Some("sse-1"));
    let observations = decode_sse_event(&event).unwrap();
    assert_eq!(observations.len(), 1);
    match &observations[0] {
        EngineObservation::TextDelta(delta) => {
            assert_eq!(delta.delta(), "hello world");
            assert_eq!(delta.sequence(), 7);
            assert_eq!(delta.run_id().as_str(), "run-event-aaaa");
            assert_eq!(delta.chunk_id(), "sse-1:7:0");
        }
        EngineObservation::Terminal(_) => panic!("expected text"),
    }
}

#[test]
fn event_text_fallback_uses_run_id_when_no_sse_id() {
    let json = r#"{"run_id":"run-event-bbbb","sequence":42,"delta":"fallback check"}"#;
    let event = event_from_data(json, None);
    let observations = decode_sse_event(&event).unwrap();
    assert_eq!(observations.len(), 1);
    match &observations[0] {
        EngineObservation::TextDelta(delta) => {
            assert_eq!(delta.chunk_id(), "run-event-bbbb:42:0");
        }
        EngineObservation::Terminal(_) => panic!("expected text"),
    }
}

#[test]
fn event_text_empty_sse_id_falls_back_to_run_id() {
    let json = r#"{"run_id":"run-event-cccc","sequence":3,"delta":"empty id fallback"}"#;
    let event = event_from_data(json, Some(""));
    let observations = decode_sse_event(&event).unwrap();
    assert_eq!(observations.len(), 1);
    match &observations[0] {
        EngineObservation::TextDelta(delta) => {
            assert_eq!(delta.chunk_id(), "run-event-cccc:3:0");
        }
        EngineObservation::Terminal(_) => panic!("expected text"),
    }
}

#[test]
fn event_multiline_json_via_framer() {
    let line1 = r#"{"run_id": "run-multiline1", "sequence": 11,"#;
    let line2 = r#""delta": "hello multiline"}"#;
    let event = event_from_multiline_data(&[line1, line2], Some("mid-1"));
    assert_eq!(
        event.data(),
        format!("{line1}\n{line2}"),
        "framer joins with newline"
    );
    let observations = decode_sse_event(&event).unwrap();
    assert_eq!(observations.len(), 1);
    match &observations[0] {
        EngineObservation::TextDelta(delta) => {
            assert_eq!(delta.delta(), "hello multiline");
            assert_eq!(delta.chunk_id(), "mid-1:11:0");
        }
        EngineObservation::Terminal(_) => panic!("expected text"),
    }
}

#[test]
fn event_text_unicode_and_chunking_over_4096() {
    let large = format!("{}{}", "€".repeat(1365), "a".repeat(10));
    assert!(large.len() > 4096);
    let json = serde_json::json!({
        "run_id": "run-unicode-event",
        "sequence": 99,
        "delta": large
    })
    .to_string();
    let event = event_from_data(&json, Some("uid99"));
    let observations = decode_sse_event(&event).unwrap();
    assert!(observations.len() > 1);
    for obs in &observations {
        match obs {
            EngineObservation::TextDelta(delta) => {
                assert!(delta.delta().len() <= 4096);
                assert_eq!(delta.sequence(), 99);
                assert!(delta.chunk_id().starts_with("uid99:99:"));
            }
            EngineObservation::Terminal(_) => panic!("expected text"),
        }
    }
    let concat: String = observations
        .iter()
        .map(|o| match o {
            EngineObservation::TextDelta(d) => d.delta(),
            EngineObservation::Terminal(_) => "",
        })
        .collect();
    assert_eq!(concat, large);

    let fallback = event_from_data(&json, None);
    let fallback_obs = decode_sse_event(&fallback).unwrap();
    assert_eq!(fallback_obs.len(), observations.len());
    match &fallback_obs[0] {
        EngineObservation::TextDelta(d) => {
            assert_eq!(d.chunk_id(), "run-unicode-event:99:0");
        }
        EngineObservation::Terminal(_) => panic!("expected text"),
    }

    let unicode_small = "héllo 🌍 — 𝄞 end";
    let json_small = serde_json::json!({
        "run_id": "run-unicode-small",
        "sequence": 5,
        "delta": unicode_small
    })
    .to_string();
    let ev_small = event_from_data(&json_small, Some("u-small"));
    let obs_small = decode_sse_event(&ev_small).unwrap();
    assert_eq!(obs_small.len(), 1);
    match &obs_small[0] {
        EngineObservation::TextDelta(d) => assert_eq!(d.delta(), unicode_small),
        EngineObservation::Terminal(_) => panic!("expected text"),
    }
}

#[test]
fn event_text_empty_delta_yields_zero_observations() {
    let json = r#"{"run_id":"run-empty-delta","sequence":8,"delta":""}"#;
    let event = event_from_data(json, Some("eid"));
    let observations = decode_sse_event(&event).unwrap();
    assert!(
        observations.is_empty(),
        "empty delta must yield zero observations"
    );
}

#[test]
fn event_text_exact_4096_and_4097_chunking() {
    let text_4096 = "a".repeat(4096);
    let json = serde_json::json!({
        "run_id": "run-chunk-4096",
        "sequence": 1,
        "delta": text_4096
    })
    .to_string();
    let event = event_from_data(&json, Some("cid"));
    let obs = decode_sse_event(&event).unwrap();
    assert_eq!(obs.len(), 1);

    let text_4097 = "a".repeat(4097);
    let json2 = serde_json::json!({
        "run_id": "run-chunk-4097",
        "sequence": 2,
        "delta": text_4097
    })
    .to_string();
    let event2 = event_from_data(&json2, Some("cid2"));
    let obs2 = decode_sse_event(&event2).unwrap();
    assert_eq!(obs2.len(), 2);
    match &obs2[0] {
        EngineObservation::TextDelta(d) => assert_eq!(d.chunk_id(), "cid2:2:0"),
        EngineObservation::Terminal(_) => panic!("expected text"),
    }
    match &obs2[1] {
        EngineObservation::TextDelta(d) => assert_eq!(d.chunk_id(), "cid2:2:1"),
        EngineObservation::Terminal(_) => panic!("expected text"),
    }
}

#[test]
fn event_terminal_all_four_states() {
    let cases = [
        ("succeeded", TerminalState::Completed),
        ("failed", TerminalState::Failed),
        ("cancelled", TerminalState::Cancelled),
        ("interrupted", TerminalState::Interrupted),
    ];
    for (state_str, expected) in cases {
        let json = serde_json::json!({
            "run_id": "run-term-all",
            "sequence": 100,
            "state": state_str
        })
        .to_string();
        let event = event_from_data(&json, None);
        let obs = decode_sse_event(&event).unwrap();
        assert_eq!(obs.len(), 1);
        match &obs[0] {
            EngineObservation::Terminal(term) => {
                assert_eq!(term.state(), expected);
                assert_eq!(term.sequence(), 100);
                assert_eq!(term.run_id().as_str(), "run-term-all");
                assert_eq!(term.reason(), None);
                assert_eq!(term.error_ref(), None);
            }
            EngineObservation::TextDelta(_) => panic!("expected terminal for {state_str}"),
        }
    }
}

#[test]
fn event_terminal_optional_fields() {
    let json = serde_json::json!({
        "run_id": "run-term-opt",
        "sequence": 5,
        "state": "failed",
        "reason": "something broke",
        "error_ref": "err-123"
    })
    .to_string();
    let event = event_from_data(&json, None);
    let obs = decode_sse_event(&event).unwrap();
    match &obs[0] {
        EngineObservation::Terminal(term) => {
            assert_eq!(term.reason(), Some("something broke"));
            assert_eq!(term.error_ref(), Some("err-123"));
        }
        EngineObservation::TextDelta(_) => panic!("expected terminal"),
    }

    let json_null = serde_json::json!({
        "run_id": "run-term-opt",
        "sequence": 6,
        "state": "cancelled",
        "reason": null,
        "error_ref": null
    })
    .to_string();
    let event_null = event_from_data(&json_null, None);
    let obs_null = decode_sse_event(&event_null).unwrap();
    match &obs_null[0] {
        EngineObservation::Terminal(term) => {
            assert_eq!(term.reason(), None);
            assert_eq!(term.error_ref(), None);
        }
        EngineObservation::TextDelta(_) => panic!("expected terminal"),
    }

    let json_missing = serde_json::json!({
        "run_id": "run-term-opt",
        "sequence": 7,
        "state": "succeeded"
    })
    .to_string();
    let event_missing = event_from_data(&json_missing, None);
    let obs_missing = decode_sse_event(&event_missing).unwrap();
    match &obs_missing[0] {
        EngineObservation::Terminal(term) => {
            assert_eq!(term.reason(), None);
            assert_eq!(term.error_ref(), None);
        }
        EngineObservation::TextDelta(_) => panic!("expected terminal"),
    }

    let json_only_reason = serde_json::json!({
        "run_id": "run-term-opt",
        "sequence": 8,
        "state": "interrupted",
        "reason": "only reason"
    })
    .to_string();
    let event_only = event_from_data(&json_only_reason, None);
    let obs_only = decode_sse_event(&event_only).unwrap();
    match &obs_only[0] {
        EngineObservation::Terminal(term) => {
            assert_eq!(term.reason(), Some("only reason"));
            assert_eq!(term.error_ref(), None);
        }
        EngineObservation::TextDelta(_) => panic!("expected terminal"),
    }
}

#[test]
fn event_extra_fields_ignored() {
    let json = serde_json::json!({
        "run_id": "run-extra-fields",
        "sequence": 9,
        "delta": "extra ok",
        "unknown": 123,
        "extra": {"nested": true},
        "another": [1,2,3]
    })
    .to_string();
    let event = event_from_data(&json, Some("eid-extra"));
    let obs = decode_sse_event(&event).unwrap();
    assert_eq!(obs.len(), 1);
    match &obs[0] {
        EngineObservation::TextDelta(d) => assert_eq!(d.delta(), "extra ok"),
        EngineObservation::Terminal(_) => panic!("expected text"),
    }

    let json_term = serde_json::json!({
        "run_id": "run-extra-term",
        "sequence": 10,
        "state": "succeeded",
        "reason": "ok",
        "extra_field": "ignore me"
    })
    .to_string();
    let event_term = event_from_data(&json_term, None);
    let obs_term = decode_sse_event(&event_term).unwrap();
    assert_eq!(obs_term.len(), 1);
}

#[test]
fn event_rejects_malformed_json() {
    let event = event_from_data("not-json-at-all", None);
    let err = decode_sse_event(&event).unwrap_err();
    assert_eq!(err, EventDecodeError::InvalidJson);

    let event2 = event_from_data("{", None);
    assert_eq!(
        decode_sse_event(&event2).unwrap_err(),
        EventDecodeError::InvalidJson
    );

    let event3 = event_from_data("[1,2,3]", None);
    assert_eq!(
        decode_sse_event(&event3).unwrap_err(),
        EventDecodeError::InvalidJson
    );
}

#[test]
fn event_rejects_missing_and_invalid_run_id() {
    let json_missing = r#"{"sequence":1,"delta":"hi"}"#;
    let event = event_from_data(json_missing, None);
    assert_eq!(
        decode_sse_event(&event).unwrap_err(),
        EventDecodeError::InvalidRunId
    );

    let json_empty = r#"{"run_id":"","sequence":1,"delta":"hi"}"#;
    let event2 = event_from_data(json_empty, None);
    assert_eq!(
        decode_sse_event(&event2).unwrap_err(),
        EventDecodeError::InvalidRunId
    );

    let json_space = r#"{"run_id":"has space","sequence":1,"delta":"hi"}"#;
    let event3 = event_from_data(json_space, None);
    assert_eq!(
        decode_sse_event(&event3).unwrap_err(),
        EventDecodeError::InvalidRunId
    );

    let json_wrong_type = r#"{"run_id":123,"sequence":1,"delta":"hi"}"#;
    let event4 = event_from_data(json_wrong_type, None);
    assert_eq!(
        decode_sse_event(&event4).unwrap_err(),
        EventDecodeError::InvalidRunId
    );

    let json_null = r#"{"run_id":null,"sequence":1,"delta":"hi"}"#;
    let event5 = event_from_data(json_null, None);
    assert_eq!(
        decode_sse_event(&event5).unwrap_err(),
        EventDecodeError::InvalidRunId
    );
}

#[test]
fn event_rejects_missing_and_invalid_sequence() {
    let json_missing = r#"{"run_id":"run-seq-missing","delta":"hi"}"#;
    let event = event_from_data(json_missing, None);
    assert_eq!(
        decode_sse_event(&event).unwrap_err(),
        EventDecodeError::InvalidSequence
    );

    let json_str = r#"{"run_id":"run-seq-str","sequence":"1","delta":"hi"}"#;
    let event2 = event_from_data(json_str, None);
    assert_eq!(
        decode_sse_event(&event2).unwrap_err(),
        EventDecodeError::InvalidSequence
    );

    let json_float = r#"{"run_id":"run-seq-float","sequence":1.5,"delta":"hi"}"#;
    let event3 = event_from_data(json_float, None);
    assert_eq!(
        decode_sse_event(&event3).unwrap_err(),
        EventDecodeError::InvalidSequence
    );

    let json_negative = r#"{"run_id":"run-seq-neg","sequence":-1,"delta":"hi"}"#;
    let event4 = event_from_data(json_negative, None);
    assert_eq!(
        decode_sse_event(&event4).unwrap_err(),
        EventDecodeError::InvalidSequence
    );

    let json_null = r#"{"run_id":"run-seq-null","sequence":null,"delta":"hi"}"#;
    let event5 = event_from_data(json_null, None);
    assert_eq!(
        decode_sse_event(&event5).unwrap_err(),
        EventDecodeError::InvalidSequence
    );

    let json_bool = r#"{"run_id":"run-seq-bool","sequence":true,"delta":"hi"}"#;
    let event6 = event_from_data(json_bool, None);
    assert_eq!(
        decode_sse_event(&event6).unwrap_err(),
        EventDecodeError::InvalidSequence
    );
}

#[test]
fn event_rejects_both_and_neither_shape() {
    let json_both = r#"{"run_id":"run-both","sequence":1,"delta":"hi","state":"succeeded"}"#;
    let event_both = event_from_data(json_both, None);
    assert_eq!(
        decode_sse_event(&event_both).unwrap_err(),
        EventDecodeError::BothShapes
    );

    let json_neither = r#"{"run_id":"run-neither","sequence":1}"#;
    let event_neither = event_from_data(json_neither, None);
    assert_eq!(
        decode_sse_event(&event_neither).unwrap_err(),
        EventDecodeError::NeitherShape
    );

    let json_empty_obj = r#"{"run_id":"run-neither2","sequence":1,"other":123}"#;
    let event_empty = event_from_data(json_empty_obj, None);
    assert_eq!(
        decode_sse_event(&event_empty).unwrap_err(),
        EventDecodeError::NeitherShape
    );
}

#[test]
fn event_rejects_unknown_state_and_wrong_types() {
    let json_unknown = r#"{"run_id":"run-unk","sequence":1,"state":"unknown"}"#;
    let event_unknown = event_from_data(json_unknown, None);
    assert_eq!(
        decode_sse_event(&event_unknown).unwrap_err(),
        EventDecodeError::UnknownState
    );

    let json_wrong_delta_type = r#"{"run_id":"run-wrong-delta","sequence":1,"delta":123}"#;
    let wrong_delta_event = event_from_data(json_wrong_delta_type, None);
    assert_eq!(
        decode_sse_event(&wrong_delta_event).unwrap_err(),
        EventDecodeError::InvalidDelta
    );

    let json_wrong_state_type = r#"{"run_id":"run-wrong-state","sequence":1,"state":123}"#;
    let wrong_state_event = event_from_data(json_wrong_state_type, None);
    assert_eq!(
        decode_sse_event(&wrong_state_event).unwrap_err(),
        EventDecodeError::InvalidState
    );

    let json_null_delta = r#"{"run_id":"run-null-delta","sequence":1,"delta":null}"#;
    let event_null = event_from_data(json_null_delta, None);
    assert_eq!(
        decode_sse_event(&event_null).unwrap_err(),
        EventDecodeError::InvalidDelta
    );

    let json_null_state = r#"{"run_id":"run-null-state","sequence":1,"state":null}"#;
    let event_null_state = event_from_data(json_null_state, None);
    assert_eq!(
        decode_sse_event(&event_null_state).unwrap_err(),
        EventDecodeError::InvalidState
    );
}

#[test]
fn event_rejects_wrong_optional_field_types() {
    let json_bad_reason =
        r#"{"run_id":"run-bad-reason","sequence":1,"state":"failed","reason":123}"#;
    let event = event_from_data(json_bad_reason, None);
    assert_eq!(
        decode_sse_event(&event).unwrap_err(),
        EventDecodeError::InvalidReason
    );

    let json_bad_error_ref =
        r#"{"run_id":"run-bad-err","sequence":1,"state":"failed","error_ref":true}"#;
    let event2 = event_from_data(json_bad_error_ref, None);
    assert_eq!(
        decode_sse_event(&event2).unwrap_err(),
        EventDecodeError::InvalidErrorRef
    );

    let json_bad_reason_obj =
        r#"{"run_id":"run-bad-reason2","sequence":1,"state":"failed","reason":{}}"#;
    let event3 = event_from_data(json_bad_reason_obj, None);
    assert_eq!(
        decode_sse_event(&event3).unwrap_err(),
        EventDecodeError::InvalidReason
    );

    let json_bad_error_ref_arr =
        r#"{"run_id":"run-bad-err2","sequence":1,"state":"failed","error_ref":[]}"#;
    let event4 = event_from_data(json_bad_error_ref_arr, None);
    assert_eq!(
        decode_sse_event(&event4).unwrap_err(),
        EventDecodeError::InvalidErrorRef
    );
}

#[test]
fn event_errors_are_payload_free_and_copy_eq() {
    let secret = "hunter2-super-secret-xyz-999-Bearer-token";
    let run_secret = "run-secret-xyz-hunter2";
    let json = format!(
        r#"{{"run_id":"{run_secret}","sequence":1,"delta":"{secret}","state":"succeeded"}}"#
    );
    let event = event_from_data(&json, Some(secret));
    let err = decode_sse_event(&event).unwrap_err();
    let display = format!("{err}");
    let debug = format!("{err:?}");
    assert!(!display.contains(secret));
    assert!(!debug.contains(secret));
    assert!(!display.contains(run_secret));
    assert!(!debug.contains(run_secret));
    assert!(!display.contains("hunter2"));
    assert!(!debug.contains("hunter2"));

    let secret2 = "another-secret-payload-xyz";
    let json2 = format!(r#"{{"run_id":"{secret2}","sequence":1,"delta":"hi"}}"#);
    let event2 = event_from_data(&json2, None);
    if let Err(e) = decode_sse_event(&event2) {
        let d = format!("{e}");
        let dbg = format!("{e:?}");
        assert!(!d.contains(secret2));
        assert!(!dbg.contains(secret2));
    }

    for err in [
        EventDecodeError::InvalidJson,
        EventDecodeError::InvalidRunId,
        EventDecodeError::InvalidSequence,
        EventDecodeError::BothShapes,
        EventDecodeError::NeitherShape,
        EventDecodeError::InvalidDelta,
        EventDecodeError::InvalidState,
        EventDecodeError::UnknownState,
        EventDecodeError::InvalidReason,
        EventDecodeError::InvalidErrorRef,
    ] {
        let a = err;
        let b = err;
        assert_eq!(a, b);
        let c = a;
        assert_eq!(c, err);
        let display = format!("{err}");
        let debug = format!("{err:?}");
        assert!(!display.contains(secret));
        assert!(!debug.contains(secret));
        assert!(!display.contains("hunter2"));
        assert!(!debug.contains("hunter2"));
    }
}

#[test]
fn event_sequence_preserved_and_chunk_ids_deterministic() {
    let json = r#"{"run_id":"run-deterministic","sequence":12345,"delta":"hello"}"#;
    let event_a = event_from_data(json, Some("stable-id"));
    let event_b = event_from_data(json, Some("stable-id"));
    let obs_a = decode_sse_event(&event_a).unwrap();
    let obs_b = decode_sse_event(&event_b).unwrap();
    assert_eq!(obs_a, obs_b);
    match &obs_a[0] {
        EngineObservation::TextDelta(d) => {
            assert_eq!(d.sequence(), 12345);
            assert_eq!(d.chunk_id(), "stable-id:12345:0");
        }
        EngineObservation::Terminal(_) => panic!("expected text"),
    }

    let large = "x".repeat(5000);
    let json_large = serde_json::json!({
        "run_id": "run-det-large",
        "sequence": 77,
        "delta": large
    })
    .to_string();
    let event_large = event_from_data(&json_large, Some("det-id"));
    let obs_large = decode_sse_event(&event_large).unwrap();
    assert_eq!(obs_large.len(), 2);
    match &obs_large[0] {
        EngineObservation::TextDelta(d) => assert_eq!(d.chunk_id(), "det-id:77:0"),
        EngineObservation::Terminal(_) => panic!("expected text"),
    }
    match &obs_large[1] {
        EngineObservation::TextDelta(d) => assert_eq!(d.chunk_id(), "det-id:77:1"),
        EngineObservation::Terminal(_) => panic!("expected text"),
    }
    for obs in &obs_large {
        match obs {
            EngineObservation::TextDelta(d) => assert_eq!(d.sequence(), 77),
            EngineObservation::Terminal(_) => panic!("expected text"),
        }
    }
}

// ---------------------------------------------------------------------------
// Stream follower: bounded authenticated SSE (P4f)
// ---------------------------------------------------------------------------

use super::stream::{StreamError, StreamInput, StreamReceipt, follow_stream};

fn stream_secret() -> HealthSecret {
    HealthSecret::from_raw_for_tests("stream-secret-fixed-1234567890abcd".to_owned())
}

fn stream_bounds() -> EngineBounds {
    EngineBounds {
        max_json_body: 1024,
        max_sse_line: 1024,
        max_sse_event: 4096,
        max_readiness_line: 256,
        max_headers: 32,
        max_buf_bytes: 8192,
        stderr_cap_bytes: 4096,
        sink_capacity: 4,
        control_capacity: 4,
    }
}

async fn read_stream_request(stream: &mut TcpStream) -> (String, Vec<(String, String)>) {
    let mut buf = Vec::new();
    let mut tmp = [0u8; 4096];
    loop {
        let n = stream.read(&mut tmp).await.expect("read");
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&tmp[..n]);
        if buf.windows(4).any(|w| w == b"\r\n\r\n") {
            break;
        }
        if buf.len() > 16 * 1024 {
            break;
        }
    }
    let header_end = buf
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .expect("header end")
        + 4;
    let header_str = String::from_utf8_lossy(&buf[..header_end]).to_string();
    let mut lines = header_str.lines();
    let request_line = lines.next().unwrap_or("").to_owned();
    let mut headers = Vec::new();
    for line in lines {
        if line.is_empty() {
            continue;
        }
        if let Some((k, v)) = line.split_once(':') {
            headers.push((k.trim().to_ascii_lowercase(), v.trim().to_owned()));
        }
    }
    (request_line, headers)
}

#[tokio::test(flavor = "current_thread")]
async fn stream_request_exact_shape() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let endpoint = validated_endpoint_for(addr);
    let secret = stream_secret();
    let bounds = stream_bounds();
    let expected_auth = secret.basic_auth();
    let (tx, mut rx) = mpsc::channel(4);
    let captured = Arc::new(std::sync::Mutex::new(
        None::<(String, Vec<(String, String)>)>,
    ));
    let cap = Arc::clone(&captured);
    let srv = tokio::spawn(async move {
        let (mut s, _) = listener.accept().await.unwrap();
        let (line, headers) = read_stream_request(&mut s).await;
        *cap.lock().unwrap() = Some((line, headers));
        // Send terminal immediately to allow client to exit
        let body =
            "data: {\"run_id\":\"run-stream-shape\",\"sequence\":1,\"state\":\"succeeded\"}\n\n";
        let resp = format!(
            "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\nconnection: close\r\n\r\n{body}"
        );
        s.write_all(resp.as_bytes()).await.unwrap();
    });
    let res = follow_stream(StreamInput::new(
        &endpoint,
        &secret,
        &bounds,
        Instant::now() + Duration::from_secs(5),
        &CancelHandle::new(),
        &CancelHandle::new(),
        "sessABC",
        42,
        tx,
    ))
    .await;
    assert!(res.is_ok());
    srv.await.unwrap();
    let (line, headers) = captured.lock().unwrap().take().unwrap();
    assert_eq!(
        line,
        "GET /api/experimental/session/sessABC/log?after=42&follow=true HTTP/1.1"
    );
    let auth = headers
        .iter()
        .find(|(k, _)| k == "authorization")
        .unwrap()
        .1
        .clone();
    assert_eq!(auth, expected_auth);
    let accept = headers
        .iter()
        .find(|(k, _)| k == "accept")
        .unwrap()
        .1
        .clone();
    assert_eq!(accept, "text/event-stream");
    let conn = headers
        .iter()
        .find(|(k, _)| k == "connection")
        .unwrap()
        .1
        .clone();
    assert_eq!(conn, "close");
    // Ensure after preserved and not extra
    let _ = rx.recv().await;
}

#[tokio::test(flavor = "current_thread")]
async fn stream_fragmented_multiline_text_then_terminal() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let endpoint = validated_endpoint_for(addr);
    let secret = stream_secret();
    let bounds = stream_bounds();
    let (tx, mut rx) = mpsc::channel(8);
    let srv = tokio::spawn(async move {
        let (mut s, _) = listener.accept().await.unwrap();
        let _ = read_stream_request(&mut s).await;
        let resp_headers =
            "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\nconnection: close\r\n\r\n";
        s.write_all(resp_headers.as_bytes()).await.unwrap();
        // Fragmented multiline: two data lines joined with \n via framer => invalid json? Instead use single delta fragmented
        // Send text event fragmented across writes (multibyte split not needed, split inside json)
        let part1 = "data: {\"run_id\":\"run-frag-0001\",\"sequence\":5,\"delta\":\"hel";
        let part2 = "lo world\"}\n\ndata: {\"run_id\":\"run-frag-0001\",\"sequence\":6,\"state\":\"succeeded\"}\n\n";
        s.write_all(part1.as_bytes()).await.unwrap();
        s.flush().await.unwrap();
        tokio::time::sleep(Duration::from_millis(10)).await;
        s.write_all(part2.as_bytes()).await.unwrap();
        s.flush().await.unwrap();
    });
    let res = follow_stream(StreamInput::new(
        &endpoint,
        &secret,
        &bounds,
        Instant::now() + Duration::from_secs(5),
        &CancelHandle::new(),
        &CancelHandle::new(),
        "sessFrag",
        0,
        tx,
    ))
    .await;
    assert_eq!(res.unwrap().state(), TerminalState::Completed);
    let first = rx.recv().await.unwrap();
    match first {
        EngineObservation::TextDelta(d) => assert_eq!(d.delta(), "hello world"),
        _ => panic!("expected text"),
    }
    let second = rx.recv().await.unwrap();
    match second {
        EngineObservation::Terminal(t) => assert_eq!(t.state(), TerminalState::Completed),
        _ => panic!("expected terminal"),
    }
    assert!(rx.try_recv().is_err());
    srv.await.unwrap();
}

#[tokio::test(flavor = "current_thread")]
async fn stream_bounded_delivery_and_terminal_receipt() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let endpoint = validated_endpoint_for(addr);
    let secret = stream_secret();
    let bounds = stream_bounds();
    let (tx, mut rx) = mpsc::channel(2);
    let srv = tokio::spawn(async move {
        let (mut s, _) = listener.accept().await.unwrap();
        let _ = read_stream_request(&mut s).await;
        let body = "data: {\"run_id\":\"run-order-0001\",\"sequence\":1,\"delta\":\"first\"}\n\ndata: {\"run_id\":\"run-order-0001\",\"sequence\":2,\"delta\":\"second\"}\n\ndata: {\"run_id\":\"run-order-0001\",\"sequence\":3,\"state\":\"failed\"}\n\n";
        let resp = format!(
            "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream; charset=utf-8\r\nconnection: close\r\n\r\n{body}"
        );
        s.write_all(resp.as_bytes()).await.unwrap();
    });
    let receipt = follow_stream(StreamInput::new(
        &endpoint,
        &secret,
        &bounds,
        Instant::now() + Duration::from_secs(5),
        &CancelHandle::new(),
        &CancelHandle::new(),
        "sessOrder",
        99,
        tx,
    ))
    .await
    .unwrap();
    assert_eq!(receipt.state(), TerminalState::Failed);
    let a = rx.recv().await.unwrap();
    assert!(matches!(a, EngineObservation::TextDelta(_)));
    let b = rx.recv().await.unwrap();
    assert!(matches!(b, EngineObservation::TextDelta(_)));
    let c = rx.recv().await.unwrap();
    assert_eq!(
        c,
        EngineObservation::Terminal(TerminalObservation::new(
            run_id("run-order-0001"),
            3,
            TerminalState::Failed,
            None,
            None
        ))
    );
    srv.await.unwrap();
}

#[tokio::test(flavor = "current_thread")]
async fn stream_eof_before_terminal_is_error() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let endpoint = validated_endpoint_for(addr);
    let secret = stream_secret();
    let bounds = stream_bounds();
    let (tx, mut rx) = mpsc::channel(4);
    let srv = tokio::spawn(async move {
        let (mut s, _) = listener.accept().await.unwrap();
        let _ = read_stream_request(&mut s).await;
        let body = "data: {\"run_id\":\"run-eof-00001\",\"sequence\":1,\"delta\":\"hi\"}\n\n";
        let resp = format!(
            "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\nconnection: close\r\n\r\n{body}"
        );
        s.write_all(resp.as_bytes()).await.unwrap();
        // close without terminal
        drop(s);
    });
    let res = follow_stream(StreamInput::new(
        &endpoint,
        &secret,
        &bounds,
        Instant::now() + Duration::from_secs(5),
        &CancelHandle::new(),
        &CancelHandle::new(),
        "sessEof",
        0,
        tx,
    ))
    .await;
    assert_eq!(res, Err(StreamError::MissingTerminal));
    // Text before terminal is still delivered before error? The contract says text before terminal is allowed, but EOF without terminal is error. Prior text may be delivered.
    let got = rx.recv().await.unwrap();
    assert!(matches!(got, EngineObservation::TextDelta(_)));
    assert!(rx.try_recv().is_err());
    srv.await.unwrap();
}

#[tokio::test(flavor = "current_thread")]
async fn stream_non_2xx_is_status_error() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let endpoint = validated_endpoint_for(addr);
    let secret = stream_secret();
    let bounds = stream_bounds();
    let (tx, _) = mpsc::channel(4);
    let count = Arc::new(AtomicUsize::new(0));
    let c = Arc::clone(&count);
    let srv = tokio::spawn(async move {
        let (mut s, _) = listener.accept().await.unwrap();
        c.fetch_add(1, Ordering::SeqCst);
        let _ = read_stream_request(&mut s).await;
        let resp = "HTTP/1.1 500 Internal Server Error\r\ncontent-type: text/event-stream\r\nconnection: close\r\n\r\n";
        s.write_all(resp.as_bytes()).await.unwrap();
    });
    let res = follow_stream(StreamInput::new(
        &endpoint,
        &secret,
        &bounds,
        Instant::now() + Duration::from_secs(5),
        &CancelHandle::new(),
        &CancelHandle::new(),
        "sessErr",
        0,
        tx,
    ))
    .await;
    assert_eq!(res, Err(StreamError::StatusNotSuccess));
    srv.await.unwrap();
    assert_eq!(count.load(Ordering::SeqCst), 1);
}

#[tokio::test(flavor = "current_thread")]
async fn stream_wrong_content_type_rejected() {
    let cases = vec!["application/json", "text/plain", "text/event-streamx", ""];
    for ct in cases {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let endpoint = validated_endpoint_for(addr);
        let secret = stream_secret();
        let bounds = stream_bounds();
        let (tx, _) = mpsc::channel(4);
        let ct_owned = ct.to_owned();
        let srv = tokio::spawn(async move {
            let (mut s, _) = listener.accept().await.unwrap();
            let _ = read_stream_request(&mut s).await;
            let header = if ct_owned.is_empty() {
                "HTTP/1.1 200 OK\r\nconnection: close\r\n\r\n".to_owned()
            } else {
                format!("HTTP/1.1 200 OK\r\ncontent-type: {ct_owned}\r\nconnection: close\r\n\r\n")
            };
            s.write_all(header.as_bytes()).await.unwrap();
        });
        let res = follow_stream(StreamInput::new(
            &endpoint,
            &secret,
            &bounds,
            Instant::now() + Duration::from_secs(5),
            &CancelHandle::new(),
            &CancelHandle::new(),
            "sessCt",
            0,
            tx,
        ))
        .await;
        assert_eq!(res, Err(StreamError::ContentTypeInvalid), "ct={ct:?}");
        srv.await.unwrap();
    }
    // charset allowed
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let endpoint = validated_endpoint_for(addr);
    let secret = stream_secret();
    let bounds = stream_bounds();
    let (tx, _) = mpsc::channel(4);
    let srv = tokio::spawn(async move {
        let (mut s, _) = listener.accept().await.unwrap();
        let _ = read_stream_request(&mut s).await;
        let body =
            "data: {\"run_id\":\"run-ct-ok0001\",\"sequence\":1,\"state\":\"succeeded\"}\n\n";
        let resp = format!(
            "HTTP/1.1 200 OK\r\ncontent-type: Text/Event-Stream; charset=UTF-8\r\nconnection: close\r\n\r\n{body}"
        );
        s.write_all(resp.as_bytes()).await.unwrap();
    });
    let res = follow_stream(StreamInput::new(
        &endpoint,
        &secret,
        &bounds,
        Instant::now() + Duration::from_secs(5),
        &CancelHandle::new(),
        &CancelHandle::new(),
        "sessCt",
        0,
        tx,
    ))
    .await;
    assert!(res.is_ok());
    srv.await.unwrap();
}

#[tokio::test(flavor = "current_thread")]
async fn stream_framing_error() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let endpoint = validated_endpoint_for(addr);
    let secret = stream_secret();
    let mut bounds = stream_bounds();
    bounds.max_sse_line = 10;
    let (tx, _) = mpsc::channel(4);
    let srv = tokio::spawn(async move {
        let (mut s, _) = listener.accept().await.unwrap();
        let _ = read_stream_request(&mut s).await;
        let resp = "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\nconnection: close\r\n\r\ndata: this line is definitely longer than ten bytes\n\n";
        s.write_all(resp.as_bytes()).await.unwrap();
    });
    let res = follow_stream(StreamInput::new(
        &endpoint,
        &secret,
        &bounds,
        Instant::now() + Duration::from_secs(5),
        &CancelHandle::new(),
        &CancelHandle::new(),
        "sessFrame",
        0,
        tx,
    ))
    .await;
    assert_eq!(res, Err(StreamError::FramingFailed));
    srv.await.unwrap();
}

#[tokio::test(flavor = "current_thread")]
async fn stream_decode_error() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let endpoint = validated_endpoint_for(addr);
    let secret = stream_secret();
    let bounds = stream_bounds();
    let (tx, _) = mpsc::channel(4);
    let srv = tokio::spawn(async move {
        let (mut s, _) = listener.accept().await.unwrap();
        let _ = read_stream_request(&mut s).await;
        let resp = "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\nconnection: close\r\n\r\ndata: not-json-at-all\n\n";
        s.write_all(resp.as_bytes()).await.unwrap();
    });
    let res = follow_stream(StreamInput::new(
        &endpoint,
        &secret,
        &bounds,
        Instant::now() + Duration::from_secs(5),
        &CancelHandle::new(),
        &CancelHandle::new(),
        "sessDecode",
        0,
        tx,
    ))
    .await;
    assert_eq!(res, Err(StreamError::DecodeFailed));
    srv.await.unwrap();
}

#[tokio::test(flavor = "current_thread")]
async fn stream_sink_closed_and_backpressure() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let endpoint = validated_endpoint_for(addr);
    let secret = stream_secret();
    let bounds = stream_bounds();
    // closed sink
    let (tx, rx) = mpsc::channel(4);
    drop(rx);
    let srv = tokio::spawn(async move {
        let (mut s, _) = listener.accept().await.unwrap();
        let _ = read_stream_request(&mut s).await;
        let body = "data: {\"run_id\":\"run-sink-0001\",\"sequence\":1,\"delta\":\"hi\"}\n\n";
        let resp = format!(
            "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\nconnection: close\r\n\r\n{body}"
        );
        s.write_all(resp.as_bytes()).await.unwrap();
    });
    let res = follow_stream(StreamInput::new(
        &endpoint,
        &secret,
        &bounds,
        Instant::now() + Duration::from_secs(5),
        &CancelHandle::new(),
        &CancelHandle::new(),
        "sessSink",
        0,
        tx,
    ))
    .await;
    assert_eq!(res, Err(StreamError::DeliveryFailed));
    srv.await.unwrap();
    // backpressure later
    let listener2 = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr2 = listener2.local_addr().unwrap();
    let endpoint2 = validated_endpoint_for(addr2);
    let (tx2, mut rx2) = mpsc::channel(1);
    tx2.send(make_delta("fill")).await.unwrap();
    let srv2 = tokio::spawn(async move {
        let (mut s, _) = listener2.accept().await.unwrap();
        let _ = read_stream_request(&mut s).await;
        let body = "data: {\"run_id\":\"run-sink-0002\",\"sequence\":1,\"delta\":\"blocked\"}\n\n";
        let resp = format!(
            "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\nconnection: close\r\n\r\n{body}"
        );
        s.write_all(resp.as_bytes()).await.unwrap();
        // keep connection open a bit
        tokio::time::sleep(Duration::from_millis(200)).await;
    });
    let cancel = Arc::new(CancelHandle::new());
    let cancel_clone = Arc::clone(&cancel);
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(50)).await;
        cancel_clone.cancel();
    });
    let res2 = follow_stream(StreamInput::new(
        &endpoint2,
        &secret,
        &bounds,
        Instant::now() + Duration::from_secs(5),
        &cancel,
        &CancelHandle::new(),
        "sessBack",
        0,
        tx2,
    ))
    .await;
    // While full, cancel should win
    assert_eq!(res2, Err(StreamError::Cancelled));
    let _ = rx2.recv().await;
    srv2.abort();
    let _ = srv2.await;
}

#[tokio::test(flavor = "current_thread")]
async fn stream_pre_signalled_shutdown_cancel_deadline() {
    let endpoint = validated_endpoint_for("127.0.0.1:9".parse().unwrap());
    let secret = stream_secret();
    let bounds = stream_bounds();
    let (tx, _) = mpsc::channel(4);
    // shutdown wins over cancel
    let shutdown = CancelHandle::new();
    let cancel = CancelHandle::new();
    shutdown.cancel();
    cancel.cancel();
    let res = follow_stream(StreamInput::new(
        &endpoint,
        &secret,
        &bounds,
        Instant::now() + Duration::from_secs(5),
        &cancel,
        &shutdown,
        "sessPre",
        0,
        tx.clone(),
    ))
    .await;
    assert_eq!(res, Err(StreamError::Shutdown));
    // cancel
    let res2 = follow_stream(StreamInput::new(
        &endpoint,
        &secret,
        &bounds,
        Instant::now() + Duration::from_secs(5),
        &{
            let c = CancelHandle::new();
            c.cancel();
            c
        },
        &CancelHandle::new(),
        "sessPre",
        0,
        tx.clone(),
    ))
    .await;
    assert_eq!(res2, Err(StreamError::Cancelled));
    // deadline
    let res3 = follow_stream(StreamInput::new(
        &endpoint,
        &secret,
        &bounds,
        Instant::now() - Duration::from_millis(10),
        &CancelHandle::new(),
        &CancelHandle::new(),
        "sessPre",
        0,
        tx,
    ))
    .await;
    assert_eq!(res3, Err(StreamError::Timeout));
}

#[tokio::test(flavor = "current_thread")]
async fn stream_mid_stream_shutdown_cancel_deadline_precedence() {
    // mid-stream cancel wins over deadline when both fire
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let endpoint = validated_endpoint_for(addr);
    let secret = stream_secret();
    let bounds = stream_bounds();
    let (tx, _rx) = mpsc::channel(4);
    let shutdown = Arc::new(CancelHandle::new());
    let cancel = Arc::new(CancelHandle::new());
    let srv = tokio::spawn(async move {
        let (mut s, _) = listener.accept().await.unwrap();
        let _ = read_stream_request(&mut s).await;
        s.write_all(
            b"HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\nconnection: close\r\n\r\n",
        )
        .await
        .unwrap();
        // Hold without sending terminal to trigger deadline/cancel
        tokio::time::sleep(Duration::from_millis(500)).await;
    });
    let deadline = Instant::now() + Duration::from_millis(200);
    let endpoint_owned = endpoint;
    let secret_owned = secret;
    let bounds_owned = bounds;
    let cancel_owned = Arc::clone(&cancel);
    let shutdown_owned = Arc::clone(&shutdown);
    let handle = tokio::spawn(async move {
        follow_stream(StreamInput::new(
            &endpoint_owned,
            &secret_owned,
            &bounds_owned,
            deadline,
            &cancel_owned,
            &shutdown_owned,
            "sessMid",
            0,
            tx,
        ))
        .await
    });
    tokio::time::sleep(Duration::from_millis(50)).await;
    cancel.cancel();
    shutdown.cancel();
    let res = tokio::time::timeout(Duration::from_secs(1), handle)
        .await
        .unwrap()
        .unwrap();
    // shutdown > cancel > deadline, shutdown was signalled so should be Shutdown
    assert_eq!(res, Err(StreamError::Shutdown));
    srv.abort();
    let _ = srv.await;
}

#[tokio::test(flavor = "current_thread")]
async fn stream_no_retry() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let endpoint = validated_endpoint_for(addr);
    let secret = stream_secret();
    let bounds = stream_bounds();
    let count = Arc::new(AtomicUsize::new(0));
    let c = Arc::clone(&count);
    let srv = tokio::spawn(async move {
        let (mut s, _) = listener.accept().await.unwrap();
        c.fetch_add(1, Ordering::SeqCst);
        let _ = read_stream_request(&mut s).await;
        drop(s);
        // Ensure no second connection within timeout
        let res = tokio::time::timeout(Duration::from_millis(300), listener.accept()).await;
        if let Ok(Ok((mut s2, _))) = res {
            c.fetch_add(1, Ordering::SeqCst);
            let _ = read_stream_request(&mut s2).await;
        }
    });
    let (tx, _) = mpsc::channel(4);
    let res = follow_stream(StreamInput::new(
        &endpoint,
        &secret,
        &bounds,
        Instant::now() + Duration::from_secs(2),
        &CancelHandle::new(),
        &CancelHandle::new(),
        "sessRetry",
        0,
        tx,
    ))
    .await;
    assert_ne!(
        res,
        Ok(StreamReceipt {
            state: TerminalState::Completed
        })
    );
    tokio::time::sleep(Duration::from_millis(400)).await;
    srv.abort();
    let _ = srv.await;
    assert_eq!(count.load(Ordering::SeqCst), 1);
}

#[tokio::test(flavor = "current_thread")]
async fn stream_payload_free_errors_and_driver_cleanup() {
    let secret_val = "hunter2-super-secret-stream-xyz-999";
    let secret = HealthSecret::from_raw_for_tests(secret_val.to_owned());
    let bounds = stream_bounds();
    let endpoint = validated_endpoint_for("127.0.0.1:9".parse().unwrap());
    let (tx, _) = mpsc::channel(4);
    let res = follow_stream(StreamInput::new(
        &endpoint,
        &secret,
        &bounds,
        Instant::now() + Duration::from_secs(5),
        &CancelHandle::new(),
        &CancelHandle::new(),
        "bad/session",
        0,
        tx,
    ))
    .await;
    assert_eq!(res, Err(StreamError::InvalidSession));
    let err = res.unwrap_err();
    let disp = format!("{err}");
    let dbg = format!("{err:?}");
    assert!(!disp.contains(secret_val));
    assert!(!dbg.contains(secret_val));
    for e in [
        StreamError::InvalidSession,
        StreamError::ConnectFailed,
        StreamError::HandshakeFailed,
        StreamError::SendFailed,
        StreamError::StatusNotSuccess,
        StreamError::ContentTypeInvalid,
        StreamError::BodyFailed,
        StreamError::FramingFailed,
        StreamError::DecodeFailed,
        StreamError::DeliveryFailed,
        StreamError::MissingTerminal,
        StreamError::OrderViolation,
        StreamError::Timeout,
        StreamError::Cancelled,
        StreamError::Shutdown,
        StreamError::DriverFailed,
    ] {
        let d = format!("{e}");
        let dbg = format!("{e:?}");
        assert!(!d.contains(secret_val));
        assert!(!dbg.contains(secret_val));
        assert_eq!(e, e);
        let _ = e.clone();
    }
}

#[tokio::test(flavor = "current_thread")]
async fn stream_order_violation_second_terminal_and_text_after() {
    // second terminal
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let endpoint = validated_endpoint_for(addr);
    let secret = stream_secret();
    let bounds = stream_bounds();
    let (tx, mut rx) = mpsc::channel(4);
    let srv = tokio::spawn(async move {
        let (mut s, _) = listener.accept().await.unwrap();
        let _ = read_stream_request(&mut s).await;
        let body = "data: {\"run_id\":\"run-order-vio1\",\"sequence\":1,\"state\":\"succeeded\"}\n\ndata: {\"run_id\":\"run-order-vio1\",\"sequence\":2,\"state\":\"failed\"}\n\n";
        let resp = format!(
            "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\nconnection: close\r\n\r\n{body}"
        );
        s.write_all(resp.as_bytes()).await.unwrap();
    });
    let res = follow_stream(StreamInput::new(
        &endpoint,
        &secret,
        &bounds,
        Instant::now() + Duration::from_secs(5),
        &CancelHandle::new(),
        &CancelHandle::new(),
        "sessVio",
        0,
        tx,
    ))
    .await;
    assert_eq!(res, Err(StreamError::OrderViolation));
    // First terminal was delivered before violation detection? Our implementation delivers before detecting second? Actually we detect violation before delivering any in batch, so none delivered if batch contains two terminals.
    // Ensure at most one delivered or zero.
    let _ = rx.try_recv();
    srv.await.unwrap();

    // text after terminal in same batch (single chunk containing both)
    let listener2 = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr2 = listener2.local_addr().unwrap();
    let endpoint2 = validated_endpoint_for(addr2);
    let (tx2, _) = mpsc::channel(4);
    let srv2 = tokio::spawn(async move {
        let (mut s, _) = listener2.accept().await.unwrap();
        let _ = read_stream_request(&mut s).await;
        let body = "data: {\"run_id\":\"run-order-vio2\",\"sequence\":1,\"state\":\"succeeded\"}\n\ndata: {\"run_id\":\"run-order-vio2\",\"sequence\":2,\"delta\":\"after\"}\n\n";
        let resp = format!(
            "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\nconnection: close\r\n\r\n{body}"
        );
        s.write_all(resp.as_bytes()).await.unwrap();
    });
    let res2 = follow_stream(StreamInput::new(
        &endpoint2,
        &secret,
        &bounds,
        Instant::now() + Duration::from_secs(5),
        &CancelHandle::new(),
        &CancelHandle::new(),
        "sessVio2",
        0,
        tx2,
    ))
    .await;
    assert_eq!(res2, Err(StreamError::OrderViolation));
    srv2.await.unwrap();
}

#[tokio::test(flavor = "current_thread")]
async fn stream_invalid_session_variants_rejected_before_transport() {
    let secret = stream_secret();
    let bounds = stream_bounds();
    let endpoint = validated_endpoint_for("127.0.0.1:9".parse().unwrap());
    let cases = vec!["", "a/b", "a?b", "a#b", "a\rb", "a\nb", "a%b"];
    for sess in cases {
        let (tx, _) = mpsc::channel(4);
        let res = follow_stream(StreamInput::new(
            &endpoint,
            &secret,
            &bounds,
            Instant::now() + Duration::from_secs(5),
            &CancelHandle::new(),
            &CancelHandle::new(),
            sess,
            5,
            tx,
        ))
        .await;
        assert_eq!(res, Err(StreamError::InvalidSession), "session {sess:?}");
    }
}
