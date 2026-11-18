use super::*;
#[test]
fn first_clause_keeps_arithmetic_delimiters_from_later_prose() {
    assert_eq!(
        label("17*23: breaking it into 17*20 gives 340", true),
        "17*23"
    );
}

#[test]
fn provider_reasoning_is_bounded_canonical_replacement_text() {
    let mut summary = Summary::default();
    let events = summary.normalize(
        vec![EngineEvent::Live {
            target: LiveKind::Reasoning,
            text: "**Check ownership**\n\nI will check the run.".into(),
            append: true,
        }],
        false,
    );
    assert_eq!(
        events,
        vec![EngineEvent::Live {
            target: LiveKind::Reasoning,
            text: "Check ownership".into(),
            append: false
        }]
    );
    summary.normalize(
        vec![EngineEvent::Live {
            target: LiveKind::Reasoning,
            text: "x".repeat(100_000),
            append: true,
        }],
        false,
    );
    assert!(summary.raw.len() <= MAX_RAW_BYTES);
    assert!(summary.shown.chars().count() <= MAX_LABEL_CHARS);
    let clear = summary.normalize(
        vec![EngineEvent::Live {
            target: LiveKind::Reasoning,
            text: String::new(),
            append: false,
        }],
        false,
    );
    assert_eq!(
        clear,
        vec![EngineEvent::Live {
            target: LiveKind::Reasoning,
            text: String::new(),
            append: false
        }]
    );
}
