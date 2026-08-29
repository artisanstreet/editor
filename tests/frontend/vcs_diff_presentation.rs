//! Focused parity coverage for the repository diff-presentation leaf.
//!
//! The cases mirror `modules/frontend/src/lib/vcs/diff-presentation.ts` and
//! the reached fields of the repository diff protocol projection. They keep
//! formatting boundaries and every reportability trigger independent so a
//! zero-line binary or rename-only change cannot regress to "clean".

use artisan_frontend::vcs_diff_presentation::{
    comparison_label, diff_count, diff_file_count, format_diff_count, format_file_count,
    has_reportable_work, BranchComparison, ComparisonKind, DiffCounts, DiffSnapshot,
};

fn snapshot(
    working: DiffCounts,
    untracked_file_count: u64,
    stash_count: u64,
    comparisons: &[ComparisonKind],
    truncated: bool,
) -> DiffSnapshot {
    DiffSnapshot::new(
        working,
        untracked_file_count,
        stash_count,
        comparisons
            .iter()
            .copied()
            .map(BranchComparison::new)
            .collect(),
        truncated,
    )
}

#[test]
fn diff_counts_use_english_grouping_at_boundaries() {
    let cases: &[(u64, &str)] = &[
        (0, "0"),
        (1, "1"),
        (999, "999"),
        (1_000, "1,000"),
        (1_001, "1,001"),
        (12_345, "12,345"),
        (1_234_567, "1,234,567"),
        (2_147_483_647, "2,147,483,647"),
    ];

    for (value, expected) in cases {
        assert_eq!(diff_count(*value), *expected, "grouping for {value}");
        assert_eq!(
            format_diff_count(*value),
            *expected,
            "descriptive grouping alias for {value}"
        );
    }
}

#[test]
fn file_counts_keep_exact_singular_and_plural_words() {
    let cases: &[(u64, &str)] = &[
        (0, "0 files"),
        (1, "1 file"),
        (2, "2 files"),
        (1_000, "1,000 files"),
    ];

    for (value, expected) in cases {
        assert_eq!(diff_file_count(*value), *expected, "file tally for {value}");
        assert_eq!(
            format_file_count(*value),
            *expected,
            "descriptive file tally alias for {value}"
        );
    }
}

#[test]
fn comparison_kinds_keep_protocol_literals_and_display_labels() {
    let cases = [
        (ComparisonKind::Upstream, "upstream", "upstream"),
        (
            ComparisonKind::DefaultBranch,
            "default_branch",
            "default branch",
        ),
    ];

    for (kind, protocol, label) in cases {
        assert_eq!(kind.as_protocol_str(), protocol);
        assert_eq!(kind.to_string(), protocol);
        assert_eq!(kind.label(), label);
        assert_eq!(comparison_label(kind), label);
        assert_eq!(ComparisonKind::from_protocol_str(protocol), Some(kind));
    }

    assert_eq!(ComparisonKind::from_protocol_str("default branch"), None);
    assert_eq!(ComparisonKind::from_protocol_str("DEFAULT_BRANCH"), None);
    assert_eq!(ComparisonKind::from_protocol_str(""), None);
}

#[test]
fn clean_snapshot_is_not_reportable() {
    assert!(!has_reportable_work(&DiffSnapshot::clean()));
    assert!(!has_reportable_work(&snapshot(
        DiffCounts::default(),
        0,
        0,
        &[],
        false,
    )));
}

#[test]
fn working_file_count_reports_binary_and_rename_only_changes() {
    let binary = snapshot(DiffCounts::new(1, 1, 0, 0), 0, 0, &[], false);
    assert!(has_reportable_work(&binary));

    let rename_only = snapshot(DiffCounts::new(0, 1, 0, 0), 0, 0, &[], false);
    assert!(has_reportable_work(&rename_only));
}

#[test]
fn every_non_working_reportability_trigger_is_independent() {
    let cases = [
        (
            "working file",
            snapshot(DiffCounts::with_file_count(1), 0, 0, &[], false),
        ),
        (
            "untracked file",
            snapshot(DiffCounts::default(), 1, 0, &[], false),
        ),
        ("stash", snapshot(DiffCounts::default(), 0, 1, &[], false)),
        (
            "upstream comparison",
            snapshot(
                DiffCounts::default(),
                0,
                0,
                &[ComparisonKind::Upstream],
                false,
            ),
        ),
        (
            "default branch comparison",
            snapshot(
                DiffCounts::default(),
                0,
                0,
                &[ComparisonKind::DefaultBranch],
                false,
            ),
        ),
        (
            "truncation",
            snapshot(DiffCounts::default(), 0, 0, &[], true),
        ),
    ];

    for (trigger, diff) in cases {
        assert!(has_reportable_work(&diff), "{trigger} must be reportable");
    }
}
