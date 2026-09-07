//! Focused parity coverage for repository-host mark presentation.
//!
//! The production module is loaded directly so this harness remains
//! dependency-free and does not require shared crate registration, UI code, or
//! the native asset catalog.

use std::str::FromStr;

#[path = "../../modules/frontend/src/repository_mark.rs"]
mod repository_mark;

use repository_mark::{
    DEFAULT_MARK_SIZE, RepositoryHost, RepositoryLogo, RepositoryMark, UnknownRepositoryHost,
    repository_chip_mark_class, repository_mark_class, repository_mark_for,
};

const WHITE_MARK: &str = "brightness-0 invert";
const OPPOSING_MARK: &str = "brightness-0 invert dark:invert-0";

#[test]
fn host_vocabulary_is_exhaustive_and_round_trips_exact_protocol_spellings() {
    let expected = [
        (RepositoryHost::Azure, "azure"),
        (RepositoryHost::Bitbucket, "bitbucket"),
        (RepositoryHost::Codeberg, "codeberg"),
        (RepositoryHost::Gitea, "gitea"),
        (RepositoryHost::GitHub, "github"),
        (RepositoryHost::GitLab, "gitlab"),
        (RepositoryHost::Other, "other"),
        (RepositoryHost::Sourcehut, "sourcehut"),
        (RepositoryHost::Unknown, "unknown"),
    ];

    assert_eq!(RepositoryHost::ALL, expected.map(|(host, _)| host));
    for (host, spelling) in expected {
        assert_eq!(host.as_str(), spelling);
        assert_eq!(RepositoryHost::from_str(spelling), Ok(host));
        assert_eq!(spelling.parse::<RepositoryHost>(), Ok(host));
    }

    for spelling in ["", "GitHub", "source-hut", " azure"] {
        assert_eq!(
            RepositoryHost::from_str(spelling),
            Err(UnknownRepositoryHost),
            "spelling={spelling:?}"
        );
        let error = spelling.parse::<RepositoryHost>().unwrap_err();
        assert_eq!(error, UnknownRepositoryHost, "spelling={spelling:?}");
        assert_eq!(error.to_string(), "unknown repository host");
    }
}

#[test]
fn logo_identity_vocabulary_is_distinct_and_asset_independent() {
    assert_eq!(
        RepositoryLogo::ALL,
        [
            RepositoryLogo::Git,
            RepositoryLogo::GitHub,
            RepositoryLogo::GitLab,
            RepositoryLogo::MicrosoftAzure,
            RepositoryLogo::Bitbucket,
            RepositoryLogo::Codeberg,
            RepositoryLogo::Gitea,
            RepositoryLogo::Sourcehut,
        ]
    );
}

#[test]
fn every_host_preserves_the_legacy_mark_mapping_and_exact_classes() {
    let expected: [(RepositoryHost, RepositoryMark); 9] = [
        (
            RepositoryHost::Azure,
            RepositoryMark {
                logo: RepositoryLogo::MicrosoftAzure,
                monochrome: false,
                chip: "bg-[#0078D4]",
                chip_mark: WHITE_MARK,
            },
        ),
        (
            RepositoryHost::Bitbucket,
            RepositoryMark {
                logo: RepositoryLogo::Bitbucket,
                monochrome: false,
                chip: "bg-[#0052CC]",
                chip_mark: WHITE_MARK,
            },
        ),
        (
            RepositoryHost::Codeberg,
            RepositoryMark {
                logo: RepositoryLogo::Codeberg,
                monochrome: false,
                chip: "bg-[#2185D0]",
                chip_mark: WHITE_MARK,
            },
        ),
        (
            RepositoryHost::Gitea,
            RepositoryMark {
                logo: RepositoryLogo::Gitea,
                monochrome: false,
                chip: "bg-[#609926]",
                chip_mark: WHITE_MARK,
            },
        ),
        (
            RepositoryHost::GitHub,
            RepositoryMark {
                logo: RepositoryLogo::GitHub,
                monochrome: true,
                chip: "bg-[#181717] dark:bg-white",
                chip_mark: OPPOSING_MARK,
            },
        ),
        (
            RepositoryHost::GitLab,
            RepositoryMark {
                logo: RepositoryLogo::GitLab,
                monochrome: false,
                chip: "bg-[#FC6D26]",
                chip_mark: WHITE_MARK,
            },
        ),
        (
            RepositoryHost::Other,
            RepositoryMark {
                logo: RepositoryLogo::Git,
                monochrome: false,
                chip: "bg-[#DE4C36]",
                chip_mark: WHITE_MARK,
            },
        ),
        (
            RepositoryHost::Sourcehut,
            RepositoryMark {
                logo: RepositoryLogo::Sourcehut,
                monochrome: false,
                chip: "bg-black dark:bg-white",
                chip_mark: OPPOSING_MARK,
            },
        ),
        (
            RepositoryHost::Unknown,
            RepositoryMark {
                logo: RepositoryLogo::Git,
                monochrome: false,
                chip: "bg-[#DE4C36]",
                chip_mark: WHITE_MARK,
            },
        ),
    ];

    for (host, expected_mark) in expected {
        assert_eq!(
            repository_mark_for(Some(host)),
            expected_mark,
            "host={host:?}"
        );
    }
}

#[test]
fn absent_host_and_fallback_hosts_share_the_plain_git_mark() {
    let plain = repository_mark_for(None);
    assert_eq!(plain.logo, RepositoryLogo::Git);
    assert!(!plain.monochrome);
    assert_eq!(plain.chip, "bg-[#DE4C36]");
    assert_eq!(plain.chip_mark, WHITE_MARK);
    assert_eq!(repository_mark_for(Some(RepositoryHost::Other)), plain);
    assert_eq!(repository_mark_for(Some(RepositoryHost::Unknown)), plain);
}

#[test]
fn standalone_mark_classes_use_the_size_four_default_and_only_monochrome_inverts() {
    let normal = repository_mark_for(Some(RepositoryHost::GitLab));
    let monochrome = repository_mark_for(Some(RepositoryHost::GitHub));

    assert_eq!(DEFAULT_MARK_SIZE, "size-4");
    assert_eq!(repository_mark_class(normal, None), "size-4 shrink-0");
    assert_eq!(
        repository_mark_class(monochrome, None),
        "size-4 shrink-0 dark:invert"
    );

    // SourceHut has a dark chip-mark rule but is not a monochrome standalone
    // mark, so the standalone helper must not inherit that chip behavior.
    let sourcehut = repository_mark_for(Some(RepositoryHost::Sourcehut));
    assert_eq!(repository_mark_class(sourcehut, None), "size-4 shrink-0");
}

#[test]
fn both_class_helpers_preserve_custom_sizes_and_exact_mark_policy() {
    let cases = [
        (
            RepositoryHost::Azure,
            "size-3.5",
            "size-3.5 shrink-0",
            "size-3.5 shrink-0 brightness-0 invert",
        ),
        (
            RepositoryHost::GitHub,
            "size-6",
            "size-6 shrink-0 dark:invert",
            "size-6 shrink-0 brightness-0 invert dark:invert-0",
        ),
        (
            RepositoryHost::Sourcehut,
            "h-5 w-5",
            "h-5 w-5 shrink-0",
            "h-5 w-5 shrink-0 brightness-0 invert dark:invert-0",
        ),
    ];

    for (host, size, expected_mark, expected_chip_mark) in cases {
        let mark = repository_mark_for(Some(host));
        assert_eq!(repository_mark_class(mark, Some(size)), expected_mark);
        assert_eq!(
            repository_chip_mark_class(mark, Some(size)),
            expected_chip_mark
        );
    }
}

#[test]
fn chip_mark_classes_never_replace_the_exact_chip_face_class() {
    let github = repository_mark_for(Some(RepositoryHost::GitHub));
    assert_eq!(github.chip, "bg-[#181717] dark:bg-white");
    assert_eq!(
        repository_chip_mark_class(github, None),
        "size-4 shrink-0 brightness-0 invert dark:invert-0"
    );

    let gitlab = repository_mark_for(Some(RepositoryHost::GitLab));
    assert_eq!(gitlab.chip, "bg-[#FC6D26]");
    assert_eq!(
        repository_chip_mark_class(gitlab, None),
        "size-4 shrink-0 brightness-0 invert"
    );
}
