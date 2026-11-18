use super::*;

#[test]
fn overflow_retains_the_request_and_latest_evidence_at_utf8_boundaries() {
    let transcript = Transcript {
        entries: vec![
            Entry {
                user: true,
                text: "[User]: finish the parser\n".into(),
            },
            Entry {
                user: false,
                text: "🦀".repeat(20_000),
            },
            Entry {
                user: false,
                text: "[Tool result]: newest tests passed\n".into(),
            },
        ],
        omitted: false,
    };
    let text = serialized(&transcript, 2000);
    assert!(text.len() <= 2000);
    assert!(text.starts_with("[User]: finish the parser"));
    assert!(text.ends_with("[Tool result]: newest tests passed\n"));
    assert!(text.contains("[entry truncated]"));
    assert!(recent(&transcript.entries).is_empty());
}
