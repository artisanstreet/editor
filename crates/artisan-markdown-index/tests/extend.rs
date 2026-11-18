use artisan_markdown_index::MarkdownIndex;

const REPLIES: &[&str] = &[
    "# Plan\n\nFirst we read the code.\n\nThen we *change* it, carefully.\nStill the same paragraph.\n\nTitle\n=====\n\n## Steps\n\nDone.\n",
    "Intro paragraph.\n\n- one\n- two\n\n  continued item\n- three\n\nAfter the list.\n\n1. first\n2. second\n\nClosing words.\n",
    "Before.\n\n```rust\nfn main() {\n\n    println!(\"hi\");\n}\n```\n\nBetween.\n\n```\nunclosed fence\n\nstill code\n",
    "Text.\n\n> quoted\n>\n> more quote\n\nplain\n\n| a | b |\n| --- | --- |\n| 1 | 2 |\n\nafter table\n\n    indented code\n\n    more code\n\nend\n",
    "Para.\n\n<div>\nhtml block\n\nstill html?\n</div>\n\nNext para with <b>inline</b> html.\n\n---\n\n* [ ] task\n* [x] done\n\nfin\n",
    "See [the docs][docs] and [more].\n\nAnother paragraph.\n\n[docs]: https://example.com\n[more]: https://example.com/more\n\nTail.\n",
    "Line one\nline two\n\n\n\nAfter many blank lines.\n\nSetext attempt\n\n===\n\nTrailing   \n\n\tTabbed\n",
];

fn records(index: &MarkdownIndex) -> Vec<serde_json::Value> {
    (0..index.leaves().len())
        .map(|ordinal| {
            let mut record =
                serde_json::to_value(index.record(ordinal).expect("a leaf has a record"))
                    .expect("a record serializes");
            record["leaf"]
                .as_object_mut()
                .expect("a leaf serializes as an object")
                .remove("context")
                .expect("a leaf names its interned context");
            record
        })
        .collect()
}

#[test]
fn extending_a_streamed_reply_indexes_it_exactly_as_a_full_build() {
    let mut reused = 0;
    for reply in REPLIES {
        let mut previous = String::new();
        let mut index = MarkdownIndex::build("").expect("index");
        for end in (1..=reply.len()).filter(|end| reply.is_char_boundary(*end)) {
            let source = reply.get(..end).expect("a char boundary");
            let (extended, kept) = index.extend(&previous, source).expect("index");
            let full = MarkdownIndex::build(source).expect("index");
            let (extended_records, full_records) = (records(&extended), records(&full));
            assert_eq!(extended_records.len(), full_records.len(), "{source:?}");
            for (ordinal, (left, right)) in extended_records.iter().zip(&full_records).enumerate() {
                assert_eq!(
                    left, right,
                    "leaf {ordinal} of {source:?} matches a full build"
                );
            }
            reused += kept;
            index = extended;
            previous = source.to_owned();
        }
    }
    assert!(reused > 0, "streaming reuses the settled leaves");
}

#[test]
fn a_rewritten_reply_is_indexed_from_scratch() {
    let (index, _) = MarkdownIndex::build("First.\n\nSecond.\n")
        .expect("index")
        .extend("First.\n\nSecond.\n", "Other.\n\nSecond.\n")
        .expect("index")
        .0
        .extend("Other.\n\nSecond.\n", "Other.\n\nSecond.\n\nThird")
        .expect("index");
    assert_eq!(
        records(&index),
        records(&MarkdownIndex::build("Other.\n\nSecond.\n\nThird").expect("index"))
    );
}
