use artisan_markdown_index::{LeafRecord, MarkdownIndex, TableAlignment};
use pulldown_cmark::{Event, Tag};

#[test]
fn serialized_leaf_replays_original_reference_resolution_and_entities() {
    let text = "First &amp; [target][later].\n\nOther.\n\n[later]: https://example.com\n";
    let index = MarkdownIndex::build(text).unwrap();
    let encoded = serde_json::to_string(&index.record(0).unwrap()).unwrap();
    let leaf: LeafRecord = serde_json::from_str(&encoded).unwrap();
    let range = leaf.leaf.source_range();
    let events = leaf.replay(range.start, &text[range]).unwrap();
    assert!(events.iter().any(|(event,_)|matches!(event,Event::Start(Tag::Link{dest_url,..}) if dest_url.as_ref()=="https://example.com")));
    assert!(
        events
            .iter()
            .any(|(event, _)| matches!(event,Event::Text(text) if text.as_ref()=="&"))
    );
}

#[test]
fn portable_context_retains_lists_tasks_quotes_and_table_identity() {
    let text =
        "> 7. [x] First.\n>\n>    continuation.\n\n| left | right |\n| :--- | ---: |\n| a | b |\n";
    let index = MarkdownIndex::build(text).unwrap();
    let first: LeafRecord =
        serde_json::from_str(&serde_json::to_string(&index.record(0).unwrap()).unwrap()).unwrap();
    assert_eq!(first.context.quote_depth, 1);
    assert_eq!(first.context.lists[0].number, Some(7));
    assert_eq!(first.context.lists[0].task, Some(true));
    let last = index.record(index.leaves().len() - 1).unwrap();
    assert_eq!(
        last.context.table_alignments,
        vec![TableAlignment::Left, TableAlignment::Right]
    );
    assert!(last.context.table_source.is_some());
}

#[test]
fn literal_replay_preserves_every_byte_and_has_no_markdown_events() {
    let source = "\n**α** &amp; [x][ref]\r\n[ref]: https://example.com\n";
    let mut start = 0;
    let mut joined = String::new();
    for line in source.split_inclusive('\n').chain(std::iter::once("")) {
        let record = LeafRecord::literal(start..start + line.len());
        let decoded: LeafRecord =
            serde_json::from_str(&serde_json::to_string(&record).unwrap()).unwrap();
        let events = decoded.replay(start, line).unwrap();
        assert_eq!(events.len(), 1);
        let artisan_markdown_index::Event::Text(text) = &events[0].0 else {
            panic!("literal event")
        };
        assert_eq!(text.as_ref(), line);
        assert_eq!(events[0].1, 0..line.len());
        assert!(!decoded.table_continues);
        joined.push_str(text);
        start += line.len();
    }
    assert_eq!(joined, source);
    assert_eq!(LeafRecord::literal(0..0).replay(0, "").unwrap().len(), 1);
}
