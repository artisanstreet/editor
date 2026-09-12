//! Render budgets for the native transcript surface.
//!
//! A conversation transcript is unbounded while the viewport that shows it is
//! bounded. Building every accepted turn on every frame makes per-frame cost
//! scale with the transcript instead of the window: turn layout, Markdown
//! parsing, rich-link probing, image-tile bookkeeping, and element-tree
//! allocation all grow with the conversation. This module owns the explicit
//! budgets that keep one frame bounded, in one place, so they can be reviewed
//! and tested without reading the render path.
//!
//! # Budgets
//!
//! - [`TRANSCRIPT_OVERSCAN_ROWS`] — turn rows kept built above and below the
//!   visible window. Overscan absorbs estimation error and small scrolls
//!   without rebuilding the window on the next frame. Every overscan row is a
//!   real element subtree (layout, shaping, prepaint), so this trades frame
//!   time for scroll smoothness. Four rows covers a coarse wheel notch at
//!   prose heights while keeping the built set far below the hard cap.
//! - [`TRANSCRIPT_MAX_BUILT_ROWS`] — hard cap on the viewport-derived window
//!   (viewport + overscan). Without it, a tall window over short rows
//!   (one-line messages, ticks) could ask for hundreds of rows and
//!   reintroduce per-frame cost proportional to the transcript. 48 rows is
//!   roughly four flat-screen viewports of prose; the cap trims the window
//!   and the next scroll re-centers it. One extra row may be force-built
//!   outside the window for a pending scroll target, so the absolute
//!   per-frame bound is this cap plus one.
//! - [`TRANSCRIPT_MAX_MARKDOWN_BYTES_PER_ROW`] — per-row Markdown shaping
//!   budget. The shared renderer parses synchronously during render, so one
//!   pathological body would cost a full document parse every frame it is
//!   visible. Bodies at or below this bound shape as Markdown; larger bodies
//!   keep their exact full text through the plain selectable fallback instead
//!   of being parsed. The bound mirrors [`SCENE_MAX_MESSAGE_BODY_BYTES`], the
//!   accepted-scene ceiling, so accepted scenes never take the fallback; the
//!   guard keeps a future scene regression from multiplying parse cost.
//! - [`TRANSCRIPT_UNMEASURED_ROW_HEIGHT_PX`], [`TRANSCRIPT_MIN_ROW_HEIGHT_PX`],
//!   [`TRANSCRIPT_MAX_ROW_HEIGHT_PX`] — the height model for rows that have
//!   never been measured. Windowing runs before layout, so it plans from
//!   remembered heights with a bounded fallback; min/max keep one pathological
//!   measurement from poisoning the running average used for the rest.
//! - [`TRANSCRIPT_TURN_GAP_PX`] — the inter-turn gap the offsets model adds
//!   between rows (`gap-8` on the 4 px spacing base).
//!
//! # Test seams
//!
//! [`plan_transcript_window`] is pure over row offsets and viewport geometry,
//! so overscan, tail anchoring, and the row cap are tested without a window.
//! [`transcript_markdown_within_budget`] is the pure per-row shaping decision.

use std::ops::Range;

use crate::conversation_scene::SCENE_MAX_MESSAGE_BODY_BYTES;

/// Turn rows kept built above and below the visible window.
pub(super) const TRANSCRIPT_OVERSCAN_ROWS: usize = 4;

/// Hard cap on turn rows built by one frame (viewport + overscan).
pub(super) const TRANSCRIPT_MAX_BUILT_ROWS: usize = 48;

/// Inter-turn gap in px (`gap-8` on the 4 px spacing base).
pub(super) const TRANSCRIPT_TURN_GAP_PX: f64 = 32.0;

/// Fallback height in px for a turn with no measurement yet.
pub(super) const TRANSCRIPT_UNMEASURED_ROW_HEIGHT_PX: f64 = 160.0;

/// Lower clamp for one measured turn height in the running estimate.
pub(super) const TRANSCRIPT_MIN_ROW_HEIGHT_PX: f64 = 1.0;

/// Upper clamp for one measured turn height in the running estimate.
pub(super) const TRANSCRIPT_MAX_ROW_HEIGHT_PX: f64 = 4_096.0;

/// Per-row Markdown shaping budget in UTF-8 bytes.
pub(super) const TRANSCRIPT_MAX_MARKDOWN_BYTES_PER_ROW: usize = SCENE_MAX_MESSAGE_BODY_BYTES;

/// Whether one body fits the per-row Markdown shaping budget.
#[must_use]
pub(super) const fn transcript_markdown_within_budget(bytes: usize) -> bool {
    bytes <= TRANSCRIPT_MAX_MARKDOWN_BYTES_PER_ROW
}

/// One planned viewport window over the transcript's turn rows.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct TranscriptWindowPlan {
    /// Half-open range of turn indices the window requests.
    pub(super) built: Range<usize>,
    /// Rows the viewport and overscan requested before the hard cap.
    pub(super) desired_rows: usize,
    /// Whether the hard row cap trimmed the requested window.
    pub(super) capped: bool,
}

/// Plans the transcript window from row offsets and viewport geometry.
///
/// `offsets[i]` is the top of turn `i` including the gaps that precede it,
/// and `offsets[rows]` is the total estimated content height, so `offsets`
/// has `rows + 1` entries. This is the single place the overscan and row-cap
/// budgets apply: callers cannot build a window larger than
/// [`TRANSCRIPT_MAX_BUILT_ROWS`].
///
/// Tail anchoring is explicit and bounded: when the caller reports the reader
/// at the content bottom, the window walks back from the final row just far
/// enough to cover the viewport (plus overscan) instead of trusting the
/// offsets of rows above. The walk touches only the rows it builds.
#[must_use]
pub(super) fn plan_transcript_window(
    offsets: &[f64],
    viewport_top: f64,
    viewport_height: f64,
    tail_anchored: bool,
) -> TranscriptWindowPlan {
    let rows = offsets.len().saturating_sub(1);
    if rows == 0 {
        return TranscriptWindowPlan {
            built: 0..0,
            desired_rows: 0,
            capped: false,
        };
    }

    let (mut start, mut end);
    if tail_anchored {
        end = rows;
        start = rows;
        let mut covered = 0.0_f64;
        while start > 0 {
            covered += offsets[start] - offsets[start - 1];
            start -= 1;
            if covered > viewport_height {
                break;
            }
        }
        start = start.saturating_sub(TRANSCRIPT_OVERSCAN_ROWS);
    } else {
        let top = viewport_top.max(0.0);
        let bottom = top + viewport_height.max(0.0);
        // The row containing the viewport top: the last row whose top is at
        // or above it. `partition_point` is exact on the sorted offsets.
        let first = offsets
            .partition_point(|offset| *offset <= top)
            .saturating_sub(1)
            .min(rows - 1);
        // The first row whose top is at or below the viewport bottom.
        let after_last = offsets.partition_point(|offset| *offset < bottom);
        let last = after_last.max(first + 1).min(rows);
        start = first.saturating_sub(TRANSCRIPT_OVERSCAN_ROWS);
        end = (last + TRANSCRIPT_OVERSCAN_ROWS).min(rows);
    }

    let desired_rows = end - start;
    let capped = desired_rows > TRANSCRIPT_MAX_BUILT_ROWS;
    if capped {
        // Trim from the far side so the reader's anchor stays covered: a
        // tail-anchored window keeps its last rows, a viewport window keeps
        // the rows at the viewport top.
        if tail_anchored {
            end = rows;
            start = rows.saturating_sub(TRANSCRIPT_MAX_BUILT_ROWS);
        } else {
            end = (start + TRANSCRIPT_MAX_BUILT_ROWS).min(rows);
            start = end.saturating_sub(TRANSCRIPT_MAX_BUILT_ROWS);
        }
    }
    TranscriptWindowPlan {
        built: start..end,
        desired_rows,
        capped,
    }
}
