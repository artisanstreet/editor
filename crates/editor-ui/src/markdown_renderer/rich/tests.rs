//! Core contract: recognition, streaming, failure, envelope, and the kit.
//!
//! Each family keeps its own kind tests beside its files; these cover what
//! every kind shares.

use super::kit::{CodeRef, format_number, with_unit};
use super::{RichFence, prepare};

fn ready(kind: &str, json: &str) -> super::Prepared {
    match prepare(Some(&format!("artisan-{kind}")), json, true) {
        Some(RichFence::Ready(prepared)) => prepared,
        other => panic!("expected artisan-{kind} to parse, got {other:?}"),
    }
}

fn note(kind: &str, json: &str) -> String {
    match prepare(Some(&format!("artisan-{kind}")), json, true) {
        Some(RichFence::Invalid { note }) => note.to_string(),
        other => panic!("expected artisan-{kind} to be invalid, got {other:?}"),
    }
}

#[test]
fn other_fences_are_not_rich() {
    assert!(prepare(Some("rust"), "fn main() {}", true).is_none());
    assert!(prepare(None, "{}", true).is_none());
    assert!(prepare(Some("artisan-"), "{}", true).is_none());
}

#[test]
fn an_open_fence_waits_with_its_kind() {
    match prepare(Some("artisan-before-after"), "{\"bef", false) {
        Some(RichFence::Pending { kind }) => assert_eq!(kind.as_ref(), "before after"),
        other => panic!("expected pending, got {other:?}"),
    }
}

#[test]
fn bad_json_unknown_kinds_and_bad_fields_name_the_problem() {
    assert!(note("callout", "{ nope").contains("does not parse"));
    assert!(note("no-such-kind", "{}").contains("not a rich block kind"));
    assert!(note("callout", "[1, 2]").contains("JSON object"));
    assert!(note("callout", "{\"kind\": \"note\"}").contains("body"));
}

#[test]
fn the_envelope_frames_the_markdown_rendition() {
    let prepared = ready(
        "callout",
        r#"{"kind": "risk", "title": "Breaking", "body": "One.\n\nTwo.", "caption": "From the PR"}"#,
    );
    let markdown = prepared.markdown();
    // The callout draws its own title, so the envelope does not repeat it.
    assert!(markdown.starts_with("> **Risk:** Breaking"), "{markdown}");
    assert!(markdown.contains("> One."), "{markdown}");
    assert!(markdown.ends_with("_From the PR_"), "{markdown}");
}

#[test]
fn code_refs_parse_paths_lines_and_ranges() {
    let range = CodeRef::parse("src/a.rs:42-60");
    assert_eq!(
        (range.path.as_ref(), range.start, range.end),
        ("src/a.rs", Some(42), Some(60))
    );
    let line = CodeRef::parse("src/a.rs:7");
    assert_eq!((line.start, line.end), (Some(7), None));
    let path = CodeRef::parse("C:/x/y.rs");
    assert_eq!((path.path.as_ref(), path.start), ("C:/x/y.rs", None));
    assert_eq!(range.display(), "src/a.rs:42-60");
}

#[test]
fn numbers_read_compactly() {
    assert_eq!(format_number(85_000.0), "85k");
    assert_eq!(format_number(24_000_000.0), "24M");
    assert_eq!(format_number(27.0), "27");
    assert_eq!(format_number(0.126), "0.13");
    assert_eq!(format_number(1234.0), "1234");
    assert_eq!(with_unit("18", Some("%")), "18%");
    assert_eq!(with_unit("41", Some("s")), "41 s");
}

/// The `artisan-editor` skill teaches agents the catalog by example; every
/// example in it must draw, or the skill teaches a block that fails.
#[test]
fn every_skill_catalog_example_draws() {
    let catalog = include_str!("../../../../../skills/artisan-editor/references/blocks.md");
    let mut examples = 0;
    let mut failures = Vec::new();
    let mut lines = catalog.lines();
    while let Some(line) = lines.next() {
        let Some(language) = line
            .strip_prefix("```")
            .filter(|tag| tag.starts_with("artisan-"))
        else {
            continue;
        };
        let body = lines
            .by_ref()
            .take_while(|line| !line.starts_with("```"))
            .collect::<Vec<_>>()
            .join("\n");
        examples += 1;
        match prepare(Some(language), &body, true) {
            Some(RichFence::Ready(_)) => {}
            Some(RichFence::Invalid { note }) => failures.push(note.to_string()),
            other => failures.push(format!("{language}: {other:?}")),
        }
    }
    assert!(examples >= 13, "found only {examples} examples");
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
