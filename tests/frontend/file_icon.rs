//! Focused parity coverage for native file-icon resolution.
//!
//! The shared frontend module/export registration is VP-owned, so this packet
//! includes the owned production file directly to keep the focused test
//! independently runnable while that registration remains pending.

#[path = "../../modules/frontend/src/file_icon.rs"]
mod file_icon;

use file_icon::{FileIcon, resolve_file_icon};

#[test]
fn semantic_keys_map_to_the_vendored_file_icon_assets() {
    let cases = [
        (FileIcon::Text, "text", "jetbrains.text"),
        (
            FileIcon::TypeScriptTest,
            "typescript-test",
            "jetbrains.ts-test",
        ),
        (FileIcon::TypeScript, "typescript", "jetbrains.typescript"),
        (FileIcon::Svelte, "svelte", "jetbrains.svelte"),
    ];

    assert_eq!(FileIcon::default(), FileIcon::Text);
    assert_eq!(FileIcon::ALL, cases.map(|(icon, _, _)| icon));
    for (icon, key, asset_id) in cases {
        assert_eq!(icon.key(), key);
        assert_eq!(icon.asset_id(), asset_id);
    }
}

#[test]
fn basename_supports_forward_backward_and_mixed_slashes() {
    let cases = [
        ("src/components/view.svelte", FileIcon::Svelte),
        (r"src\components\view.svelte", FileIcon::Svelte),
        (r"src\components/view.svelte", FileIcon::Svelte),
        ("/view.ts", FileIcon::TypeScript),
        (r"\view.ts", FileIcon::TypeScript),
        ("src//nested\\view.spec.ts", FileIcon::TypeScriptTest),
    ];

    for (path, expected) in cases {
        assert_eq!(resolve_file_icon(path), expected, "path: {path}");
    }
}

#[test]
fn suffix_matching_is_case_insensitive() {
    let cases = [
        ("COMPONENT.SVELTE", FileIcon::Svelte),
        ("src/Widget.TS", FileIcon::TypeScript),
        (r"src\Widget.TeSt.Ts", FileIcon::TypeScriptTest),
        ("src/widget.SPEC.ts", FileIcon::TypeScriptTest),
    ];

    for (path, expected) in cases {
        assert_eq!(resolve_file_icon(path), expected, "path: {path}");
    }
}

#[test]
fn longest_suffix_has_precedence_over_the_typescript_fallback() {
    let cases = [
        ("unit.test.ts", FileIcon::TypeScriptTest),
        ("unit.spec.ts", FileIcon::TypeScriptTest),
        ("unit.TEST.TS", FileIcon::TypeScriptTest),
        ("unit.SPEC.TS", FileIcon::TypeScriptTest),
        ("unit.ts", FileIcon::TypeScript),
        ("unit.tsx", FileIcon::Text),
    ];

    for (path, expected) in cases {
        assert_eq!(resolve_file_icon(path), expected, "path: {path}");
    }
}

#[test]
fn reached_fallback_associations_cover_each_known_icon() {
    assert_eq!(resolve_file_icon("component.svelte"), FileIcon::Svelte);
    assert_eq!(resolve_file_icon("module.ts"), FileIcon::TypeScript);
    assert_eq!(
        resolve_file_icon("module.test.ts"),
        FileIcon::TypeScriptTest
    );
    assert_eq!(
        resolve_file_icon("module.spec.ts"),
        FileIcon::TypeScriptTest
    );
}

#[test]
fn directory_dots_do_not_determine_the_basename_icon() {
    let cases = [
        ("src.ts/main", FileIcon::Text),
        ("src.test.ts/readme", FileIcon::Text),
        (r"folder.svelte\README", FileIcon::Text),
        (r"folder.test.ts\README.md", FileIcon::Text),
        (r"folder.svelte\main.ts", FileIcon::TypeScript),
    ];

    for (path, expected) in cases {
        assert_eq!(resolve_file_icon(path), expected, "path: {path}");
    }
}

#[test]
fn unknown_and_no_extension_paths_use_the_text_fallback() {
    let paths = [
        "README",
        "README.md",
        "main.tsx",
        "main.test.js",
        "file.",
        "src/file.ts/README",
    ];

    for path in paths {
        assert_eq!(resolve_file_icon(path), FileIcon::Text, "path: {path}");
    }
}

#[test]
fn empty_and_separator_edge_cases_match_basename_extraction() {
    let cases = [
        ("", FileIcon::Text),
        ("/", FileIcon::Text),
        (r"\", FileIcon::Text),
        ("folder/", FileIcon::Text),
        (r"folder\", FileIcon::Text),
        (".ts", FileIcon::TypeScript),
        (".svelte", FileIcon::Svelte),
        ("/nested/.test.ts", FileIcon::TypeScriptTest),
        (r"\nested\file.ts", FileIcon::TypeScript),
    ];

    for (path, expected) in cases {
        assert_eq!(resolve_file_icon(path), expected, "path: {path}");
    }
}
