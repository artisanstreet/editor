//! Pure editor-language identification for native editor callers.
//!
//! This is the dependency-free Rust counterpart of
//! `modules/frontend/src/lib/editor/language.ts`'s identification policy. It
//! deliberately stops before CodeMirror grammar loading: callers receive a
//! closed language identifier and can decide how to render or highlight it.

#![allow(clippy::module_name_repetitions)]

/// A language identifier understood by the editor.
///
/// The set is intentionally closed and mirrors the TypeScript union exactly.
/// `Plaintext` is the only identifier without a grammar/highlighter.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum EditorLanguageId {
    /// CSS and SCSS source.
    Css,
    /// Go source.
    Go,
    /// HTML-shaped source, including Svelte files.
    Html,
    /// JavaScript source, including JSX files.
    JavaScript,
    /// JSON and JSON-with-comments source.
    Json,
    /// Markdown source, including MDX files.
    Markdown,
    /// Unhighlighted source or an unrecognized file.
    Plaintext,
    /// Python source, including type-stub files.
    Python,
    /// Rust source.
    Rust,
    /// SQL source.
    Sql,
    /// TypeScript source, including TSX files.
    TypeScript,
    /// XML source.
    Xml,
    /// YAML source.
    Yaml,
}

impl EditorLanguageId {
    /// Every language identifier in the same order as the TypeScript union.
    pub const ALL: [Self; 13] = [
        Self::Css,
        Self::Go,
        Self::Html,
        Self::JavaScript,
        Self::Json,
        Self::Markdown,
        Self::Plaintext,
        Self::Python,
        Self::Rust,
        Self::Sql,
        Self::TypeScript,
        Self::Xml,
        Self::Yaml,
    ];

    /// Returns the canonical language identifier used by the TypeScript API.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Css => "css",
            Self::Go => "go",
            Self::Html => "html",
            Self::JavaScript => "javascript",
            Self::Json => "json",
            Self::Markdown => "markdown",
            Self::Plaintext => "plaintext",
            Self::Python => "python",
            Self::Rust => "rust",
            Self::Sql => "sql",
            Self::TypeScript => "typescript",
            Self::Xml => "xml",
            Self::Yaml => "yaml",
        }
    }

    /// Returns whether this identifier has a grammar/highlighter.
    #[must_use]
    pub const fn is_highlighted(self) -> bool {
        !matches!(self, Self::Plaintext)
    }

    /// Parses a declared language only when it names a highlighted language.
    ///
    /// `plaintext` is intentionally absent. The TypeScript policy checks the
    /// grammar table for declared values, and that table has no plaintext
    /// entry; an unsupported declaration therefore falls through to path
    /// detection.
    fn highlighted_from_declared(value: &str) -> Option<Self> {
        match value {
            "css" => Some(Self::Css),
            "go" => Some(Self::Go),
            "html" => Some(Self::Html),
            "javascript" => Some(Self::JavaScript),
            "json" => Some(Self::Json),
            "markdown" => Some(Self::Markdown),
            "python" => Some(Self::Python),
            "rust" => Some(Self::Rust),
            "sql" => Some(Self::Sql),
            "typescript" => Some(Self::TypeScript),
            "xml" => Some(Self::Xml),
            "yaml" => Some(Self::Yaml),
            _ => None,
        }
    }
}

const BY_FILENAME: [(&str, EditorLanguageId); 4] = [
    (".babelrc", EditorLanguageId::Json),
    (".prettierrc", EditorLanguageId::Json),
    ("dockerfile", EditorLanguageId::Plaintext),
    ("makefile", EditorLanguageId::Plaintext),
];

const BY_EXTENSION: [(&str, EditorLanguageId); 25] = [
    ("cjs", EditorLanguageId::JavaScript),
    ("css", EditorLanguageId::Css),
    ("go", EditorLanguageId::Go),
    ("htm", EditorLanguageId::Html),
    ("html", EditorLanguageId::Html),
    ("js", EditorLanguageId::JavaScript),
    ("json", EditorLanguageId::Json),
    ("jsonc", EditorLanguageId::Json),
    ("jsx", EditorLanguageId::JavaScript),
    ("md", EditorLanguageId::Markdown),
    ("mdx", EditorLanguageId::Markdown),
    ("mjs", EditorLanguageId::JavaScript),
    ("mts", EditorLanguageId::TypeScript),
    ("py", EditorLanguageId::Python),
    ("pyi", EditorLanguageId::Python),
    ("rs", EditorLanguageId::Rust),
    ("scss", EditorLanguageId::Css),
    ("sql", EditorLanguageId::Sql),
    ("sv", EditorLanguageId::Html),
    ("svelte", EditorLanguageId::Html),
    ("ts", EditorLanguageId::TypeScript),
    ("tsx", EditorLanguageId::TypeScript),
    ("xml", EditorLanguageId::Xml),
    ("yaml", EditorLanguageId::Yaml),
    ("yml", EditorLanguageId::Yaml),
];

fn find_mapping(table: &[(&str, EditorLanguageId)], candidate: &str) -> Option<EditorLanguageId> {
    table
        .iter()
        .find_map(|&(value, language)| candidate.eq_ignore_ascii_case(value).then_some(language))
}

fn file_name(path: &str) -> &str {
    path.rsplit(|character| matches!(character, '/' | '\\'))
        .next()
        .unwrap_or_default()
}

fn file_extension(file_name: &str) -> &str {
    if let Some(extension) = file_name.strip_prefix('.') {
        // Match the TypeScript rule: a leading dot makes the rest of the
        // complete name the extension rather than selecting the final dot.
        extension
    } else {
        file_name
            .rsplit_once('.')
            .map_or("", |(_, extension)| extension)
    }
}

/// Resolves the editor language for a path.
///
/// A declared language takes precedence only when it is one of the known
/// highlighted identifiers. Otherwise the basename table is consulted before
/// the final extension. Filename and extension matching is ASCII
/// case-insensitive, and both `/` and `\\` are accepted as path separators.
/// Unknown, extensionless, and unsupported dotfile names resolve to
/// [`EditorLanguageId::Plaintext`] unless one of the exact tables recognizes
/// them.
#[must_use]
pub fn editor_language_for_path(path: &str, declared_language: Option<&str>) -> EditorLanguageId {
    if let Some(declared_language) = declared_language
        && let Some(language) = EditorLanguageId::highlighted_from_declared(declared_language)
    {
        return language;
    }

    let file_name = file_name(path);
    if let Some(language) = find_mapping(&BY_FILENAME, file_name) {
        return language;
    }

    find_mapping(&BY_EXTENSION, file_extension(file_name)).unwrap_or(EditorLanguageId::Plaintext)
}

/// Returns whether a resolved language has a grammar/highlighter.
#[must_use]
pub const fn editor_language_is_highlighted(language: EditorLanguageId) -> bool {
    language.is_highlighted()
}
