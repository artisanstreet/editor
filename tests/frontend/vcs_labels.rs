//! Direct, dependency-free parity coverage for the frontend VCS URL labels.
//!
//! The production module is included by path so this focused suite needs no
//! Cargo target, crate registration, protocol dependency, or generated code.
//! Expected strings are written as independent oracle values rather than being
//! recomputed by a second Rust implementation.

#[path = "../../modules/frontend/src/vcs_labels.rs"]
mod vcs_labels;

use vcs_labels::{repository_destination_label, repository_link_label, repository_qualified_label};

#[test]
fn repository_link_label_matches_path_and_host_fallbacks() {
    let cases: &[(&str, &str)] = &[
        ("https://github.com/owner/repo", "repo"),
        ("https://github.com/owner/repo/", "repo"),
        ("https://github.com/repo", "repo"),
        ("https://github.com/repo/", "repo"),
        ("https://github.com", "github.com"),
        ("https://github.com/", "github.com"),
        ("https://example.com", "example.com"),
        ("https://example.com/", "example.com"),
        (
            "https://gitlab.example.com/group/subgroup/project",
            "project",
        ),
        ("https://gitlab.example.com/a/b/c/d/e", "e"),
        ("https://example.com//owner//repo//", "repo"),
        ("https://example.com/a///b/c", "c"),
        ("https://github.com/owner/repo.git", "repo.git"),
        ("https://github.com/owner/my-repo_2.0", "my-repo_2.0"),
        ("https://example.com/owner/a%2Fb", "a%2Fb"),
        ("http://github.com/owner/repo", "repo"),
        ("https://GitHub.COM/owner/Repo", "Repo"),
    ];

    for &(input, expected) in cases {
        assert_eq!(repository_link_label(input), expected, "input {input:?}");
    }
}

#[test]
fn repository_qualified_label_preserves_innermost_order() {
    let cases: &[(&str, &str)] = &[
        ("https://github.com/owner/repo", "owner/repo"),
        ("https://github.com/owner/repo/", "owner/repo"),
        ("https://github.com/repo", "repo"),
        ("https://github.com/repo/", "repo"),
        ("https://github.com", "github.com"),
        ("https://github.com/", "github.com"),
        ("https://example.com/a/b", "a/b"),
        (
            "https://gitlab.example.com/group/subgroup/project",
            "subgroup/project",
        ),
        ("https://example.com/a/b/c", "b/c"),
        ("https://example.com/a/b/c/d", "c/d"),
        ("https://example.com//owner//repo//", "owner/repo"),
        ("https://example.com///a///b///c", "b/c"),
        ("https://github.com/owner/repo.git", "owner/repo.git"),
        ("https://example.com/owner/a%2Fb", "owner/a%2Fb"),
        ("https://GitHub.COM/Owner/Repo", "Owner/Repo"),
    ];

    for &(input, expected) in cases {
        assert_eq!(
            repository_qualified_label(input),
            expected,
            "input {input:?}"
        );
    }
}

#[test]
fn repository_destination_label_strips_only_transport_and_one_trailing_slash() {
    let cases: &[(&str, &str)] = &[
        ("https://github.com/owner/repo", "github.com/owner/repo"),
        ("https://github.com/owner/repo/", "github.com/owner/repo"),
        ("https://github.com", "github.com"),
        ("https://github.com/", "github.com"),
        ("https://example.com/a/b", "example.com/a/b"),
        ("https://example.com/a/b/", "example.com/a/b"),
        ("https://example.com//", "example.com/"),
        ("https://example.com///", "example.com//"),
        ("https://example.com//a//b/", "example.com//a//b"),
        ("https://example.com/a///b/c/", "example.com/a///b/c"),
        (
            "https://gitlab.example.com/group/subgroup/project/",
            "gitlab.example.com/group/subgroup/project",
        ),
        (
            "https://github.com/owner/repo.git",
            "github.com/owner/repo.git",
        ),
        ("https://example.com/a%2Fb/c", "example.com/a%2Fb/c"),
        ("https://GitHub.COM/Owner/Repo", "github.com/Owner/Repo"),
        ("http://github.com/owner/repo", "github.com/owner/repo"),
    ];

    for &(input, expected) in cases {
        assert_eq!(
            repository_destination_label(input),
            expected,
            "input {input:?}"
        );
    }
}

#[test]
fn credentials_ports_queries_and_fragments_are_not_labels() {
    let inputs = [
        (
            "https://user:pass@github.com:8080/owner/repo?query=1#fragment",
            "repo",
            "owner/repo",
            "github.com/owner/repo",
        ),
        (
            "https://user@GitHub.COM:443/owner/repo/",
            "repo",
            "owner/repo",
            "github.com/owner/repo",
        ),
    ];

    for &(input, link, qualified, destination) in &inputs {
        assert_eq!(repository_link_label(input), link);
        assert_eq!(repository_qualified_label(input), qualified);
        assert_eq!(repository_destination_label(input), destination);
    }
}

#[test]
fn invalid_urls_fall_back_byte_for_byte() {
    let inputs = [
        "",
        "not a url",
        "github.com/owner/repo",
        "://missing-scheme.com/owner/repo",
        "https://",
        "https://:443/owner/repo",
        "https://host:bad/owner/repo",
        "git@github.com:owner/repo.git",
    ];

    for input in inputs {
        assert_eq!(repository_link_label(input), input);
        assert_eq!(repository_qualified_label(input), input);
        assert_eq!(repository_destination_label(input), input);
    }
}

#[test]
fn special_url_syntax_keeps_the_browser_host_and_path_order() {
    let input = "https:///owner/repository";

    assert_eq!(repository_link_label(input), "repository");
    assert_eq!(repository_qualified_label(input), "repository");
    assert_eq!(repository_destination_label(input), "owner/repository");
}

#[test]
fn unknown_hosts_use_the_same_default_host_label() {
    let cases = [
        ("https://unknown.invalid", "unknown.invalid"),
        ("https://unknown.invalid/", "unknown.invalid"),
        ("https://other.example.net", "other.example.net"),
        ("https://other.example.net/", "other.example.net"),
    ];

    for &(input, expected) in &cases {
        assert_eq!(repository_link_label(input), expected);
        assert_eq!(repository_qualified_label(input), expected);
        assert_eq!(repository_destination_label(input), expected);
    }
}

#[test]
fn labels_keep_their_defined_output_order_and_are_pure() {
    let input = "https://example.com/owner/repo";

    assert_eq!(repository_link_label(input), "repo");
    assert_eq!(repository_qualified_label(input), "owner/repo");
    assert_eq!(
        repository_destination_label(input),
        "example.com/owner/repo"
    );

    assert_eq!(repository_link_label(input), repository_link_label(input));
    assert_eq!(
        repository_qualified_label(input),
        repository_qualified_label(input)
    );
    assert_eq!(
        repository_destination_label(input),
        repository_destination_label(input)
    );
}
