//! Focused parity tests for the dependency-free Markdown test parser plan.
//!
//! The production module is path-linked deliberately. This packet describes a
//! parser composition only, so it needs no Cargo/Bazel registration, parser
//! dependency, browser runtime, or native renderer.

#[path = "../../modules/frontend/src/markdown_test_parser_policy.rs"]
mod markdown_test_parser_policy;

use markdown_test_parser_policy::{
    CONVERSATION_TEST_PARSER_PLAN, ConversationRichMarkdownPlugin, MarkdownParserGraph,
    MarkdownTestParserPlugin, TestHighlightGrammar, TestHighlightTheme,
    conversation_test_parser_plan,
};

#[test]
fn rich_markdown_plugins_precede_the_test_highlighter_in_exact_order() {
    let plan = conversation_test_parser_plan();

    assert_eq!(plan.plugins().len(), 3);
    assert_eq!(
        plan.plugins()[0],
        MarkdownTestParserPlugin::ConversationRich(ConversationRichMarkdownPlugin::Math)
    );
    assert_eq!(
        plan.plugins()[1],
        MarkdownTestParserPlugin::ConversationRich(ConversationRichMarkdownPlugin::Mermaid)
    );

    let MarkdownTestParserPlugin::TestHighlighting(highlighting) = plan.plugins()[2] else {
        panic!("test highlighting must be the final plugin");
    };
    assert_eq!(
        highlighting,
        markdown_test_parser_policy::TEST_HIGHLIGHTING_CONFIG
    );
}

#[test]
fn test_highlighter_has_only_typescript_and_the_exact_github_theme_pair() {
    let MarkdownTestParserPlugin::TestHighlighting(highlighting) =
        CONVERSATION_TEST_PARSER_PLAN.plugins()[2]
    else {
        panic!("test highlighting must be the final plugin");
    };

    assert_eq!(highlighting.grammars(), &[TestHighlightGrammar::TypeScript]);
    assert_eq!(highlighting.grammars()[0].identifier(), "typescript");
    assert_eq!(
        highlighting.grammars()[0].module_path(),
        "shiki/dist/langs/typescript.mjs"
    );
    assert_eq!(highlighting.dark_theme(), TestHighlightTheme::GithubDark);
    assert_eq!(highlighting.light_theme(), TestHighlightTheme::GithubLight);
    assert_eq!(highlighting.dark_theme().identifier(), "github-dark");
    assert_eq!(
        highlighting.dark_theme().module_path(),
        "shiki/dist/themes/github-dark.mjs"
    );
    assert_eq!(highlighting.light_theme().identifier(), "github-light");
    assert_eq!(
        highlighting.light_theme().module_path(),
        "shiki/dist/themes/github-light.mjs"
    );
    assert!(!highlighting.register_default_languages());
    assert!(!highlighting.register_default_themes());
}

#[test]
fn plan_keeps_the_conversation_dialect_and_raw_html_disabled() {
    let options = conversation_test_parser_plan().parse_options();

    assert!(!options.html_enabled());
    assert!(!options.allows_html());
}

#[test]
fn plan_is_explicitly_outside_the_live_renderer_graph() {
    let plan = conversation_test_parser_plan();

    assert_eq!(plan.graph(), MarkdownParserGraph::TestOnly);
    assert!(plan.graph().is_test_only());
    assert!(!plan.graph().is_live_renderer());
    assert!(plan.is_test_only());
    assert!(!plan.is_live_renderer_graph());

    assert!(MarkdownParserGraph::LiveRenderer.is_live_renderer());
    assert!(!MarkdownParserGraph::LiveRenderer.is_test_only());
}

#[test]
fn canonical_constant_and_constructor_are_identical_and_side_effect_free() {
    assert_eq!(
        CONVERSATION_TEST_PARSER_PLAN,
        conversation_test_parser_plan()
    );
    assert_eq!(
        conversation_test_parser_plan().plugins(),
        CONVERSATION_TEST_PARSER_PLAN.plugins()
    );
}
