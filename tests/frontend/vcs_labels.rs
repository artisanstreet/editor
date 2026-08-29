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
fn special_url_dot_segments_match_normalized_pathnames() {
    let cases: &[(&str, &str, &str, &str)] = &[
        (
            "https://example.com/a/../repo",
            "repo",
            "repo",
            "example.com/repo",
        ),
        (
            "https://example.com/a/./repo",
            "repo",
            "a/repo",
            "example.com/a/repo",
        ),
        (
            "https://example.com/../repo",
            "repo",
            "repo",
            "example.com/repo",
        ),
        (
            "https://example.com/./repo",
            "repo",
            "repo",
            "example.com/repo",
        ),
        (
            "https://example.com/../../repo",
            "repo",
            "repo",
            "example.com/repo",
        ),
        (
            "https://example.com/a/b/../../repo",
            "repo",
            "repo",
            "example.com/repo",
        ),
        (
            "https://example.com/a//../repo",
            "repo",
            "a/repo",
            "example.com/a/repo",
        ),
        (
            "https://example.com/a///../repo",
            "repo",
            "a/repo",
            "example.com/a//repo",
        ),
        (
            "https://example.com/a/..",
            "example.com",
            "example.com",
            "example.com",
        ),
        (
            "https://example.com/..",
            "example.com",
            "example.com",
            "example.com",
        ),
        (
            "https://example.com/.",
            "example.com",
            "example.com",
            "example.com",
        ),
        (
            "https://example.com/./",
            "example.com",
            "example.com",
            "example.com",
        ),
        ("https://example.com/a/.", "a", "a", "example.com/a"),
        (
            "https://example.com/a/../",
            "example.com",
            "example.com",
            "example.com",
        ),
    ];

    for &(input, link, qualified, destination) in cases {
        assert_eq!(repository_link_label(input), link, "input {input:?}");
        assert_eq!(
            repository_qualified_label(input),
            qualified,
            "input {input:?}"
        );
        assert_eq!(
            repository_destination_label(input),
            destination,
            "input {input:?}"
        );
    }
}

#[test]
fn special_url_backslashes_match_normalized_pathnames() {
    let cases: &[(&str, &str, &str, &str)] = &[
        (
            "https://example.com\\owner\\repo",
            "repo",
            "owner/repo",
            "example.com/owner/repo",
        ),
        (
            "https://example.com\\\\owner\\\\repo",
            "repo",
            "owner/repo",
            "example.com//owner//repo",
        ),
        (
            "https:\\example.com\\owner\\repo",
            "repo",
            "owner/repo",
            "example.com/owner/repo",
        ),
        (
            "https:\\\\example.com\\\\owner\\\\repo",
            "repo",
            "owner/repo",
            "example.com//owner//repo",
        ),
        (
            "https://example.com/owner\\repo",
            "repo",
            "owner/repo",
            "example.com/owner/repo",
        ),
    ];

    for &(input, link, qualified, destination) in cases {
        assert_eq!(repository_link_label(input), link, "input {input:?}");
        assert_eq!(
            repository_qualified_label(input),
            qualified,
            "input {input:?}"
        );
        assert_eq!(
            repository_destination_label(input),
            destination,
            "input {input:?}"
        );
    }
}

#[test]
fn host_and_path_unicode_are_canonicalized_without_dependencies() {
    let cases: &[(&str, &str, &str, &str)] = &[
        (
            "https://例え.テスト/こんにちは/世界",
            "%E4%B8%96%E7%95%8C",
            "%E3%81%93%E3%82%93%E3%81%AB%E3%81%A1%E3%81%AF/%E4%B8%96%E7%95%8C",
            "xn--r8jz45g.xn--zckzah/%E3%81%93%E3%82%93%E3%81%AB%E3%81%A1%E3%81%AF/%E4%B8%96%E7%95%8C",
        ),
        (
            "https://例え.テスト/所有者/リポジトリ",
            "%E3%83%AA%E3%83%9D%E3%82%B8%E3%83%88%E3%83%AA",
            "%E6%89%80%E6%9C%89%E8%80%85/%E3%83%AA%E3%83%9D%E3%82%B8%E3%83%88%E3%83%AA",
            "xn--r8jz45g.xn--zckzah/%E6%89%80%E6%9C%89%E8%80%85/%E3%83%AA%E3%83%9D%E3%82%B8%E3%83%88%E3%83%AA",
        ),
        (
            "https://example.com/owner/my repo",
            "my%20repo",
            "owner/my%20repo",
            "example.com/owner/my%20repo",
        ),
        (
            "https://example.com/owner/a^b{c}|d\x60e\"f<g>h",
            "a%5Eb%7Bc%7D|d%60e%22f%3Cg%3Eh",
            "owner/a%5Eb%7Bc%7D|d%60e%22f%3Cg%3Eh",
            "example.com/owner/a%5Eb%7Bc%7D|d%60e%22f%3Cg%3Eh",
        ),
        (
            "https://example.com/owner/é 中/😀",
            "%F0%9F%98%80",
            "%C3%A9%20%E4%B8%AD/%F0%9F%98%80",
            "example.com/owner/%C3%A9%20%E4%B8%AD/%F0%9F%98%80",
        ),
    ];

    for &(input, link, qualified, destination) in cases {
        assert_eq!(repository_link_label(input), link, "input {input:?}");
        assert_eq!(
            repository_qualified_label(input),
            qualified,
            "input {input:?}"
        );
        assert_eq!(
            repository_destination_label(input),
            destination,
            "input {input:?}"
        );
    }
}

#[test]
fn existing_escapes_are_preserved_and_encoded_dot_segments_normalize() {
    let cases: &[(&str, &str, &str, &str)] = &[
        (
            "https://example.com/%E2%9C%93/%e2%9c%93/%41/%4a",
            "%4a",
            "%41/%4a",
            "example.com/%E2%9C%93/%e2%9c%93/%41/%4a",
        ),
        (
            "https://example.com/a%20b/%E3%81%82/%2F/%zz/%2e/%2E%2e",
            "%2F",
            "%E3%81%82/%2F",
            "example.com/a%20b/%E3%81%82/%2F",
        ),
        (
            "https://example.com/owner/%2e%2e/repo",
            "repo",
            "repo",
            "example.com/repo",
        ),
        (
            "https://example.com/owner/%2E./repo",
            "repo",
            "repo",
            "example.com/repo",
        ),
        (
            "https://example.com/owner/.%2e/repo",
            "repo",
            "repo",
            "example.com/repo",
        ),
        (
            "https://example.com/owner/%2e%2E/repo",
            "repo",
            "repo",
            "example.com/repo",
        ),
    ];

    for &(input, link, qualified, destination) in cases {
        assert_eq!(repository_link_label(input), link, "input {input:?}");
        assert_eq!(
            repository_qualified_label(input),
            qualified,
            "input {input:?}"
        );
        assert_eq!(
            repository_destination_label(input),
            destination,
            "input {input:?}"
        );
    }
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
