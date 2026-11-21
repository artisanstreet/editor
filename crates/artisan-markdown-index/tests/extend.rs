use artisan_markdown_index::MarkdownIndex;

const REPLIES: &[&str] = &[
    "# Plan\n\nFirst we read the code.\n\nThen we *change* it, carefully.\nStill the same paragraph.\n\nTitle\n=====\n\n## Steps\n\nDone.\n",
    "Intro paragraph.\n\n- one\n- two\n\n  continued item\n- three\n\nAfter the list.\n\n1. first\n2. second\n\nClosing words.\n",
    "Before.\n\n```rust\nfn main() {\n\n    println!(\"hi\");\n}\n```\n\nBetween.\n\n```\nunclosed fence\n\nstill code\n",
    "Text.\n\n> quoted\n>\n> more quote\n\nplain\n\n| a | b |\n| --- | --- |\n| 1 | 2 |\n\nafter table\n\n    indented code\n\n    more code\n\nend\n",
    "Para.\n\n<div>\nhtml block\n\nstill html?\n</div>\n\nNext para with <b>inline</b> html.\n\n---\n\n* [ ] task\n* [x] done\n\nfin\n",
    "See [the docs][docs] and [more].\n\nAnother paragraph.\n\n[docs]: https://example.com\n[more]: https://example.com/more\n\nTail.\n",
    "Line one\nline two\n\n\n\nAfter many blank lines.\n\nSetext attempt\n\n===\n\nTrailing   \n\n\tTabbed\n",
    "- one\n- two\n- three\n- four\n\n- loose now\n- six\n  continued\n\n  second paragraph\n- seven\n",
    "1. first\n1. second\n1. third\n7. fourth\n   - nested a\n   - nested b\n1. fifth\n\nafter\n\n3) three\n4) four\n5) five\n",
    "- [ ] task one\n- [x] task two\n- [ ] task three\n* other bullet\n* starts a list\n+ third list\n",
    "  - indented one\n  - indented two\n\n  - indented three\n-\n  empty first line\n- ```\n  code in item\n  ```\n- last\n",
    "> one\n>\n> two\n>\n> # heading\n>\n>> nested\n>> more\n>\n> back\nlazy\n\nplain\n",
    "```ts\ntype Map = { [key: string]: number };\n```\n\nUse [it] later.\n\n- [x]: not a definition\n- done\n\n[it]: https://example.com\n\nend\n",
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
        let mut index = MarkdownIndex::build("").expect("index");
        for end in (1..=reply.len()).filter(|end| reply.is_char_boundary(*end)) {
            let source = reply.get(..end).expect("a char boundary");
            let (extended, kept) = index.append(source).expect("index");
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
        }
    }
    assert!(reused > 0, "streaming reuses the settled leaves");
}

fn reparsed_share(reply: &str) -> (usize, usize) {
    let mut index = MarkdownIndex::build("").expect("index");
    let (mut reparsed, mut total) = (0, 0);
    for end in (1..=reply.len()).filter(|end| reply.is_char_boundary(*end)) {
        let source = reply.get(..end).expect("a char boundary");
        let (extended, kept) = index.append(source).expect("index");
        reparsed += extended.leaves().len() - kept;
        total += extended.leaves().len();
        index = extended;
    }
    (reparsed, total)
}

fn repeated(lines: usize, line: impl Fn(usize) -> String) -> String {
    let mut text = String::new();
    for index in 0..lines {
        text.push_str(&line(index));
    }
    text
}

#[test]
fn long_lists_quotes_and_fenced_brackets_stream_without_reparsing_from_the_start() {
    let tight = repeated(80, |item| format!("- step {item} keeps the cache warm\n"));
    let loose = repeated(80, |item| {
        format!("{}. step {item} keeps the cache warm\n\n", item + 1)
    });
    let quote = repeated(80, |line| format!("> quoted paragraph {line}\n>\n"));
    let fenced = format!(
        "```ts\ntype Map = {{ [key: string]: number }};\n```\n\n{}",
        repeated(80, |line| format!("Paragraph {line} after the fence.\n\n"))
    );
    for reply in [tight, loose, quote, fenced] {
        let (reparsed, total) = reparsed_share(&reply);
        assert!(
            reparsed * 10 < total,
            "{reparsed} of {total} leaves were reparsed while streaming {:?}",
            reply.get(..40)
        );
    }
}

const TRICKY: &[&str] = &[
    "- a\n- b\n-x\n- four\n",
    "1. a\n2. b\n3.0 is the default\n\n- c\n",
    "1. a\n2. b\n---\n3. c\n4. d\n",
    "- a\n- b\n---\n1. c\n2. d\n",
    "* a\n* b\n* * *\n1. c\n",
    "- a\n- b\n+ c\n- d\n",
    "1. a\n2. b\n3) c\n4. d\n",
    "- a\n- b\n  - c\n - d\n   - e\n",
    "1. a\n2. b\n\n3. c\n4. d\n",
    "- a\n- b\n-\n  x\n- c\n",
    "- a\n- b\n- ```\n  code\n  ```\n- c\n",
    "- a\n- b\n```\nfence\n- not an item\n```\n- c\n",
    "- a\n- b\n- c\n  ---\n- d\n",
    "- a\n- b\n- c\n===\n- d\n",
    "Para\n\n- a\n- b\n2. c\n1. d\n",
    "1. a\n1. b\n10. c\n100. d\n",
    "- a\n- b\n    - code?\n- c\n",
    "> - a\n> - b\n- c\n- d\n",
    "- a\n- b\n- | x | y |\n  | - | - |\n  | 1 | 2 |\n- c\n",
    "- [ ] a\n- [x] b\n- [ ]c\n- [ ] d\n",
];

fn assert_matches_full(index: &MarkdownIndex, source: &str) {
    let full = MarkdownIndex::build(source).expect("index");
    assert_eq!(
        records(index),
        records(&full),
        "the incremental index of {source:?} matches a full build"
    );
}

fn char_ends(text: &str, chunk: usize) -> impl Iterator<Item = usize> + '_ {
    (chunk..text.len())
        .step_by(chunk)
        .chain([text.len()])
        .filter(|end| text.is_char_boundary(*end))
}

#[test]
fn appending_tricky_lists_fences_and_rules_in_any_chunks_indexes_them_as_a_full_build() {
    for text in TRICKY {
        for split in (0..=text.len()).filter(|split| text.is_char_boundary(*split)) {
            let prefix = text.get(..split).expect("a char boundary");
            let index = MarkdownIndex::build(prefix).expect("index");
            let (index, _) = index.append(text).expect("index");
            assert_matches_full(&index, text);
        }
        for chunk in [1, 2, 3, 5, 8, 13] {
            let mut index = MarkdownIndex::build("").expect("index");
            for end in char_ends(text, chunk) {
                let source = text.get(..end).expect("a char boundary");
                index = index.append(source).expect("index").0;
                assert_matches_full(&index, source);
            }
        }
    }
}
