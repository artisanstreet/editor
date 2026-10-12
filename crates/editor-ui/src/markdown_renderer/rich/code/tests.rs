//! Code and repository kinds: parse, required fields, Markdown, diff
//! parsing, and file-tree derivation.

use super::super::{Prepared, RichFence, prepare};
use super::file_tree::{FileTree, RowKind};
use super::{code_lines, fence};

fn ready(kind: &str, json: &str) -> Prepared {
    match prepare(Some(&format!("artisan-{kind}")), json, true) {
        Some(RichFence::Ready(prepared)) => prepared,
        other => panic!("expected artisan-{kind} to parse, got {other:?}"),
    }
}

fn invalid(kind: &str, json: &str) -> String {
    match prepare(Some(&format!("artisan-{kind}")), json, true) {
        Some(RichFence::Invalid { note }) => note.to_string(),
        other => panic!("expected artisan-{kind} to be invalid, got {other:?}"),
    }
}

#[test]
fn fences_outrun_backticks_in_their_body() {
    assert_eq!(
        fence(Some("rs"), "let a = 1;\n"),
        "```rs\nlet a = 1;\n```\n"
    );
    assert!(fence(None, "a ```` b").starts_with("`````\n"));
    assert_eq!(code_lines("a\r\n\tb\n"), ["a", "    b"]);
    assert!(code_lines("").is_empty());
}

#[test]
fn code_ref_numbers_from_its_first_line() {
    let markdown = ready(
        "code-ref",
        r#"{"ref": "src/lib.rs:42-44", "language": "rust", "excerpt": "fn a() {}\n", "note": "Entry point"}"#,
    )
    .markdown();
    assert!(
        markdown.starts_with("`src/lib.rs:42-44` — Entry point\n"),
        "{markdown}"
    );
    assert!(markdown.contains("```rust\nfn a() {}\n```"), "{markdown}");
    assert!(invalid("code-ref", r#"{"excerpt": "x"}"#).contains("ref"));
}

#[test]
fn annotated_code_lists_its_notes() {
    let json = r#"{"code": "a\nb\nc\nd", "path": "src/x.rs", "start_line": 10, "language": "rust",
        "notes": [{"lines": "11-12", "text": "Middle"}, {"lines": 2, "text": "Relative"}, {"lines": 99, "text": "Far"}]}"#;
    let markdown = ready("annotated-code", json).markdown();
    assert!(markdown.starts_with("`src/x.rs:10`"), "{markdown}");
    assert!(markdown.contains("1. **L11–12** Middle"), "{markdown}");
    // A span past the excerpt that fits it counted from one is relative.
    assert!(markdown.contains("2. **L11** Relative"), "{markdown}");
    assert!(markdown.contains("3. **L99** Far"), "{markdown}");
    assert!(invalid("annotated-code", r#"{"notes": []}"#).contains("code"));
    assert!(
        invalid(
            "annotated-code",
            r#"{"code": "x", "notes": [{"lines": "a-b", "text": "t"}]}"#
        )
        .contains("line")
    );
}

#[test]
fn file_trees_derive_and_fold_directories() {
    let json = r#"{"entries": [
        {"path": "modules/ui/src/rich/code/mod.rs", "change": "added"},
        {"path": "./modules/ui/src/rich/code/diff.rs", "change": "modified", "note": "Parses patches"},
        {"path": "modules/ui/src/lib.rs", "change": "wat"},
        {"path": "docs/rich-blocks.md", "change": "deleted"},
        {"path": "README.md"},
        {"path": "  "}
    ]}"#;
    let tree: FileTree = serde_json::from_str(json).expect("tree");
    let rows: Vec<(usize, &str, bool)> = tree
        .rows()
        .iter()
        .map(|row| {
            (
                row.depth,
                row.name.as_ref(),
                matches!(row.kind, RowKind::Dir { .. }),
            )
        })
        .collect();
    assert_eq!(
        rows,
        [
            (0, "docs/", true),
            (1, "rich-blocks.md", false),
            (0, "modules/ui/src/", true),
            (1, "rich/code/", true),
            (2, "diff.rs", false),
            (2, "mod.rs", false),
            (1, "lib.rs", false),
            (0, "README.md", false),
        ]
    );
    match &tree.rows()[2].kind {
        RowKind::Dir { key, files } => {
            assert_eq!((key.as_ref(), *files), ("modules/ui/src/", 3));
        }
        RowKind::File { .. } => panic!("expected a directory"),
    }
    let markdown = ready("file-tree", json).markdown();
    assert!(markdown.contains("- **modules/ui/src/**\n  - **rich/code/**\n    - `diff.rs` (modified) — Parses patches\n"), "{markdown}");
    assert!(markdown.contains("  - `lib.rs` (changed)\n"), "{markdown}");
    assert!(invalid("file-tree", r#"{"entries": [{"path": ""}]}"#).contains("no file"));
    assert!(invalid("file-tree", "{}").contains("entries"));
}
