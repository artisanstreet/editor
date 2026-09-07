//! Table-driven parity tests for the pure editor-language policy.
//!
//! The source is included directly so this focused harness stays dependency-
//! free while the frontend module and shared test registrations remain
//! controller-owned.

#[path = "../../modules/frontend/src/editor_language.rs"]
mod editor_language;

use editor_language::{EditorLanguageId, editor_language_for_path, editor_language_is_highlighted};

#[test]
fn language_ids_are_closed_and_highlighting_is_exhaustive() {
    let cases = [
        (EditorLanguageId::Css, "css", true),
        (EditorLanguageId::Go, "go", true),
        (EditorLanguageId::Html, "html", true),
        (EditorLanguageId::JavaScript, "javascript", true),
        (EditorLanguageId::Json, "json", true),
        (EditorLanguageId::Markdown, "markdown", true),
        (EditorLanguageId::Plaintext, "plaintext", false),
        (EditorLanguageId::Python, "python", true),
        (EditorLanguageId::Rust, "rust", true),
        (EditorLanguageId::Sql, "sql", true),
        (EditorLanguageId::TypeScript, "typescript", true),
        (EditorLanguageId::Xml, "xml", true),
        (EditorLanguageId::Yaml, "yaml", true),
    ];

    assert_eq!(EditorLanguageId::ALL.len(), cases.len());
    for (index, (language, expected_name, expected_highlighted)) in cases.into_iter().enumerate() {
        assert_eq!(EditorLanguageId::ALL[index], language);
        assert_eq!(language.as_str(), expected_name);
        assert_eq!(language.is_highlighted(), expected_highlighted);
        assert_eq!(
            editor_language_is_highlighted(language),
            expected_highlighted
        );
    }
}

#[test]
fn filename_table_covers_every_exact_mapping() {
    let cases = [
        (".babelrc", EditorLanguageId::Json),
        (".prettierrc", EditorLanguageId::Json),
        ("Dockerfile", EditorLanguageId::Plaintext),
        ("Makefile", EditorLanguageId::Plaintext),
    ];

    for (file_name, expected) in cases {
        assert_eq!(
            editor_language_for_path(file_name, None),
            expected,
            "{file_name}"
        );
    }
}

#[test]
fn extension_table_covers_every_exact_mapping() {
    let cases = [
        ("file.cjs", EditorLanguageId::JavaScript),
        ("file.js", EditorLanguageId::JavaScript),
        ("file.jsx", EditorLanguageId::JavaScript),
        ("file.mjs", EditorLanguageId::JavaScript),
        ("file.css", EditorLanguageId::Css),
        ("file.scss", EditorLanguageId::Css),
        ("file.go", EditorLanguageId::Go),
        ("file.htm", EditorLanguageId::Html),
        ("file.html", EditorLanguageId::Html),
        ("file.sv", EditorLanguageId::Html),
        ("file.svelte", EditorLanguageId::Html),
        ("file.json", EditorLanguageId::Json),
        ("file.jsonc", EditorLanguageId::Json),
        ("file.md", EditorLanguageId::Markdown),
        ("file.mdx", EditorLanguageId::Markdown),
        ("file.mts", EditorLanguageId::TypeScript),
        ("file.ts", EditorLanguageId::TypeScript),
        ("file.tsx", EditorLanguageId::TypeScript),
        ("file.py", EditorLanguageId::Python),
        ("file.pyi", EditorLanguageId::Python),
        ("file.rs", EditorLanguageId::Rust),
        ("file.sql", EditorLanguageId::Sql),
        ("file.xml", EditorLanguageId::Xml),
        ("file.yaml", EditorLanguageId::Yaml),
        ("file.yml", EditorLanguageId::Yaml),
    ];

    for (path, expected) in cases {
        assert_eq!(editor_language_for_path(path, None), expected, "{path}");
    }
}

#[test]
fn every_highlighted_declared_language_takes_precedence() {
    let declared = [
        ("css", EditorLanguageId::Css),
        ("go", EditorLanguageId::Go),
        ("html", EditorLanguageId::Html),
        ("javascript", EditorLanguageId::JavaScript),
        ("json", EditorLanguageId::Json),
        ("markdown", EditorLanguageId::Markdown),
        ("python", EditorLanguageId::Python),
        ("rust", EditorLanguageId::Rust),
        ("sql", EditorLanguageId::Sql),
        ("typescript", EditorLanguageId::TypeScript),
        ("xml", EditorLanguageId::Xml),
        ("yaml", EditorLanguageId::Yaml),
    ];

    for (value, expected) in declared {
        assert_eq!(
            editor_language_for_path("README.unsupported", Some(value)),
            expected,
            "declared={value}"
        );
        assert_eq!(
            editor_language_for_path("source.rs", Some(value)),
            expected,
            "declared={value}"
        );
    }
}

#[test]
fn unsupported_declared_values_fall_back_to_path_detection() {
    let cases = [
        ("source.rs", Some("plaintext"), EditorLanguageId::Rust),
        ("source.rs", Some("ruby"), EditorLanguageId::Rust),
        ("source.rs", Some("Rust"), EditorLanguageId::Rust),
        ("source.rs", Some("typescript "), EditorLanguageId::Rust),
        (
            "README.unsupported",
            Some("plaintext"),
            EditorLanguageId::Plaintext,
        ),
        (
            "README.unsupported",
            Some("unknown"),
            EditorLanguageId::Plaintext,
        ),
        ("README.unsupported", Some(""), EditorLanguageId::Plaintext),
        ("README.unsupported", None, EditorLanguageId::Plaintext),
    ];

    for (path, declared, expected) in cases {
        assert_eq!(
            editor_language_for_path(path, declared),
            expected,
            "path={path} declared={declared:?}"
        );
    }
}

#[test]
fn filename_and_extension_matching_is_case_insensitive() {
    let cases = [
        (".BABELRC", EditorLanguageId::Json),
        (".PRETTIERRC", EditorLanguageId::Json),
        ("DOCKERFILE", EditorLanguageId::Plaintext),
        ("makefile", EditorLanguageId::Plaintext),
        ("file.CJS", EditorLanguageId::JavaScript),
        ("file.SCSS", EditorLanguageId::Css),
        ("file.SVELTE", EditorLanguageId::Html),
        ("file.JSONC", EditorLanguageId::Json),
        ("file.MDX", EditorLanguageId::Markdown),
        ("file.TSX", EditorLanguageId::TypeScript),
        ("file.PYI", EditorLanguageId::Python),
        ("file.RS", EditorLanguageId::Rust),
        ("file.SQL", EditorLanguageId::Sql),
        ("file.XML", EditorLanguageId::Xml),
        ("file.YML", EditorLanguageId::Yaml),
    ];

    for (path, expected) in cases {
        assert_eq!(editor_language_for_path(path, None), expected, "{path}");
    }
}

#[test]
fn slash_and_backslash_paths_use_the_last_component() {
    let cases = [
        (r"C:\workspace\src\main.rs", EditorLanguageId::Rust),
        (
            r"C:\workspace\src\component.TSX",
            EditorLanguageId::TypeScript,
        ),
        (r"C:/workspace\src\view.SVELTE", EditorLanguageId::Html),
        (r"\\server\share\Dockerfile", EditorLanguageId::Plaintext),
        (r"C:\repo/mixed/.BABELRC", EditorLanguageId::Json),
        (r"C:/repo\nested\Makefile", EditorLanguageId::Plaintext),
    ];

    for (path, expected) in cases {
        assert_eq!(editor_language_for_path(path, None), expected, "{path}");
    }
}

#[test]
fn multi_dot_names_and_dotfiles_follow_typescript_extension_rules() {
    let cases = [
        ("component.test.tsx", EditorLanguageId::TypeScript),
        ("data.generated.jsonc", EditorLanguageId::Json),
        ("styles.module.scss", EditorLanguageId::Css),
        ("README.en.MDX", EditorLanguageId::Markdown),
        ("Dockerfile.ts", EditorLanguageId::TypeScript),
        ("foo.babelrc", EditorLanguageId::Plaintext),
        (".babelrc.json", EditorLanguageId::Plaintext),
        (".hidden.rs", EditorLanguageId::Plaintext),
        (".env.local", EditorLanguageId::Plaintext),
        (".gitignore", EditorLanguageId::Plaintext),
        // The leading-dot rule makes the remainder the extension, so `.rs`
        // itself is recognized just like the TypeScript implementation.
        (".rs", EditorLanguageId::Rust),
    ];

    for (path, expected) in cases {
        assert_eq!(editor_language_for_path(path, None), expected, "{path}");
    }
}

#[test]
fn unknown_extensionless_and_empty_paths_fall_back_to_plaintext() {
    let paths = [
        "",
        "README",
        "README.",
        "README.unknown",
        ".",
        "..",
        "C:/workspace/",
        r"C:\workspace\",
        "/",
        r"\",
    ];

    for path in paths {
        assert_eq!(
            editor_language_for_path(path, None),
            EditorLanguageId::Plaintext,
            "{path:?}"
        );
    }
}
