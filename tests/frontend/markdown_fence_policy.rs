//! Dependency-free parity coverage for the native Markdown fence policy.

#[path = "../../modules/frontend/src/markdown_fence_policy.rs"]
mod markdown_fence_policy;

use markdown_fence_policy::{
    ConversationFence, is_open_conversation_fence_body, open_conversation_fence_body,
    requested_conversation_fence_languages, scan_conversation_fences,
};
use std::fmt::Write as _;

fn fence(
    label: Option<&str>,
    language: Option<&str>,
    body: &str,
    closed: bool,
) -> ConversationFence {
    ConversationFence {
        label: label.map(str::to_owned),
        language: language.map(str::to_owned),
        body: body.to_owned(),
        closed,
    }
}

#[test]
fn every_legacy_alias_resolves_and_canonical_grammars_keep_first_seen_order() {
    let aliases = [
        ("astro", "astro"),
        ("bash", "bash"),
        ("c", "c"),
        ("c#", "csharp"),
        ("c++", "cpp"),
        ("cpp", "cpp"),
        ("csharp", "csharp"),
        ("cs", "csharp"),
        ("css", "css"),
        ("cxx", "cpp"),
        ("go", "go"),
        ("golang", "go"),
        ("htm", "html"),
        ("html", "html"),
        ("java", "java"),
        ("javascript", "javascript"),
        ("js", "javascript"),
        ("jsx", "jsx"),
        ("markdown", "markdown"),
        ("md", "markdown"),
        ("mjs", "javascript"),
        ("ps", "powershell"),
        ("ps1", "powershell"),
        ("powershell", "powershell"),
        ("py", "python"),
        ("python", "python"),
        ("rs", "rust"),
        ("rust", "rust"),
        ("sh", "bash"),
        ("shell", "bash"),
        ("sql", "sql"),
        ("svelte", "svelte"),
        ("toml", "toml"),
        ("ts", "typescript"),
        ("tsx", "tsx"),
        ("typescript", "typescript"),
        ("vue", "vue"),
        ("xhtml", "xml"),
        ("xml", "xml"),
        ("yaml", "yaml"),
        ("yml", "yaml"),
        ("json", "json"),
    ];
    let mut markdown = String::new();
    for (alias, _) in aliases {
        writeln!(markdown, "```{alias}").expect("writing to a String cannot fail");
        markdown.push_str("```\n");
    }
    let mut expected = Vec::new();
    for (_, grammar) in aliases {
        if !expected.iter().any(|known| known == grammar) {
            expected.push(grammar.to_owned());
        }
    }

    assert_eq!(requested_conversation_fence_languages(&markdown), expected);
}

#[test]
fn requested_languages_are_admitted_from_opener_like_lines_only() {
    let markdown = concat!(
        "paragraph ```rust\n",
        "    ```python\n",
        "\t```go\n",
        "  ~~~PY[example.py]\n",
        "~~~javascript{filename=app.js}\n",
        "```rust`malformed-info\n",
        "~~~unknown\n",
        "~~~{filename=missing-label}\n",
        "~~~\n",
    );

    assert_eq!(
        requested_conversation_fence_languages(markdown),
        ["python", "javascript", "rust"]
            .into_iter()
            .map(str::to_owned)
            .collect::<Vec<_>>()
    );
}

#[test]
fn requested_language_whitespace_matches_legacy_javascript_boundaries() {
    let markdown = concat!(
        "```rust\u{001c}[main.rs]\n",
        "```\n",
        "~~~rust\u{0085}[other.py]\n",
        "~~~\n",
    );

    assert!(requested_conversation_fence_languages(markdown).is_empty());
    assert_eq!(
        scan_conversation_fences(markdown),
        vec![
            fence(Some("rust\u{001c}"), None, "", true),
            fence(Some("rust\u{0085}"), None, "", true),
        ]
    );
}

#[test]
fn scan_preserves_labels_metadata_and_newline_terminated_bodies() {
    let markdown = concat!(
        "before\n",
        "  ```RUST[main.rs]{fold}\n",
        "let answer = 42;\n",
        "nested ``` marker text\n",
        "  ```  \t\n",
        "after\n",
        "~~~PY {filename=example.py}\n",
        "print('ok')",
    );

    assert_eq!(
        scan_conversation_fences(markdown),
        vec![
            fence(
                Some("rust"),
                Some("rust"),
                "let answer = 42;\nnested ``` marker text\n",
                true,
            ),
            fence(Some("py"), Some("python"), "print('ok')\n", false),
        ]
    );
}

#[test]
fn unknown_and_empty_info_labels_are_retained_without_a_grammar() {
    let markdown = concat!(
        "```\n",
        "empty info\n",
        "```\n",
        "~~~totally-unknown\n",
        "unknown body\n",
        "~~~\n",
        "```[filename-only]\n",
        "filename body\n",
        "```\n",
        "~~~{metadata-only}\n",
        "meta body\n",
        "~~~\n",
    );

    assert_eq!(
        scan_conversation_fences(markdown),
        vec![
            fence(None, None, "empty info\n", true),
            fence(Some("totally-unknown"), None, "unknown body\n", true),
            fence(None, None, "filename body\n", true),
            fence(None, None, "meta body\n", true),
        ]
    );
}

#[test]
fn openers_require_three_markers_and_no_more_than_three_leading_spaces() {
    let markdown = concat!(
        "``rust\n",
        "not a fence\n",
        "    ```python\n",
        "also not a fence\n",
        "   ~~~go\n",
        "body\n",
        "   ~~~\n",
    );

    assert_eq!(
        scan_conversation_fences(markdown),
        vec![fence(Some("go"), Some("go"), "body\n", true)]
    );
}

#[test]
fn backtick_info_is_invalid_but_tilde_info_may_contain_backticks() {
    let markdown = concat!(
        "~~~rust`allowed\n",
        "tilde body\n",
        "~~~\n",
        "```rust`invalid\n",
    );

    assert_eq!(
        scan_conversation_fences(markdown),
        vec![fence(Some("rust`allowed"), None, "tilde body\n", true)]
    );
}

#[test]
fn closing_runs_match_marker_kind_length_indent_and_trailing_whitespace() {
    let markdown = concat!(
        "````rust\n",
        "short\n",
        "```\n",
        "opposite ~~~~\n",
        "four-space     \n",
        "~~~~\n",
        "still open\n",
        "  ```` \t\n",
        "~~~go\n",
        "wrong ```\n",
        "~~\n",
        "wrong suffix text\n",
        "~~~\n",
    );

    assert_eq!(
        scan_conversation_fences(markdown),
        vec![
            fence(
                Some("rust"),
                Some("rust"),
                "short\n```\nopposite ~~~~\nfour-space     \n~~~~\nstill open\n",
                true,
            ),
            fence(
                Some("go"),
                Some("go"),
                "wrong ```\n~~\nwrong suffix text\n",
                true,
            ),
        ]
    );
}

#[test]
fn crlf_syntax_is_recognized_while_body_carriage_returns_stay_exact() {
    let markdown = "```rs\r\nlet x = 1;\r\n```\r\n~~~py\r\nprint(x)\r";

    assert_eq!(
        scan_conversation_fences(markdown),
        vec![
            fence(Some("rs"), Some("rust"), "let x = 1;\r\n", true),
            fence(Some("py"), Some("python"), "print(x)\r\n", false),
        ]
    );
    assert_eq!(
        open_conversation_fence_body(markdown),
        Some("print(x)\r".to_owned())
    );
}

#[test]
fn multiple_fences_and_first_open_body_follow_streaming_transitions() {
    let closed = "```rust\nlet x = 1;\n```\n";
    let open = format!("{closed}```rust\nlet x = 2;");
    let completed = format!("{open}\n```\n~~~python\nprint(x)\n");

    assert_eq!(open_conversation_fence_body(closed), None);
    assert_eq!(
        open_conversation_fence_body(&open),
        Some("let x = 2;".to_owned())
    );
    assert_eq!(
        open_conversation_fence_body(&completed),
        Some("print(x)".to_owned())
    );
    assert_eq!(scan_conversation_fences(&completed).len(), 3);
}

#[test]
fn normalized_open_body_comparison_is_exact_except_for_trailing_newlines() {
    assert!(!is_open_conversation_fence_body("body", None));
    assert!(is_open_conversation_fence_body("body\n\n", Some("body")));
    assert!(!is_open_conversation_fence_body("body", Some("body\n\n")));
    assert!(!is_open_conversation_fence_body("Body", Some("body")));
    assert!(!is_open_conversation_fence_body("body ", Some("body")));
    assert!(!is_open_conversation_fence_body("body\r", Some("body")));
}
