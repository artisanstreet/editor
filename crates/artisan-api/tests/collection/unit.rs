use super::*;
#[test]
fn inline_collections_are_plain_arrays_and_stored_ones_round_trip() {
    let records = vec!["α\n🙂".to_owned(), "second".to_owned()];
    let inline = Collection::from(records.clone());
    assert_eq!(
        serde_json::to_string(&inline).unwrap(),
        serde_json::to_string(&records).unwrap()
    );
    let stored = Collection::<String>::Stored(CollectionRef {
        source: ContentRef::from_text("\"a\"\n"),
        count: 1,
    });
    assert!(stored.inline().is_none());
    let roundtrip: Collection<String> =
        serde_json::from_str(&serde_json::to_string(&stored).unwrap()).unwrap();
    assert_eq!(roundtrip, stored);
}
