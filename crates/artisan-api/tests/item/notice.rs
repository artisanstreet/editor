use super::NoticeKind;

#[test]
fn a_note_stored_with_a_title_still_reads() {
    let stored = r#"{"type":"you_should_know","heads_up":true,"title":"Orders cascade"}"#;
    assert_eq!(
        serde_json::from_str::<NoticeKind>(stored).unwrap(),
        NoticeKind::YouShouldKnow { heads_up: true }
    );
}
