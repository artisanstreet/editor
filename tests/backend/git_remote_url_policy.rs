//! Focused dependency-free parity coverage for the Git remote URL policy.

#![forbid(unsafe_code)]

#[path = "../../modules/backend/src/git_remote_url_policy.rs"]
mod git_remote_url_policy;

use git_remote_url_policy::{RepositoryHost, repository_host_for, repository_web_url_for};

#[test]
fn every_remote_syntax_preserves_host_custody_and_https_projection() {
    let cases = [
        (
            " \u{feff}https://GITHUB.com/owner/repository.git/\u{00a0} ",
            RepositoryHost::Github,
            Some("https://github.com/owner/repository"),
        ),
        (
            "ssh://git@github.com:22/owner/repository.git",
            RepositoryHost::Github,
            Some("https://github.com/owner/repository"),
        ),
        (
            "git@github.com:owner/repository.git",
            RepositoryHost::Github,
            Some("https://github.com/owner/repository"),
        ),
        (
            "git+ssh://git@github.com/owner/repository.git",
            RepositoryHost::Github,
            Some("https://github.com/owner/repository"),
        ),
        (
            "https://user:password@forge.example.test/owner/repository.git?ref=main#readme",
            RepositoryHost::Other,
            Some("https://forge.example.test/owner/repository"),
        ),
        ("file:///tmp/repository.git", RepositoryHost::Unknown, None),
        ("/tmp/repository.git", RepositoryHost::Unknown, None),
        ("./repository.git", RepositoryHost::Unknown, None),
        ("../repository.git", RepositoryHost::Unknown, None),
        ("C:/src/repository.git", RepositoryHost::Unknown, None),
        (r"C:\src\repository.git", RepositoryHost::Unknown, None),
    ];

    for (remote, expected_host, expected_web_url) in cases {
        assert_eq!(
            repository_host_for(remote),
            expected_host,
            "remote: {remote}"
        );
        assert_eq!(
            repository_web_url_for(remote).as_deref(),
            expected_web_url,
            "remote: {remote}"
        );
    }
}

#[test]
fn known_host_families_are_case_insensitive_and_include_exact_subdomain_rules() {
    let cases = [
        ("https://github.com/org/repo", RepositoryHost::Github),
        ("https://team.GITHUB.com/org/repo", RepositoryHost::Github),
        ("https://gitlab.com/org/repo", RepositoryHost::Gitlab),
        ("https://gitlab.example/org/repo", RepositoryHost::Gitlab),
        (
            "https://team.gitlab.example/org/repo",
            RepositoryHost::Gitlab,
        ),
        ("https://bitbucket.org/org/repo", RepositoryHost::Bitbucket),
        (
            "https://team.bitbucket.org/org/repo",
            RepositoryHost::Bitbucket,
        ),
        ("https://dev.azure.com/org/repo", RepositoryHost::Azure),
        ("https://team.dev.azure.com/org/repo", RepositoryHost::Azure),
        (
            "https://project.visualstudio.com/org/repo",
            RepositoryHost::Azure,
        ),
        ("https://codeberg.org/org/repo", RepositoryHost::Codeberg),
        (
            "https://team.codeberg.org/org/repo",
            RepositoryHost::Codeberg,
        ),
        ("https://sr.ht/~org/repo", RepositoryHost::Sourcehut),
        ("https://team.sr.ht/~org/repo", RepositoryHost::Sourcehut),
        ("https://gitea.com/org/repo", RepositoryHost::Gitea),
        ("https://gitea.example/org/repo", RepositoryHost::Gitea),
        ("https://team.gitea.example/org/repo", RepositoryHost::Gitea),
        ("https://notgithub.com/org/repo", RepositoryHost::Other),
        (
            "https://github.com.evil.test/org/repo",
            RepositoryHost::Other,
        ),
        ("https://notgitlab.example/org/repo", RepositoryHost::Other),
        ("https://notgitea.example/org/repo", RepositoryHost::Other),
    ];

    for (remote, expected_host) in cases {
        assert_eq!(
            repository_host_for(remote),
            expected_host,
            "remote: {remote}"
        );
    }
}

#[test]
fn host_enum_keeps_protocol_spellings_typed() {
    let cases = [
        (RepositoryHost::Azure, "azure"),
        (RepositoryHost::Bitbucket, "bitbucket"),
        (RepositoryHost::Codeberg, "codeberg"),
        (RepositoryHost::Gitea, "gitea"),
        (RepositoryHost::Github, "github"),
        (RepositoryHost::Gitlab, "gitlab"),
        (RepositoryHost::Other, "other"),
        (RepositoryHost::Sourcehut, "sourcehut"),
        (RepositoryHost::Unknown, "unknown"),
    ];

    for (host, spelling) in cases {
        assert_eq!(host.as_str(), spelling);
        assert_eq!(host.to_string(), spelling);
    }
    assert_eq!(RepositoryHost::GitHub, RepositoryHost::Github);
    assert_eq!(RepositoryHost::GitLab, RepositoryHost::Gitlab);
    assert_eq!(RepositoryHost::SourceHut, RepositoryHost::Sourcehut);
}

#[test]
fn empty_malformed_and_non_browsable_local_inputs_are_unknown() {
    let cases = [
        "",
        " \t\r\n ",
        "\u{0085}",
        "not a URL",
        "https://",
        "https:///",
        "ssh://",
        "git@github.com:",
        r"git@github.com:\owner/repo",
        "file://",
        "file:///tmp/repository",
        r"\\server\share\repository",
        "https://[::1",
        "https://host:abc/repository",
        "https://host:65536/repository",
    ];

    for remote in cases {
        assert_eq!(
            repository_host_for(remote),
            RepositoryHost::Unknown,
            "remote: {remote:?}"
        );
        assert_eq!(
            repository_web_url_for(remote).as_deref(),
            None,
            "remote: {remote:?}"
        );
    }
}

#[test]
fn path_normalization_keeps_boundaries_and_ignores_url_metadata() {
    let cases = [
        (
            "https://example.test///owner//repo.git///?ref=main#readme",
            Some("https://example.test/owner//repo"),
        ),
        (
            "https://example.test/owner/repo.git.git",
            Some("https://example.test/owner/repo.git"),
        ),
        (
            "https://example.test/owner/repo.GIT/",
            Some("https://example.test/owner/repo.GIT"),
        ),
        ("https://example.test/", None),
        ("https://example.test/.git", None),
        ("https://example.test///", None),
        (
            "https://example.test/owner/./repo/../final.git",
            Some("https://example.test/owner/final"),
        ),
    ];

    for (remote, expected) in cases {
        assert_eq!(
            repository_web_url_for(remote).as_deref(),
            expected,
            "remote: {remote}"
        );
    }
}

#[test]
fn scp_users_and_windows_drive_colons_are_classified_independently() {
    let cases = [
        (
            "git@github.com:owner/repository.git",
            RepositoryHost::Github,
            Some("https://github.com/owner/repository"),
        ),
        (
            "user:with:colon@GITLAB.example:group/repository.git",
            RepositoryHost::Gitlab,
            Some("https://gitlab.example/group/repository"),
        ),
        (
            "D:relative/repository",
            RepositoryHost::Other,
            Some("https://d/relative/repository"),
        ),
        (r"D:\work\repository", RepositoryHost::Unknown, None),
        (r"D:/work/repository", RepositoryHost::Unknown, None),
    ];

    for (remote, expected_host, expected_web_url) in cases {
        assert_eq!(
            repository_host_for(remote),
            expected_host,
            "remote: {remote}"
        );
        assert_eq!(
            repository_web_url_for(remote).as_deref(),
            expected_web_url,
            "remote: {remote}"
        );
    }
}

#[test]
fn azure_v3_ssh_paths_rewrite_only_at_the_exact_segment_boundary() {
    let cases = [
        (
            "git@ssh.dev.azure.com:v3/Org/Project/Repo.git",
            Some("https://ssh.dev.azure.com/Org/Project/_git/Repo"),
        ),
        (
            "ssh://git@SSH.DEV.AZURE.COM/v3/Org/Project/Repo/",
            Some("https://ssh.dev.azure.com/Org/Project/_git/Repo"),
        ),
        (
            "git@ssh.dev.azure.com:v30/Org/Project/Repo",
            Some("https://ssh.dev.azure.com/v30/Org/Project/Repo"),
        ),
        (
            "git@ssh.dev.azure.com:v3/Org/Project/Repo/Extra",
            Some("https://ssh.dev.azure.com/v3/Org/Project/Repo/Extra"),
        ),
        (
            "https://dev.azure.com/Org/Project/_git/Repo",
            Some("https://dev.azure.com/Org/Project/_git/Repo"),
        ),
        (
            "git@ssh.dev.azure.com:v3/Org/Project/.git",
            Some("https://ssh.dev.azure.com/v3/Org/Project/"),
        ),
    ];

    for (remote, expected) in cases {
        assert_eq!(
            repository_host_for(remote),
            RepositoryHost::Azure,
            "remote: {remote}"
        );
        assert_eq!(
            repository_web_url_for(remote).as_deref(),
            expected,
            "remote: {remote}"
        );
    }
}
