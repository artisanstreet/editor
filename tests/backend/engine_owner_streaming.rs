use super::framing::{SseEof, SseFramer, SseFramerError};
use super::observation::{TerminalState, TextDelta, chunk_text};

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
    // max_line=10, line "data: hi" is 7 bytes before LF
    let mut framer = SseFramer::new(10, 100).unwrap();
    let line = b"data: hi\n\n";
    let events = framer.feed(line).unwrap();
    assert_eq!(events.len(), 1);
    // Exactly at cap: build line of length 10
    let mut framer2 = SseFramer::new(10, 100).unwrap();
    let payload = b"0123456789"; // 10 bytes
    let mut buf = Vec::new();
    buf.extend_from_slice(payload);
    buf.push(b'\n');
    buf.extend_from_slice(b"\n");
    // First line is 10 bytes without LF -> exactly at cap
    // But it has no valid field, will be ignored then blank dispatch? Let's use data field.
    // Need a valid data line of exactly 10 bytes: "data: 1234" is 10
    let mut framer3 = SseFramer::new(10, 100).unwrap();
    let events3 = framer3.feed(b"data: 1234\n\n").unwrap();
    assert_eq!(events3.len(), 1);
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
