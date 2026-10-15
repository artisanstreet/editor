
use super::*;

#[test]
fn unloaded_reference_definition_and_decoded_text_survive_leaf_replay() {
    let source = "First **bold** &amp; [target][later].\n\nSecond paragraph.\n\n[later]: https://example.com\n";
    let index = MarkdownIndex::build(source).unwrap();
    assert_eq!(index.leaves().len(), 2);
    let range = index.leaves()[0].source_range();
    let IndexedLeafContent::Block(Block::Paragraph { spans, .. }) = index
        .normalize_leaf(
            0,
            range.start,
            &source[range],
            &MarkdownEngine::new().unwrap(),
        )
        .unwrap()
    else {
        panic!("paragraph");
    };
    assert_eq!(super::super::spans_text(&spans), "First bold & target.");
    assert!(spans.iter().any(|span|matches!(span,super::super::Span::Link { destination,.. } if destination=="https://example.com")));
}

#[test]
fn list_context_tracks_original_ordinals_tasks_and_continuation_markers() {
    let source = "7. [x] first\n\n   continuation\n\n8. second\n   - nested\n";
    let index = MarkdownIndex::build(source).unwrap();
    assert_eq!(index.leaves().len(), 4);
    let first = &index.context(0).unwrap().lists[0];
    assert_eq!(
        (first.number, first.task, first.first),
        (Some(7), Some(true), true)
    );
    assert!(!index.context(1).unwrap().lists[0].first);
    assert_eq!(index.context(2).unwrap().lists[0].number, Some(8));
    assert_eq!(index.context(3).unwrap().lists.len(), 2);
    assert!(!index.context(3).unwrap().lists[0].first);
}

#[test]
fn each_table_row_replays_without_loading_other_rows() {
    let source = "| left | right |\n| :--- | ---: |\n| **a** | b &amp; c |\n| d | e |\n";
    let index = MarkdownIndex::build(source).unwrap();
    assert_eq!(index.leaves().len(), 3);
    let range = index.leaves()[1].source_range();
    let IndexedLeafContent::TableRow {
        header,
        alignments,
        cells,
    } = index
        .normalize_leaf(
            1,
            range.start,
            &source[range],
            &MarkdownEngine::new().unwrap(),
        )
        .unwrap()
    else {
        panic!("row");
    };
    assert!(!header);
    assert_eq!(
        alignments,
        vec![TableAlignment::Left, TableAlignment::Right]
    );
    assert_eq!(super::super::spans_text(&cells[0].spans), "a");
    assert_eq!(super::super::spans_text(&cells[1].spans), "b & c");
}

#[test]
fn empty_task_items_keep_markers_and_original_item_spacing() {
    let source = "- [x]\n- [ ]\n- body\n";
    let index = MarkdownIndex::build(source).unwrap();
    assert_eq!(index.leaves().len(), 3);
    assert_eq!(index.context(0).unwrap().lists[0].task, Some(true));
    assert_eq!(index.context(1).unwrap().lists[0].task, Some(false));
    assert!(
        index
            .context(1)
            .unwrap()
            .spacing
            .contains(&LeafSpacing::Item)
    );
}
#[test]
fn source_sibling_context_keeps_heading_and_outer_list_margin_relations() {
    let source = "## Heading\n\nFollowing.\n\n- first\n- second\n\nAfter.\n";
    let index = MarkdownIndex::build(source).unwrap();
    assert_eq!(index.leaves().len(), 5);
    assert!(
        index
            .context(1)
            .unwrap()
            .spacing
            .contains(&LeafSpacing::Sibling {
                previous: Some(BlockShape::Heading(2)),
                current: BlockShape::Paragraph,
                in_item: false
            })
    );
    assert!(
        index
            .context(2)
            .unwrap()
            .spacing
            .contains(&LeafSpacing::Sibling {
                previous: Some(BlockShape::Paragraph),
                current: BlockShape::List,
                in_item: false
            })
    );
    assert_eq!(index.context(3).unwrap().spacing[0], LeafSpacing::Item);
    assert!(
        index
            .context(4)
            .unwrap()
            .spacing
            .contains(&LeafSpacing::Sibling {
                previous: Some(BlockShape::List),
                current: BlockShape::Paragraph,
                in_item: false
            })
    );
}

#[test]
fn giant_plain_paragraph_index_does_not_retain_its_text() {
    let source = "long paragraph ".repeat(100_000);
    let index = MarkdownIndex::build(&source).unwrap();
    assert_eq!(index.leaves().len(), 1);
    assert!(index.resident_bytes() < 4096);
    assert_eq!(index.leaves()[0].source_range(), 0..source.len());
    assert!(matches!(
        index.normalize_leaf(0, 1, &source[1..], &MarkdownEngine::new().unwrap()),
        Err(MarkdownIndexError::IncompleteRange)
    ));
}
