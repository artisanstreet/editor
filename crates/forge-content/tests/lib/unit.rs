use super::*;

#[test]
fn a_collection_is_named_by_its_exact_ndjson() {
    let records = vec!["α\n🙂".to_owned(), "second".to_owned()];
    let reference = collection_ref(&records).unwrap();
    let exact = records
        .iter()
        .map(|record| serde_json::to_string(record).unwrap() + "\n")
        .collect::<String>();
    assert_eq!(reference.source, ContentRef::from_text(&exact));
    assert_eq!(reference.count, 2);
}

#[test]
fn a_large_collection_keeps_its_count_and_drops_its_records() {
    let mut records = Collection::from(
        (0..5000)
            .map(|index| format!("record-{index}"))
            .collect::<Vec<_>>(),
    );
    assert!(compact_collection(&mut records).unwrap());
    assert_eq!(records.len(), 5000);
    assert!(records.inline().is_none());
    assert!(!compact_collection(&mut records).unwrap());
}
