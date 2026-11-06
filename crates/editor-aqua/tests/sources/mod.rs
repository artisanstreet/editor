use artisan_api::{CollectionRange, CollectionRef, ContentRef};
use editor_aqua::sources::{page_record, raw_rows};

#[test]
fn raw_rows_preserve_unicode_and_bound_fragments() {
    let text = format!("{}🙂\n{}", "a".repeat(1023), "β".repeat(600));
    let rows = raw_rows(&text);
    assert_eq!(
        rows.iter()
            .map(|range| &text[range.clone()])
            .collect::<String>(),
        text
    );
    assert!(rows.iter().all(|range| range.len() <= 1024));
    assert_eq!(&text[rows[1].clone()], "🙂\n");
}

#[test]
fn collection_page_rejects_stale_identity_and_outside_indices() {
    let source = CollectionRef {
        source: ContentRef::from_text("source"),
        count: 2,
    };
    let page = CollectionRange {
        source: source.source.clone(),
        start: 1,
        end: 2,
        total: 2,
        records: vec!["entry"],
    };
    assert_eq!(page_record(&source, &page, 1), Some(&"entry"));
    assert_eq!(page_record(&source, &page, 0), None);
    let stale = CollectionRef { count: 3, ..source };
    assert_eq!(page_record(&stale, &page, 1), None);
}

#[test]
fn virtualized_fragments_copy_from_the_same_loaded_source() {
    use aqua_text::document::{SelectionDocument, SelectionSourceId, SourcePosition};
    use editor_aqua::sources::{selection_source, source_slice};
    let id = SelectionSourceId(7);
    let mut document = SelectionDocument::new();
    document.register(selection_source(id, 42, "α\nβ")).unwrap();
    let first = source_slice(id, 0..3);
    let second = source_slice(id, 3..5);
    assert!(document.validate_slice(&first));
    assert!(document.validate_slice(&second));
    assert!(document.set_selection(
        SourcePosition {
            source: id,
            offset: 0
        },
        SourcePosition {
            source: id,
            offset: 5
        }
    ));
    assert_eq!(document.selected_text().as_deref(), Some("α\nβ"));
}

mod host_lifetime;
