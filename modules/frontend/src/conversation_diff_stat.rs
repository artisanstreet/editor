//! Conversation diff-stat presentation helpers.
//!
//! Pure port of `modules/frontend/src/lib/conversation/diff-stat.ts` and the
//! diff-stat presentation contract in
//! `modules/frontend/src/routes/components/conversation-changes-card.svelte`
//! together with `modules/frontend/src/lib/conversation/file-change-groups.ts`.
//!
//! The TypeScript source formats counts into a fixed four-character column and
//! aggregates per-file diffs while keeping incomplete totals visibly honest.
//! This module owns those exact presentation facts as typed Rust values so a
//! renderer cannot drift from the audit: compact formatting, additive
//! aggregation, visibility, ordering, zero and unavailable handling, and the
//! exact aria/display labels.
//!
//! The model is pure and deterministic and performs no Git execution,
//! filesystem access, or DOM work.

/// Fits a diff count into the fixed four-character statistic column.
///
/// Mirrors `format_compact_diff_count` in
/// `modules/frontend/src/lib/conversation/diff-stat.ts` exactly:
/// - `< 1_000` renders as the decimal string.
/// - `< 1_000_000` renders as `k` with one decimal place while `< 10k`,
///   otherwise rounded to the nearest whole `k`.
/// - `< 1_000_000_000` renders as `M` with the same `< 10` rule.
/// - Otherwise renders as `B` with the same rule, including stripping a
///   trailing `.0` after `toFixed(1)`.
///
/// The thresholds and rounding match `Math.round` and `Number.toFixed(1)`
/// for non-negative inputs.
#[must_use]
pub fn format_compact_diff_count(value: u64) -> String {
    if value < 1_000 {
        return value.to_string();
    }
    if value < 1_000_000 {
        #[allow(clippy::cast_precision_loss)]
        let thousands = value as f64 / 1_000.0;
        return if thousands < 10.0 {
            format_sub_ten(thousands, 'k')
        } else {
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            let rounded = thousands.round() as u64;
            format!("{rounded}k")
        };
    }
    if value < 1_000_000_000 {
        #[allow(clippy::cast_precision_loss)]
        let millions = value as f64 / 1_000_000.0;
        return if millions < 10.0 {
            format_sub_ten(millions, 'M')
        } else {
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            let rounded = millions.round() as u64;
            format!("{rounded}M")
        };
    }
    #[allow(clippy::cast_precision_loss)]
    let billions = value as f64 / 1_000_000_000.0;
    if billions < 10.0 {
        format_sub_ten(billions, 'B')
    } else {
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let rounded = billions.round() as u64;
        format!("{rounded}B")
    }
}

fn format_sub_ten(value: f64, suffix: char) -> String {
    // Rust formatting and JavaScript `toFixed` differ only on exact halfway
    // values: Rust uses ties-to-even, while JS chooses the larger decimal.
    // At one decimal place, the only positive binary-exact halfway values
    // have a `.25` or `.75` fraction. `.75` already rounds upward under both
    // rules; `.25` needs the explicit JS result.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let formatted = if value.fract().to_bits() == 0.25_f64.to_bits() {
        format!("{}.3", value.trunc() as u64)
    } else {
        format!("{value:.1}")
    };
    let trimmed = formatted.strip_suffix(".0").unwrap_or(&formatted);
    format!("{trimmed}{suffix}")
}

/// Typed diff for one file, mirroring `ConversationItem["diff"]` plus the
/// aggregate `partial` extension from `file-change-groups.ts`.
///
/// `Unknown` in the TypeScript union is `kind: "unavailable"` and the
/// aggregate extension adds `kind: "partial"` with `unavailable_files`.
/// Zero counts are valid `Known` values and remain visible.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum DiffStat {
    /// Known line counts, including `0` for either side.
    Known {
        /// Added lines.
        additions: u64,
        /// Deleted lines.
        deletions: u64,
    },
    /// Line counts are unavailable for this file or aggregate.
    Unavailable,
    /// Some files contributed known counts while others were unavailable.
    ///
    /// `unavailable_files` counts how many aggregated entries were unavailable
    /// or, for nested partials, the transitive unavailable count. It is always
    /// `> 0` when this variant is constructed by [`aggregate_diff_stats`].
    Partial {
        /// Sum of known additions.
        additions: u64,
        /// Sum of known deletions.
        deletions: u64,
        /// Number of files whose counts were unavailable.
        unavailable_files: u64,
    },
}

impl DiffStat {
    /// Whether this diff should be rendered at all.
    ///
    /// Mirrors `aggregate_diff.kind !== "unavailable"` and
    /// `file.diff.kind !== "unavailable"` in
    /// `conversation-changes-card.svelte`. Only `Unavailable` is hidden.
    #[must_use]
    pub const fn is_visible(self) -> bool {
        !matches!(self, Self::Unavailable)
    }

    /// Whether this aggregate is partial and should show the `≥` prefix.
    #[must_use]
    pub const fn is_partial(self) -> bool {
        matches!(self, Self::Partial { .. })
    }

    /// Raw additions if visible, else `None`.
    #[must_use]
    pub const fn additions(self) -> Option<u64> {
        match self {
            Self::Known { additions, .. } | Self::Partial { additions, .. } => Some(additions),
            Self::Unavailable => None,
        }
    }

    /// Raw deletions if visible, else `None`.
    #[must_use]
    pub const fn deletions(self) -> Option<u64> {
        match self {
            Self::Known { deletions, .. } | Self::Partial { deletions, .. } => Some(deletions),
            Self::Unavailable => None,
        }
    }

    /// Number of unavailable files if partial, else `None`.
    #[must_use]
    pub const fn unavailable_files(self) -> Option<u64> {
        match self {
            Self::Partial {
                unavailable_files, ..
            } => Some(unavailable_files),
            Self::Known { .. } | Self::Unavailable => None,
        }
    }

    /// Exact aria label used by `conversation-changes-card.svelte`.
    ///
    /// Returns `None` when the diff is `Unavailable` (the row is hidden).
    /// For `Known` it returns `"{additions} additions, {deletions} deletions"`.
    /// For `Partial` it returns
    /// `"At least {add} additions and {del} deletions; line counts unavailable for {n} change(s)"`
    /// with singular `change` when `n == 1` and plural `changes` otherwise,
    /// matching the TypeScript template exactly.
    #[must_use]
    pub fn label(self) -> Option<String> {
        match self {
            Self::Unavailable => None,
            Self::Known {
                additions,
                deletions,
            } => Some(format!("{additions} additions, {deletions} deletions")),
            Self::Partial {
                additions,
                deletions,
                unavailable_files,
            } => {
                let noun = if unavailable_files == 1 {
                    "change"
                } else {
                    "changes"
                };
                Some(format!(
                    "At least {additions} additions and {deletions} deletions; line counts unavailable for {unavailable_files} {noun}"
                ))
            }
        }
    }

    /// Compact display pair for the two statistic columns.
    ///
    /// Returns `None` when unavailable. Otherwise returns the compact
    /// formatted additions and deletions via [`format_compact_diff_count`],
    /// which the Svelte template renders as `+{additions}` and `-{deletions}`.
    #[must_use]
    pub fn compact_pair(self) -> Option<(String, String)> {
        match self {
            Self::Known {
                additions,
                deletions,
            }
            | Self::Partial {
                additions,
                deletions,
                ..
            } => Some((
                format_compact_diff_count(additions),
                format_compact_diff_count(deletions),
            )),
            Self::Unavailable => None,
        }
    }
}

/// Aggregates per-file diffs into the card-level diff, keeping incomplete
/// totals visibly honest.
///
/// Mirrors `aggregate_file_change_diff` in `file-change-groups.ts` exactly:
/// - Empty input yields [`DiffStat::Unavailable`].
/// - Counts of `Unavailable` entries are tracked as `unavailable_files`.
/// - `Partial` entries contribute their known counts and their transitive
///   `unavailable_files`.
/// - If no entry contributed counts (`has_counts == false`), the result is
///   `Unavailable` even when `unavailable_files > 0`.
/// - Otherwise, when `unavailable_files > 0`, the result is
///   [`DiffStat::Partial`]; else [`DiffStat::Known`].
/// - Ordering of the input is preserved for presentation grouping but does
///   not affect the numeric sums, which are commutative.
///
/// Sums use saturating `u64` addition. Protocol inputs are bounded far below
/// `u64::MAX`, while saturation keeps malformed synthetic input from wrapping
/// a large visible count back to zero.
#[must_use]
pub fn aggregate_diff_stats(diffs: &[DiffStat]) -> DiffStat {
    if diffs.is_empty() {
        return DiffStat::Unavailable;
    }
    let mut additions: u64 = 0;
    let mut deletions: u64 = 0;
    let mut unavailable_files: u64 = 0;
    let mut has_counts = false;
    for diff in diffs {
        match *diff {
            DiffStat::Unavailable => {
                unavailable_files = unavailable_files.saturating_add(1);
            }
            DiffStat::Known {
                additions: a,
                deletions: d,
            } => {
                has_counts = true;
                additions = additions.saturating_add(a);
                deletions = deletions.saturating_add(d);
            }
            DiffStat::Partial {
                additions: a,
                deletions: d,
                unavailable_files: u,
            } => {
                has_counts = true;
                additions = additions.saturating_add(a);
                deletions = deletions.saturating_add(d);
                unavailable_files = unavailable_files.saturating_add(u);
            }
        }
    }
    if !has_counts {
        return DiffStat::Unavailable;
    }
    if unavailable_files > 0 {
        DiffStat::Partial {
            additions,
            deletions,
            unavailable_files,
        }
    } else {
        DiffStat::Known {
            additions,
            deletions,
        }
    }
}
