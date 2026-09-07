//! Pure repository-diff presentation helpers.
//!
//! This is the Rust counterpart of
//! `modules/frontend/src/lib/vcs/diff-presentation.ts`. The legacy seam is
//! deliberately independent of Svelte and Git: it formats already validated
//! protocol values and decides whether a repository row has anything to say.
//!
//! The repository-diff protocol types are currently defined in TypeScript, so
//! this module keeps the small projection needed by the reached presentation
//! behavior. Counts retain the complete diff-count shape even though
//! reportability only needs `working.file_count`: binary replacements and
//! rename-only changes can have zero line counts while still being dirty.

#![allow(clippy::module_name_repetitions)]

use std::fmt;

/// A non-negative repository diff count.
///
/// The protocol bounds these values to a signed 32-bit maximum. `u64` keeps
/// the frontend helper convenient for Rust callers while preserving every
/// valid protocol value exactly.
pub type DiffCountValue = u64;

/// Counts for one repository diff.
///
/// This mirrors `RepositoryDiffCounts` in the protocol. The binary and line
/// counts are retained for callers that render the complete summary;
/// [`has_reportable_work`] intentionally uses `file_count` so binary and
/// rename-only changes remain reportable when both line counts are zero.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub struct DiffCounts {
    /// Number of changed files whose contents are binary.
    pub binary_file_count: DiffCountValue,
    /// Number of changed files, including binary and rename-only files.
    pub file_count: DiffCountValue,
    /// Number of added lines.
    pub lines_added: DiffCountValue,
    /// Number of deleted lines.
    pub lines_deleted: DiffCountValue,
}

impl DiffCounts {
    /// Builds a complete diff-count value.
    #[must_use]
    pub const fn new(
        binary_file_count: DiffCountValue,
        file_count: DiffCountValue,
        lines_added: DiffCountValue,
        lines_deleted: DiffCountValue,
    ) -> Self {
        Self {
            binary_file_count,
            file_count,
            lines_added,
            lines_deleted,
        }
    }

    /// Builds counts for a file-only change with no line-count contribution.
    #[must_use]
    pub const fn with_file_count(file_count: DiffCountValue) -> Self {
        Self {
            binary_file_count: 0,
            file_count,
            lines_added: 0,
            lines_deleted: 0,
        }
    }
}

/// Protocol spelling for the complete diff-count projection.
pub type RepositoryDiffCounts = DiffCounts;

/// Names the ref being compared with the checked-out branch.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum ComparisonKind {
    /// The branch's configured tracking ref.
    Upstream,
    /// The repository's default branch.
    DefaultBranch,
}

impl ComparisonKind {
    /// Returns the exact protocol literal.
    #[must_use]
    pub const fn as_protocol_str(self) -> &'static str {
        match self {
            Self::Upstream => "upstream",
            Self::DefaultBranch => "default_branch",
        }
    }

    /// Returns the exact human-readable comparison label.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Upstream => "upstream",
            Self::DefaultBranch => "default branch",
        }
    }

    /// Parses an exact protocol literal.
    #[must_use]
    pub fn from_protocol_str(value: &str) -> Option<Self> {
        match value {
            "upstream" => Some(Self::Upstream),
            "default_branch" => Some(Self::DefaultBranch),
            _ => None,
        }
    }
}

impl fmt::Display for ComparisonKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_protocol_str())
    }
}

/// The portion of a branch comparison consumed by presentation.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct BranchComparison {
    pub kind: ComparisonKind,
}

impl BranchComparison {
    /// Builds a comparison projection from its protocol kind.
    #[must_use]
    pub const fn new(kind: ComparisonKind) -> Self {
        Self { kind }
    }
}

/// Protocol spelling for the branch-comparison projection.
pub type RepositoryBranchComparison = BranchComparison;

/// The portion of a repository diff snapshot consumed by presentation.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct DiffSnapshot {
    /// The complete working-tree diff against `HEAD`.
    pub working: DiffCounts,
    /// Number of untracked files outside the working diff.
    pub untracked_file_count: DiffCountValue,
    /// Number of stashes available for the repository.
    pub stash_count: DiffCountValue,
    /// Comparisons available for the checked-out branch.
    pub comparisons: Vec<BranchComparison>,
    /// Whether the server truncated the snapshot.
    pub truncated: bool,
}

impl DiffSnapshot {
    /// Builds a repository snapshot projection.
    #[must_use]
    pub fn new(
        working: DiffCounts,
        untracked_file_count: DiffCountValue,
        stash_count: DiffCountValue,
        comparisons: Vec<BranchComparison>,
        truncated: bool,
    ) -> Self {
        Self {
            working,
            untracked_file_count,
            stash_count,
            comparisons,
            truncated,
        }
    }

    /// Returns a snapshot with no reportable work.
    #[must_use]
    pub fn clean() -> Self {
        Self::default()
    }
}

/// Protocol spelling for the repository snapshot projection.
pub type RepositoryDiffSnapshot = DiffSnapshot;

/// Groups decimal digits with English thousands separators.
///
/// This mirrors `value.toLocaleString("en")` for the non-negative bounded
/// integer counts accepted by the repository protocol.
#[must_use]
pub fn diff_count(value: DiffCountValue) -> String {
    let digits = value.to_string();
    let first_group_len = match digits.len() % 3 {
        0 => 3,
        remainder => remainder,
    };
    let group_count = (digits.len() - first_group_len) / 3;
    let mut formatted = String::with_capacity(digits.len() + group_count);
    formatted.push_str(&digits[..first_group_len]);

    let mut index = first_group_len;
    while index < digits.len() {
        formatted.push(',');
        formatted.push_str(&digits[index..index + 3]);
        index += 3;
    }

    formatted
}

/// Formats a file tally with English thousands separators and exact
/// singular/plural wording.
#[must_use]
pub fn diff_file_count(value: DiffCountValue) -> String {
    let noun = if value == 1 { "file" } else { "files" };
    format!("{} {noun}", diff_count(value))
}

/// Names what a comparison's ref is to the checked-out branch.
#[must_use]
pub const fn comparison_label(kind: ComparisonKind) -> &'static str {
    kind.label()
}

/// Whether a repository diff has anything to report.
///
/// Each condition mirrors one independent branch of `HasReportableWork` in
/// the TypeScript leaf. In particular, `working.file_count` is intentionally
/// independent of line counts so binary and rename-only dirty states are not
/// mistaken for a clean working tree.
#[must_use]
pub fn has_reportable_work(diff: &DiffSnapshot) -> bool {
    diff.working.file_count > 0
        || diff.untracked_file_count > 0
        || diff.stash_count > 0
        || !diff.comparisons.is_empty()
        || diff.truncated
}

/// Descriptive alias for [`diff_count`].
#[must_use]
pub fn format_diff_count(value: DiffCountValue) -> String {
    diff_count(value)
}

/// Descriptive alias for [`diff_file_count`].
#[must_use]
pub fn format_file_count(value: DiffCountValue) -> String {
    diff_file_count(value)
}
