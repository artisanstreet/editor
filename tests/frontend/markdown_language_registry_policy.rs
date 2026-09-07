//! Focused dependency-free coverage for Markdown language residency policy.

#[path = "../../modules/frontend/src/markdown_language_registry_policy.rs"]
mod markdown_language_registry_policy;

use markdown_language_registry_policy::{
    ConversationLanguageLoadOutcome, ConversationLanguageRegistrationOutcome,
    ConversationLanguageRegistrationPasses, ConversationLanguageRegistryPolicy,
    ConversationLanguageRequest, SUPPORTED_CONVERSATION_LANGUAGE_KEYS,
    is_supported_conversation_language, supported_conversation_language_keys,
};

#[test]
fn catalog_preserves_every_legacy_loader_key_and_order() {
    assert_eq!(
        SUPPORTED_CONVERSATION_LANGUAGE_KEYS,
        [
            "astro",
            "bash",
            "c",
            "cpp",
            "csharp",
            "css",
            "go",
            "html",
            "java",
            "javascript",
            "json",
            "jsx",
            "markdown",
            "powershell",
            "python",
            "rust",
            "sql",
            "svelte",
            "toml",
            "tsx",
            "typescript",
            "vue",
            "xml",
            "yaml",
        ]
    );
    assert_eq!(
        supported_conversation_language_keys(),
        &SUPPORTED_CONVERSATION_LANGUAGE_KEYS
    );
    assert_eq!(SUPPORTED_CONVERSATION_LANGUAGE_KEYS.len(), 24);
    assert!(ConversationLanguageLoadOutcome::Loaded.is_loadable());
    assert!(!ConversationLanguageLoadOutcome::LoadFailed.is_loadable());
    assert!(is_supported_conversation_language("typescript"));
    assert!(!is_supported_conversation_language("TypeScript"));
    assert!(!is_supported_conversation_language("elixir"));
}

#[test]
fn mixed_load_outcomes_register_only_supported_successes() {
    let mut registry = ConversationLanguageRegistryPolicy::new();
    let report = registry.register_requested_languages(
        &[
            ConversationLanguageRequest::loaded("rust"),
            ConversationLanguageRequest::load_failed("python"),
            ConversationLanguageRequest::unknown("elixir"),
            ConversationLanguageRequest::loaded("javascript"),
        ],
        ConversationLanguageRegistrationPasses::succeeded(),
    );

    assert_eq!(report.loadable_languages, ["rust", "javascript"]);
    assert_eq!(report.unknown_languages, ["elixir"]);
    assert_eq!(report.load_failed_languages, ["python"]);
    assert_eq!(report.registered_languages, ["rust", "javascript"]);
    assert_eq!(report.newly_resident_languages, ["rust", "javascript"]);
    assert_eq!(
        report.registration_outcome,
        ConversationLanguageRegistrationOutcome::Succeeded
    );
    assert!(report.registration_outcome.is_succeeded());
    assert_eq!(registry.resident_languages(), ["javascript", "rust"]);
    assert!(!registry.is_language_resident("python"));
    assert!(!registry.is_language_resident("elixir"));
}

#[test]
fn registration_requires_both_shared_highlighter_passes() {
    let mut registry = ConversationLanguageRegistryPolicy::new();
    let first_failed = registry.register_requested_languages(
        &[ConversationLanguageRequest::loaded("rust")],
        ConversationLanguageRegistrationPasses::first_failed(),
    );

    assert_eq!(first_failed.passes_attempted, 1);
    assert_eq!(first_failed.registered_languages, Vec::<String>::new());
    assert_eq!(
        first_failed.registration_outcome,
        ConversationLanguageRegistrationOutcome::Failed
    );
    assert!(!registry.is_language_resident("rust"));

    let second_failed = registry.register_requested_languages(
        &[ConversationLanguageRequest::loaded("rust")],
        ConversationLanguageRegistrationPasses::second_failed(),
    );

    assert_eq!(second_failed.passes_attempted, 2);
    assert_eq!(second_failed.registered_languages, Vec::<String>::new());
    assert!(!registry.is_language_resident("rust"));

    let succeeded = registry.register_requested_languages(
        &[ConversationLanguageRequest::loaded("rust")],
        ConversationLanguageRegistrationPasses::succeeded(),
    );

    assert_eq!(succeeded.passes_attempted, 2);
    assert_eq!(succeeded.registered_languages, ["rust"]);
    assert!(registry.is_language_resident("rust"));
}

#[test]
fn duplicate_and_repeated_requests_are_deterministic_and_idempotent() {
    let requests = [
        ConversationLanguageRequest::loaded("rust"),
        ConversationLanguageRequest::loaded("rust"),
        ConversationLanguageRequest::loaded("python"),
        ConversationLanguageRequest::loaded("python"),
    ];
    let mut registry = ConversationLanguageRegistryPolicy::new();

    let first = registry.register_requested_languages(
        &requests,
        ConversationLanguageRegistrationPasses::succeeded(),
    );
    let resident_after_first = registry.resident_languages();
    let second = registry.register_requested_languages(
        &requests,
        ConversationLanguageRegistrationPasses::succeeded(),
    );

    assert_eq!(first.loadable_languages, ["rust", "python"]);
    assert_eq!(first.registered_languages, ["rust", "python"]);
    assert_eq!(first.newly_resident_languages, ["rust", "python"]);
    assert_eq!(second.loadable_languages, first.loadable_languages);
    assert_eq!(second.registered_languages, first.registered_languages);
    assert!(second.newly_resident_languages.is_empty());
    assert_eq!(registry.resident_languages(), resident_after_first);
    assert_eq!(second.resident_languages, ["python", "rust"]);
}

#[test]
fn failed_registration_does_not_create_residents_and_retry_remains_possible() {
    let mut registry = ConversationLanguageRegistryPolicy::new();
    let failed = registry.register_requested_languages(
        &[ConversationLanguageRequest::loaded("typescript")],
        ConversationLanguageRegistrationPasses::second_failed(),
    );

    assert_eq!(
        failed.registration_outcome,
        ConversationLanguageRegistrationOutcome::Failed
    );
    assert!(failed.resident_languages.is_empty());
    assert!(!registry.is_language_resident("typescript"));

    let retried = registry.register_requested_languages(
        &[ConversationLanguageRequest::loaded("typescript")],
        ConversationLanguageRegistrationPasses::succeeded(),
    );

    assert_eq!(retried.registered_languages, ["typescript"]);
    assert_eq!(retried.newly_resident_languages, ["typescript"]);
    assert!(registry.is_language_resident("typescript"));
}

#[test]
fn resident_queries_are_exact_complete_and_stable() {
    let mut registry = ConversationLanguageRegistryPolicy::new();
    assert!(registry.are_languages_resident(std::iter::empty::<&str>()));
    assert!(!registry.are_languages_resident(["rust"]));

    let _ = registry.register_requested_languages(
        &[
            ConversationLanguageRequest::loaded("rust"),
            ConversationLanguageRequest::loaded("python"),
        ],
        ConversationLanguageRegistrationPasses::succeeded(),
    );

    assert!(registry.is_language_resident("rust"));
    assert!(registry.are_languages_resident(["rust", "python", "rust"]));
    assert!(!registry.are_languages_resident(["rust", "elixir"]));
    assert!(!registry.are_languages_resident(["Rust"]));
    assert_eq!(registry.resident_languages(), ["python", "rust"]);
}
