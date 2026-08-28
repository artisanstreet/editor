//! Pure repository-host mark presentation.
//!
//! This is the dependency-free Rust counterpart of
//! `modules/frontend/src/lib/vcs/presentation.ts`. It preserves the host
//! vocabulary, semantic logo identity, and Tailwind class policy without
//! loading Svelte components or vendored artwork. A renderer can map
//! [`RepositoryLogo`] to its own native asset catalog later.

/// Repository hosting services represented by the legacy presentation table.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum RepositoryHost {
    /// Azure DevOps.
    Azure,
    /// Bitbucket.
    Bitbucket,
    /// Codeberg.
    Codeberg,
    /// Gitea.
    Gitea,
    /// GitHub.
    GitHub,
    /// GitLab.
    GitLab,
    /// A reachable host without a recognized provider identity.
    Other,
    /// SourceHut.
    Sourcehut,
    /// A remote without a usable host identity.
    Unknown,
}

impl RepositoryHost {
    /// Every host in the legacy `RepositoryHost` literal vocabulary, in its
    /// source-table order.
    pub const ALL: [Self; 9] = [
        Self::Azure,
        Self::Bitbucket,
        Self::Codeberg,
        Self::Gitea,
        Self::GitHub,
        Self::GitLab,
        Self::Other,
        Self::Sourcehut,
        Self::Unknown,
    ];

    /// Returns the exact lower-case protocol spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Azure => "azure",
            Self::Bitbucket => "bitbucket",
            Self::Codeberg => "codeberg",
            Self::Gitea => "gitea",
            Self::GitHub => "github",
            Self::GitLab => "gitlab",
            Self::Other => "other",
            Self::Sourcehut => "sourcehut",
            Self::Unknown => "unknown",
        }
    }

    /// Parses one exact protocol spelling.
    #[must_use]
    pub fn from_str(value: &str) -> Option<Self> {
        match value {
            "azure" => Some(Self::Azure),
            "bitbucket" => Some(Self::Bitbucket),
            "codeberg" => Some(Self::Codeberg),
            "gitea" => Some(Self::Gitea),
            "github" => Some(Self::GitHub),
            "gitlab" => Some(Self::GitLab),
            "other" => Some(Self::Other),
            "sourcehut" => Some(Self::Sourcehut),
            "unknown" => Some(Self::Unknown),
            _ => None,
        }
    }
}

/// Asset-independent identity of a repository mark.
///
/// This is an identity key, not an asset loader or an SVG representation. The
/// native renderer owns the eventual mapping from a variant to artwork.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum RepositoryLogo {
    /// The plain Git mark used for local, other, and unknown repositories.
    Git,
    /// The GitHub mark.
    GitHub,
    /// The GitLab mark.
    GitLab,
    /// The Microsoft Azure mark.
    MicrosoftAzure,
    /// The Bitbucket mark.
    Bitbucket,
    /// The Codeberg mark.
    Codeberg,
    /// The Gitea mark.
    Gitea,
    /// The SourceHut mark.
    Sourcehut,
}

impl RepositoryLogo {
    /// Every distinct logo identity used by the host table.
    pub const ALL: [Self; 8] = [
        Self::Git,
        Self::GitHub,
        Self::GitLab,
        Self::MicrosoftAzure,
        Self::Bitbucket,
        Self::Codeberg,
        Self::Gitea,
        Self::Sourcehut,
    ];
}

/// The immutable presentation value for one repository host.
///
/// `chip` and `chip_mark` retain the exact Tailwind classes from the legacy
/// table. All strings are static, so the table returns a small copyable value
/// and never owns mutable presentation state.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct RepositoryMark {
    /// Asset-independent logo identity for a native renderer.
    pub logo: RepositoryLogo,
    /// Whether the standalone mark needs theme inversion.
    pub monochrome: bool,
    /// Classes painting the host chip face.
    pub chip: &'static str,
    /// Classes keeping the mark legible on the chip face.
    pub chip_mark: &'static str,
}

/// The legacy default size used by both mark class helpers.
pub const DEFAULT_MARK_SIZE: &str = "size-4";

const WHITE_MARK_CLASSES: &str = "brightness-0 invert";
const OPPOSING_MARK_CLASSES: &str = "brightness-0 invert dark:invert-0";

const PLAIN_GIT_MARK: RepositoryMark = RepositoryMark {
    logo: RepositoryLogo::Git,
    monochrome: false,
    chip: "bg-[#DE4C36]",
    chip_mark: WHITE_MARK_CLASSES,
};

/// Resolves a host to its immutable presentation value.
///
/// `None`, [`RepositoryHost::Other`], and [`RepositoryHost::Unknown`] all use
/// the plain Git mark, matching the legacy absent-host fallback.
#[must_use]
pub const fn repository_mark_for(host: Option<RepositoryHost>) -> RepositoryMark {
    match host {
        Some(RepositoryHost::Azure) => RepositoryMark {
            logo: RepositoryLogo::MicrosoftAzure,
            monochrome: false,
            chip: "bg-[#0078D4]",
            chip_mark: WHITE_MARK_CLASSES,
        },
        Some(RepositoryHost::Bitbucket) => RepositoryMark {
            logo: RepositoryLogo::Bitbucket,
            monochrome: false,
            chip: "bg-[#0052CC]",
            chip_mark: WHITE_MARK_CLASSES,
        },
        Some(RepositoryHost::Codeberg) => RepositoryMark {
            logo: RepositoryLogo::Codeberg,
            monochrome: false,
            chip: "bg-[#2185D0]",
            chip_mark: WHITE_MARK_CLASSES,
        },
        Some(RepositoryHost::Gitea) => RepositoryMark {
            logo: RepositoryLogo::Gitea,
            monochrome: false,
            chip: "bg-[#609926]",
            chip_mark: WHITE_MARK_CLASSES,
        },
        Some(RepositoryHost::GitHub) => RepositoryMark {
            logo: RepositoryLogo::GitHub,
            monochrome: true,
            chip: "bg-[#181717] dark:bg-white",
            chip_mark: OPPOSING_MARK_CLASSES,
        },
        Some(RepositoryHost::GitLab) => RepositoryMark {
            logo: RepositoryLogo::GitLab,
            monochrome: false,
            chip: "bg-[#FC6D26]",
            chip_mark: WHITE_MARK_CLASSES,
        },
        Some(RepositoryHost::Other) | Some(RepositoryHost::Unknown) | None => PLAIN_GIT_MARK,
        Some(RepositoryHost::Sourcehut) => RepositoryMark {
            logo: RepositoryLogo::Sourcehut,
            monochrome: false,
            chip: "bg-black dark:bg-white",
            chip_mark: OPPOSING_MARK_CLASSES,
        },
    }
}

/// Formats the standalone mark classes.
///
/// `None` preserves the TypeScript helper's `size-4` default; `Some(size)`
/// inserts the caller-provided size verbatim. Dark inversion is added only to
/// marks whose presentation is flagged [`RepositoryMark::monochrome`].
#[must_use]
pub fn repository_mark_class(mark: RepositoryMark, size: Option<&str>) -> String {
    let size = size.unwrap_or(DEFAULT_MARK_SIZE);
    if mark.monochrome {
        format!("{size} shrink-0 dark:invert")
    } else {
        format!("{size} shrink-0")
    }
}

/// Formats the classes for a mark rendered on its chip face.
///
/// `None` preserves the TypeScript helper's `size-4` default; `Some(size)`
/// inserts the caller-provided size verbatim. The remaining classes come from
/// the mark's exact chip-mark policy, including the opposing-face dark rule
/// used by GitHub and SourceHut.
#[must_use]
pub fn repository_chip_mark_class(mark: RepositoryMark, size: Option<&str>) -> String {
    let size = size.unwrap_or(DEFAULT_MARK_SIZE);
    format!("{size} shrink-0 {}", mark.chip_mark)
}
