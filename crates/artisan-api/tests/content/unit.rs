use super::*;
#[test]
fn legacy_inline_text_round_trips_and_refs_are_never_text() {
    let inline: TextContent = serde_json::from_str("\"a β\"").unwrap();
    assert_eq!(inline.inline(), Some("a β"));
    let reference = TextContent::Stored(ContentRef {
        id: ContentId("a".repeat(64)),
        bytes: 9_000_000,
        lines: 1,
    });
    assert_eq!(reference.inline(), None);
    assert_eq!(reference.byte_len(), 9_000_000);
    assert_eq!(
        serde_json::from_str::<TextContent>(&serde_json::to_string(&reference).unwrap()).unwrap(),
        reference
    );
}
