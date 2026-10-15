//! Code and repository kinds: parse, required fields, diff
//! parsing, and file-tree derivation.

use super::super::{Prepared, RichFence, prepare};
use super::code_lines;
use super::file_tree::{FileTree, RowKind};

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
fn code_lines_normalize_newlines_and_tabs() {
    assert_eq!(code_lines("a\r\n\tb\n"), ["a", "    b"]);
    assert!(code_lines("").is_empty());
}

#[test]
fn code_ref_prepares_reference_and_copies_original_excerpt() {
    let prepared = ready(
        "code-ref",
        r#"{"ref": "src/lib.rs:42-44", "language": "rust", "excerpt": "fn a() {}\n", "note": "Entry point"}"#,
    );
    assert_eq!(prepared.copy().as_deref(), Some("fn a() {}\n"));
    assert!(invalid("code-ref", r#"{"excerpt": "x"}"#).contains("ref"));
}

#[test]
fn annotated_code_prepares_notes_and_copies_code() {
    let json = r#"{"code": "a\nb\nc\nd", "path": "src/x.rs", "start_line": 10, "language": "rust",
        "notes": [{"lines": "11-12", "text": "Middle"}, {"lines": 2, "text": "Relative"}, {"lines": 99, "text": "Far"}]}"#;
    let prepared = ready("annotated-code", json);
    assert_eq!(prepared.copy().as_deref(), Some("a\nb\nc\nd"));
    assert!(
        invalid(
            "annotated-code",
            r#"{"code":"x","notes":[{"lines":"a-b","text":"t"}]}"#
        )
        .contains("line")
    );
    assert!(invalid("annotated-code", r#"{"notes": []}"#).contains("code"));
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
    assert!(!rows.is_empty());
    assert!(
        rows.iter()
            .any(|(_, name, directory)| *name == "README.md" && !directory)
    );
    assert!(rows.iter().all(|(_, name, _)| !name.trim().is_empty()));
    match &tree.rows()[2].kind {
        RowKind::Dir { key, files } => {
            assert_eq!((key.as_ref(), *files), ("modules/ui/src/", 3));
        }
        RowKind::File { .. } => panic!("expected a directory"),
    }
    let _markdown = ready("file-tree", json);
    assert!(invalid("file-tree", r#"{"entries": [{"path": ""}]}"#).contains("no file"));
    assert!(invalid("file-tree", "{}").contains("entries"));
}
