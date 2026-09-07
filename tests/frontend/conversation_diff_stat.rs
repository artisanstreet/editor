//! Table-driven parity coverage for the conversation diff-stat presentation contract.
//!
//! Exercises the public `artisan_frontend::conversation_diff_stat` surface
//! only: compact formatting, typed aggregation, visibility, ordering,
//! zero/missing/invalid handling, and exact labels.

use artisan_frontend::conversation_diff_stat::{
    DiffStat, aggregate_diff_stats, format_compact_diff_count,
};

#[test]
fn compact_formatting_matches_typescript_contract() {
    let table: &[(u64, &str)] = &[
        (0, "0"),
        (1, "1"),
        (5, "5"),
        (999, "999"),
        (1_000, "1k"),
        (1_001, "1k"),
        (1_250, "1.3k"),
        (1_750, "1.8k"),
        (1_499, "1.5k"),
        (1_500, "1.5k"),
        (1_999, "2k"),
        (9_949, "9.9k"),
        (9_950, "9.9k"),
        (9_951, "10k"),
        (10_000, "10k"),
        (10_400, "10k"),
        (10_500, "11k"),
        (99_499, "99k"),
        (99_500, "100k"),
        (999_499, "999k"),
        (999_500, "1000k"),
        (999_999, "1000k"),
        (1_000_000, "1M"),
        (1_040_000, "1M"),
        (1_250_000, "1.3M"),
        (1_750_000, "1.8M"),
        (1_490_000, "1.5M"),
        (1_500_000, "1.5M"),
        (9_949_000, "9.9M"),
        (9_950_000, "9.9M"),
        (9_951_000, "10M"),
        (10_000_000, "10M"),
        (10_500_000, "11M"),
        (999_500_000, "1000M"),
        (1_000_000_000, "1B"),
        (1_040_000_000, "1B"),
        (1_250_000_000, "1.3B"),
        (1_750_000_000, "1.8B"),
        (1_500_000_000, "1.5B"),
        (9_949_000_000, "9.9B"),
        (9_950_000_000, "9.9B"),
        (9_951_000_000, "10B"),
        (10_000_000_000, "10B"),
        (10_500_000_000, "11B"),
    ];
    for (value, expected) in table {
        assert_eq!(
            format_compact_diff_count(*value),
            *expected,
            "compact formatting for {value}"
        );
    }
}

#[test]
fn zeros_are_visible_known_and_format_as_zero() {
    let zero = DiffStat::Known {
        additions: 0,
        deletions: 0,
    };
    assert!(zero.is_visible());
    assert!(!zero.is_partial());
    assert_eq!(zero.additions(), Some(0));
    assert_eq!(zero.deletions(), Some(0));
    assert_eq!(zero.label(), Some("0 additions, 0 deletions".to_owned()));
    assert_eq!(zero.compact_pair(), Some(("0".to_owned(), "0".to_owned())));
    assert_eq!(format_compact_diff_count(0), "0");

    let aggregated = aggregate_diff_stats(&[zero, zero]);
    assert_eq!(
        aggregated,
        DiffStat::Known {
            additions: 0,
            deletions: 0
        }
    );
    assert_eq!(
        aggregated.label(),
        Some("0 additions, 0 deletions".to_owned())
    );
}

#[test]
fn additions_only_and_deletions_only_cases() {
    type Case = (DiffStat, Option<String>, Option<(String, String)>);
    let table: &[Case] = &[
        (
            DiffStat::Known {
                additions: 78,
                deletions: 0,
            },
            Some("78 additions, 0 deletions".to_owned()),
            Some(("78".to_owned(), "0".to_owned())),
        ),
        (
            DiffStat::Known {
                additions: 0,
                deletions: 22,
            },
            Some("0 additions, 22 deletions".to_owned()),
            Some(("0".to_owned(), "22".to_owned())),
        ),
        (
            DiffStat::Known {
                additions: 127,
                deletions: 0,
            },
            Some("127 additions, 0 deletions".to_owned()),
            Some(("127".to_owned(), "0".to_owned())),
        ),
        (
            DiffStat::Known {
                additions: 0,
                deletions: 1_500,
            },
            Some("0 additions, 1500 deletions".to_owned()),
            Some(("0".to_owned(), "1.5k".to_owned())),
        ),
    ];
    for (diff, expected_label, expected_compact) in table {
        assert!(diff.is_visible());
        assert_eq!(diff.label(), *expected_label);
        assert_eq!(diff.compact_pair(), *expected_compact);
        assert!(!diff.is_partial());
    }
}

#[test]
fn mixed_counts_and_large_compact_values() {
    let mixed = DiffStat::Known {
        additions: 78,
        deletions: 22,
    };
    assert_eq!(mixed.label(), Some("78 additions, 22 deletions".to_owned()));
    assert_eq!(
        mixed.compact_pair(),
        Some(("78".to_owned(), "22".to_owned()))
    );

    let large = DiffStat::Known {
        additions: 12_345,
        deletions: 9_876_543,
    };
    assert_eq!(
        large.compact_pair(),
        Some(("12k".to_owned(), "9.9M".to_owned()))
    );
    assert_eq!(
        large.label(),
        Some("12345 additions, 9876543 deletions".to_owned())
    );

    let big_agg = aggregate_diff_stats(&[
        DiffStat::Known {
            additions: 600_000,
            deletions: 400_000,
        },
        DiffStat::Known {
            additions: 500_000,
            deletions: 600_000,
        },
    ]);
    assert_eq!(
        big_agg,
        DiffStat::Known {
            additions: 1_100_000,
            deletions: 1_000_000
        }
    );
    assert_eq!(
        big_agg.compact_pair(),
        Some(("1.1M".to_owned(), "1M".to_owned()))
    );
}

#[test]
fn absent_and_malformed_inputs_map_to_unavailable() {
    // Absent: empty input mirrors `files.length === 0` in the TypeScript contract.
    assert_eq!(aggregate_diff_stats(&[]), DiffStat::Unavailable);
    assert!(!DiffStat::Unavailable.is_visible());
    assert_eq!(DiffStat::Unavailable.label(), None);
    assert_eq!(DiffStat::Unavailable.compact_pair(), None);
    assert!(!DiffStat::Unavailable.is_partial());

    // All unavailable stays unavailable, matching `!has_counts` branch.
    let all_unavailable = [
        DiffStat::Unavailable,
        DiffStat::Unavailable,
        DiffStat::Unavailable,
    ];
    assert_eq!(
        aggregate_diff_stats(&all_unavailable),
        DiffStat::Unavailable
    );

    // Malformed or missing line counts are represented by the `Unavailable`
    // variant in the typed contract; they contribute only to
    // `unavailable_files` and never to additive sums.
    let mixed_missing = [
        DiffStat::Known {
            additions: 10,
            deletions: 5,
        },
        DiffStat::Unavailable,
    ];
    assert_eq!(
        aggregate_diff_stats(&mixed_missing),
        DiffStat::Partial {
            additions: 10,
            deletions: 5,
            unavailable_files: 1
        }
    );

    // Partial transitive unavailable counts are summed exactly.
    let transitive = [
        DiffStat::Partial {
            additions: 3,
            deletions: 2,
            unavailable_files: 2,
        },
        DiffStat::Known {
            additions: 7,
            deletions: 3,
        },
        DiffStat::Unavailable,
    ];
    assert_eq!(
        aggregate_diff_stats(&transitive),
        DiffStat::Partial {
            additions: 10,
            deletions: 5,
            unavailable_files: 3
        }
    );
}

#[test]
fn aggregation_table_covers_branch_parity() {
    let cases: &[(&[DiffStat], DiffStat)] = &[
        (&[], DiffStat::Unavailable),
        (&[DiffStat::Unavailable], DiffStat::Unavailable),
        (
            &[
                DiffStat::Known {
                    additions: 2,
                    deletions: 1,
                },
                DiffStat::Known {
                    additions: 3,
                    deletions: 4,
                },
            ],
            DiffStat::Known {
                additions: 5,
                deletions: 5,
            },
        ),
        (
            &[
                DiffStat::Known {
                    additions: 1,
                    deletions: 0,
                },
                DiffStat::Unavailable,
            ],
            DiffStat::Partial {
                additions: 1,
                deletions: 0,
                unavailable_files: 1,
            },
        ),
        (
            &[
                DiffStat::Unavailable,
                DiffStat::Unavailable,
                DiffStat::Known {
                    additions: 0,
                    deletions: 0,
                },
            ],
            DiffStat::Partial {
                additions: 0,
                deletions: 0,
                unavailable_files: 2,
            },
        ),
        (
            &[
                DiffStat::Partial {
                    additions: 5,
                    deletions: 5,
                    unavailable_files: 1,
                },
                DiffStat::Known {
                    additions: 2,
                    deletions: 2,
                },
            ],
            DiffStat::Partial {
                additions: 7,
                deletions: 7,
                unavailable_files: 1,
            },
        ),
        (
            &[
                DiffStat::Known {
                    additions: 0,
                    deletions: 10,
                },
                DiffStat::Unavailable,
                DiffStat::Partial {
                    additions: 5,
                    deletions: 0,
                    unavailable_files: 3,
                },
            ],
            DiffStat::Partial {
                additions: 5,
                deletions: 10,
                unavailable_files: 4,
            },
        ),
    ];
    for (input, expected) in cases {
        assert_eq!(
            aggregate_diff_stats(input),
            *expected,
            "aggregate for {input:?}"
        );
    }
}

#[test]
fn exact_labels_match_svelte_templates() {
    let table: &[(DiffStat, Option<String>)] = &[
        (
            DiffStat::Known {
                additions: 0,
                deletions: 0,
            },
            Some("0 additions, 0 deletions".to_owned()),
        ),
        (
            DiffStat::Known {
                additions: 1,
                deletions: 1,
            },
            Some("1 additions, 1 deletions".to_owned()),
        ),
        (
            DiffStat::Known {
                additions: 10,
                deletions: 5,
            },
            Some("10 additions, 5 deletions".to_owned()),
        ),
        (DiffStat::Unavailable, None),
        (
            DiffStat::Partial {
                additions: 10,
                deletions: 5,
                unavailable_files: 1,
            },
            Some(
                "At least 10 additions and 5 deletions; line counts unavailable for 1 change"
                    .to_owned(),
            ),
        ),
        (
            DiffStat::Partial {
                additions: 10,
                deletions: 5,
                unavailable_files: 2,
            },
            Some(
                "At least 10 additions and 5 deletions; line counts unavailable for 2 changes"
                    .to_owned(),
            ),
        ),
        (
            DiffStat::Partial {
                additions: 0,
                deletions: 0,
                unavailable_files: 3,
            },
            Some(
                "At least 0 additions and 0 deletions; line counts unavailable for 3 changes"
                    .to_owned(),
            ),
        ),
    ];
    for (diff, expected) in table {
        assert_eq!(diff.label(), *expected, "label for {diff:?}");
        assert_eq!(
            diff.is_visible(),
            expected.is_some(),
            "visibility for {diff:?}"
        );
        assert_eq!(
            diff.is_partial(),
            matches!(diff, DiffStat::Partial { .. }),
            "partial flag for {diff:?}"
        );
    }
}

#[test]
fn visibility_and_partial_prefix_contract() {
    let known = DiffStat::Known {
        additions: 3,
        deletions: 4,
    };
    assert!(known.is_visible());
    assert!(!known.is_partial());

    let unavailable = DiffStat::Unavailable;
    assert!(!unavailable.is_visible());
    assert!(!unavailable.is_partial());

    let partial_one = DiffStat::Partial {
        additions: 3,
        deletions: 4,
        unavailable_files: 1,
    };
    assert!(partial_one.is_visible());
    assert!(partial_one.is_partial());
    assert_eq!(
        partial_one.label(),
        Some(
            "At least 3 additions and 4 deletions; line counts unavailable for 1 change".to_owned()
        )
    );

    let partial_many = DiffStat::Partial {
        additions: 3,
        deletions: 4,
        unavailable_files: 5,
    };
    assert!(partial_many.is_partial());
    assert_eq!(
        partial_many.label(),
        Some(
            "At least 3 additions and 4 deletions; line counts unavailable for 5 changes"
                .to_owned()
        )
    );
}

#[test]
fn ordering_does_not_affect_sums() {
    let a = DiffStat::Known {
        additions: 10,
        deletions: 1,
    };
    let b = DiffStat::Known {
        additions: 20,
        deletions: 2,
    };
    let c = DiffStat::Unavailable;
    let forward = aggregate_diff_stats(&[a, b, c]);
    let reverse = aggregate_diff_stats(&[c, b, a]);
    let shuffled = aggregate_diff_stats(&[b, c, a]);
    assert_eq!(forward, reverse);
    assert_eq!(forward, shuffled);
    assert_eq!(
        forward,
        DiffStat::Partial {
            additions: 30,
            deletions: 3,
            unavailable_files: 1
        }
    );
}

#[test]
fn malformed_overflow_saturates_instead_of_wrapping_visible_counts() {
    assert_eq!(
        aggregate_diff_stats(&[
            DiffStat::Known {
                additions: u64::MAX,
                deletions: u64::MAX,
            },
            DiffStat::Known {
                additions: 1,
                deletions: 1,
            },
        ]),
        DiffStat::Known {
            additions: u64::MAX,
            deletions: u64::MAX,
        }
    );
}
