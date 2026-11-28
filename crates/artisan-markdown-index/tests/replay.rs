use artisan_markdown_index::{LeafRecord, MarkdownIndex, TableAlignment};
use pulldown_cmark::{Event, Tag};

#[test]
fn serialized_leaf_replays_original_reference_resolution_and_entities() {
    let text = "First &amp; [target][later].\n\nOther.\n\n[later]: https://example.com\n";
    let index = MarkdownIndex::build(text).unwrap();
    let encoded = serde_json::to_string(&index.record(0).unwrap()).unwrap();
    let leaf: LeafRecord = serde_json::from_str(&encoded).unwrap();
    let range = leaf.leaf.source_range();
    let source = text
        .get(range.clone())
        .expect("the leaf range lies on character boundaries");
    let events = leaf.replay(range.start, source).unwrap();
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
        let Event::Text(text) = &events[0].0 else {
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

#[test]
fn exact_table_membership_survives_sparse_record_replay() {
    let index = MarkdownIndex::build(
        "| A | B |\n| --- | --- |\n| one | two |\n| three | four |\n\nafter\n",
    )
    .unwrap();
    assert!(index.record(0).unwrap().table_continues);
    assert!(index.record(1).unwrap().table_continues);
    assert!(!index.record(2).unwrap().table_continues);
    assert!(!index.record(3).unwrap().table_continues);
    let last = index.record(2).unwrap();
    let wire = serde_json::to_string(&last).unwrap();
    let decoded: LeafRecord = serde_json::from_str(&wire).unwrap();
    assert_eq!(decoded.context, last.context);
    assert!(!decoded.table_continues);
}

fn assert_balanced(events: &[(Event<'_>, std::ops::Range<usize>)]) {
    let mut open = Vec::new();
    for (event, _) in events {
        match event {
            Event::Start(tag) => open.push(tag.to_end()),
            Event::End(end) => {
                assert_eq!(
                    open.pop(),
                    Some(*end),
                    "closing tag {end:?} did not match last opened tag"
                );
            }
            Event::Text(_)
            | Event::Code(_)
            | Event::InlineMath(_)
            | Event::DisplayMath(_)
            | Event::Html(_)
            | Event::InlineHtml(_)
            | Event::FootnoteReference(_)
            | Event::SoftBreak
            | Event::HardBreak
            | Event::Rule
            | Event::TaskListMarker(_) => {}
        }
    }
    assert!(open.is_empty(), "unclosed tags: {open:?}");
}

#[test]
fn tight_list_item_followed_by_blockquote_table_or_rule_stays_balanced() {
    let source = concat!(
        "1. Outer\n",
        "   - Intro:\n",
        "     > Quoted line\n",
        "   - Table:\n",
        "     | A | B |\n",
        "     | --- | --- |\n",
        "     | 1 | 2 |\n",
        "   - Before rule\n",
        "     ***\n",
        "     After rule\n",
    );
    let index = MarkdownIndex::build(source).unwrap();
    for ordinal in 0..index.leaves().len() {
        let record = index.record(ordinal).unwrap();
        let events = record.replay(0, source).unwrap();
        assert_balanced(&events);
    }
    let quoted = (0..index.leaves().len())
        .map(|ordinal| index.record(ordinal).unwrap())
        .find(|record| {
            record
                .exact_source(0, source)
                .unwrap()
                .contains("Quoted line")
        })
        .expect("blockquote leaf exists");
    assert_eq!(quoted.context.quote_depth, 1);
    assert_eq!(quoted.context.lists.len(), 2);
    assert!(!quoted.context.lists[1].first);

    let table_header = (0..index.leaves().len())
        .map(|ordinal| index.record(ordinal).unwrap())
        .find(|record| record.is_table_header() == Some(true))
        .expect("table header leaf exists");
    assert_eq!(table_header.context.table_alignments.len(), 2);
    assert!(table_header.context.table_source.is_some());

    let prefix = "### Two Ways\n\n1. **First step**\n   - Intro copy:\n";
    let full = concat!(
        "### Two Ways\n\n",
        "1. **First step**\n",
        "   - Intro copy:\n",
        "     > **Help improve Artisan Editor**\n",
        "     > Zero code ever leaves your machine.\n",
        "   - Bonus item.\n",
    );
    let (streamed, _) = MarkdownIndex::build(prefix).unwrap().append(full).unwrap();
    for ordinal in 0..streamed.leaves().len() {
        let events = streamed.record(ordinal).unwrap().replay(0, full).unwrap();
        assert_balanced(&events);
    }
}
