//! Dependency-free composition plan for the test-only conversation Markdown parser.
//!
//! This is the native counterpart of
//! `modules/frontend/src/lib/components/markdown/test-parsing.ts`. It records
//! the full conversation dialect and its test highlighting registration without
//! parsing Markdown, loading grammars, tokenizing code, or importing a browser
//! or runtime dependency. A later native adapter can consume the plan at the
//! boundary where those concerns are available.

#![forbid(unsafe_code)]
#![allow(clippy::module_name_repetitions)]

/// The graph to which a parser plan belongs.
///
/// The test plan below is explicitly tagged [`Self::TestOnly`]. Keeping the
/// live renderer graph as a separate value makes an accidental registration of
/// the test highlighter mechanically visible to adapters and tests.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum MarkdownParserGraph {
    /// The production renderer's parser graph.
    LiveRenderer,
    /// A parser graph used only by tests and fixtures.
    TestOnly,
}

impl MarkdownParserGraph {
    /// Returns whether this graph is the production live-renderer graph.
    #[must_use]
    pub const fn is_live_renderer(self) -> bool {
        matches!(self, Self::LiveRenderer)
    }

    /// Returns whether this graph is reserved for tests and fixtures.
    #[must_use]
    pub const fn is_test_only(self) -> bool {
        matches!(self, Self::TestOnly)
    }
}

/// The conversation Markdown parser's non-plugin options.
///
/// The legacy `conversation_parse_options` object contains exactly
/// `{ html: false }`. Keeping the option in a typed value preserves that
/// security boundary for a later adapter without implementing parsing here.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct MarkdownParseOptions {
    html: bool,
}

impl MarkdownParseOptions {
    /// Returns whether raw HTML is enabled for the parser.
    #[must_use]
    pub const fn html_enabled(self) -> bool {
        self.html
    }

    /// Returns whether the parser may turn raw HTML into live elements.
    #[must_use]
    pub const fn allows_html(self) -> bool {
        self.html
    }
}

/// One of the rich Markdown plugins shared by conversation rendering.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum ConversationRichMarkdownPlugin {
    /// Recognizes conversation math blocks and expressions.
    Math,
    /// Recognizes conversation Mermaid blocks.
    Mermaid,
}

/// The only grammar registered by the test highlighting plugin.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum TestHighlightGrammar {
    /// Shiki's TypeScript grammar imported by the legacy test parser.
    TypeScript,
}

impl TestHighlightGrammar {
    /// Returns the stable grammar identifier expected by a native adapter.
    #[must_use]
    pub const fn identifier(self) -> &'static str {
        match self {
            Self::TypeScript => "typescript",
        }
    }

    /// Returns the exact legacy module path for this grammar.
    #[must_use]
    pub const fn module_path(self) -> &'static str {
        match self {
            Self::TypeScript => "shiki/dist/langs/typescript.mjs",
        }
    }
}

/// A theme registered by the test highlighting plugin.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum TestHighlightTheme {
    /// Shiki's GitHub dark theme.
    GithubDark,
    /// Shiki's GitHub light theme.
    GithubLight,
}

impl TestHighlightTheme {
    /// Returns the stable theme identifier expected by a native adapter.
    #[must_use]
    pub const fn identifier(self) -> &'static str {
        match self {
            Self::GithubDark => "github-dark",
            Self::GithubLight => "github-light",
        }
    }

    /// Returns the exact legacy module path for this theme.
    #[must_use]
    pub const fn module_path(self) -> &'static str {
        match self {
            Self::GithubDark => "shiki/dist/themes/github-dark.mjs",
            Self::GithubLight => "shiki/dist/themes/github-light.mjs",
        }
    }
}

/// Exact registration options for the test-only highlighting plugin.
///
/// The grammar and theme references are borrowed from immutable static plan
/// data. This keeps the model deterministic and prevents a caller from
/// silently broadening the test parser's registration set.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct TestHighlightingConfig {
    grammars: &'static [TestHighlightGrammar],
    dark_theme: TestHighlightTheme,
    light_theme: TestHighlightTheme,
    register_default_languages: bool,
    register_default_themes: bool,
}

impl TestHighlightingConfig {
    /// Returns the exact grammars registered for tests.
    #[must_use]
    pub const fn grammars(self) -> &'static [TestHighlightGrammar] {
        self.grammars
    }

    /// Returns the theme selected for dark mode.
    #[must_use]
    pub const fn dark_theme(self) -> TestHighlightTheme {
        self.dark_theme
    }

    /// Returns the theme selected for light mode.
    #[must_use]
    pub const fn light_theme(self) -> TestHighlightTheme {
        self.light_theme
    }

    /// Returns whether the highlighter should register its default languages.
    #[must_use]
    pub const fn register_default_languages(self) -> bool {
        self.register_default_languages
    }

    /// Returns whether the highlighter should register its default themes.
    #[must_use]
    pub const fn register_default_themes(self) -> bool {
        self.register_default_themes
    }
}

/// One ordered plugin entry in the test parser plan.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum MarkdownTestParserPlugin {
    /// A plugin from the conversation's shared rich-Markdown set.
    ConversationRich(ConversationRichMarkdownPlugin),
    /// The test-only syntax-highlighting plugin and its fixed registration.
    TestHighlighting(TestHighlightingConfig),
}

/// Fixed full-dialect parser plan used by native tests and fixtures.
///
/// The fields are private so a later adapter can consume the one canonical
/// composition instead of constructing a weaker or live-graph variant. The
/// plan contains no parser state and performs no work when copied.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct MarkdownTestParserPlan {
    graph: MarkdownParserGraph,
    parse_options: MarkdownParseOptions,
    plugins: &'static [MarkdownTestParserPlugin],
}

impl MarkdownTestParserPlan {
    /// Returns the graph membership of this plan.
    #[must_use]
    pub const fn graph(self) -> MarkdownParserGraph {
        self.graph
    }

    /// Returns the parser's non-plugin conversation options.
    #[must_use]
    pub const fn parse_options(self) -> MarkdownParseOptions {
        self.parse_options
    }

    /// Returns the ordered plugin composition consumed by a later adapter.
    #[must_use]
    pub const fn plugins(self) -> &'static [MarkdownTestParserPlugin] {
        self.plugins
    }

    /// Returns whether this plan is reserved for tests and fixtures.
    #[must_use]
    pub const fn is_test_only(self) -> bool {
        self.graph.is_test_only()
    }

    /// Returns whether this plan is part of the live renderer graph.
    #[must_use]
    pub const fn is_live_renderer_graph(self) -> bool {
        self.graph.is_live_renderer()
    }
}

const TEST_HIGHLIGHT_GRAMMARS: [TestHighlightGrammar; 1] = [TestHighlightGrammar::TypeScript];

/// The exact test highlighting configuration from the legacy parser.
pub const TEST_HIGHLIGHTING_CONFIG: TestHighlightingConfig = TestHighlightingConfig {
    grammars: &TEST_HIGHLIGHT_GRAMMARS,
    dark_theme: TestHighlightTheme::GithubDark,
    light_theme: TestHighlightTheme::GithubLight,
    register_default_languages: false,
    register_default_themes: false,
};

const CONVERSATION_TEST_PARSER_PLUGINS: [MarkdownTestParserPlugin; 3] = [
    MarkdownTestParserPlugin::ConversationRich(ConversationRichMarkdownPlugin::Math),
    MarkdownTestParserPlugin::ConversationRich(ConversationRichMarkdownPlugin::Mermaid),
    MarkdownTestParserPlugin::TestHighlighting(TEST_HIGHLIGHTING_CONFIG),
];

/// The canonical test-only full-dialect conversation parser plan.
pub const CONVERSATION_TEST_PARSER_PLAN: MarkdownTestParserPlan = MarkdownTestParserPlan {
    graph: MarkdownParserGraph::TestOnly,
    parse_options: MarkdownParseOptions { html: false },
    plugins: &CONVERSATION_TEST_PARSER_PLUGINS,
};

/// Returns the canonical test-only full-dialect conversation parser plan.
#[must_use]
pub const fn conversation_test_parser_plan() -> MarkdownTestParserPlan {
    CONVERSATION_TEST_PARSER_PLAN
}
