use super::*;

fn model(code: &str) -> AnnotatedCode {
    serde_json::from_value(serde_json::json!({
        "code": code,
        "start_line": 10,
        "notes": [
            {"lines": [1, 2], "text": "first"},
            {"lines": [11, 12], "text": "second"}
        ]
    }))
    .unwrap()
}

#[test]
fn overlapping_notes_keep_first_owner_and_note_hover_precedes_line() {
    let model = model("one\ntwo\nthree\n");
    let mut state = AnnotatedState::new(&model);
    assert_eq!(model.copy(), "one\ntwo\nthree");
    assert_eq!(model.0.owner, vec![Some(0), Some(0), Some(1)]);
    assert_eq!(model.0.marker, vec![Some(0), Some(1), None]);
    assert!(state.hover(false, 1, true));
    assert_eq!(state.active(), Some(0));
    assert!(state.hover(true, 1, true));
    assert_eq!(state.active(), Some(1));
    assert!(state.hover(false, 2, true));
    assert!(!state.hover(false, 1, false));
    assert_eq!(state.active(), Some(1));
    assert!(state.hover(true, 1, false));
    assert_eq!(state.active(), Some(1));
}

#[test]
fn replacing_model_resets_hover_even_with_equal_content() {
    let first = model("one\ntwo\nthree");
    let replacement = model("one\ntwo\nthree");
    let mut state = AnnotatedState::new(&first);
    state.hover(true, 1, true);
    assert!(!state.update_model(&first));
    assert_eq!(state.active(), Some(1));
    assert!(state.update_model(&replacement));
    assert_eq!(state.active(), None);
}
