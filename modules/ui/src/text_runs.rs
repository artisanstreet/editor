//! Shared `TextRun` compiler for selectable text.
//!
//! The vendor `StyledText::with_font_family_overrides` recolors existing
//! runs without splitting them, so it cannot give code ranges their own
//! family and spacing while keeping the caller's highlight weights and the
//! selection wash. This module is the one shared compiler instead: it
//! unions highlight and font-override boundaries and emits exact-coverage
//! [`TextRun`](gpui::TextRun)s from the inherited window style. Both the
//! prose worker (`markdown_renderer`/theme/fonts) and the renderer worker
//! (`inline_code_text` + conversation surface) use this function; no second
//! typography compiler is allowed.
//!
//! Re-exported through [`crate::selectable_text`] so existing consumers
//! keep one import path.

use std::ops::Range;

use gpui::{HighlightStyle, Pixels, SharedString, TextRun, TextStyle, combine_highlights};

/// Per-range font family and letter-spacing override for [`compile_text_runs`].
///
/// Both fields are `None`-inherits / `Some`-replaces: `font_family: None`
/// keeps the inherited family, `Some(family)` replaces it for the whole
/// range; `letter_spacing: None` keeps the inherited spacing,
/// `Some(spacing)` replaces it — including `Some(px(0.0))`, which is how
/// mono/code ranges normalize to zero tracking. Code weight travels in the
/// highlight ranges and the body weight stays inherited, so neither lives
/// here. There is deliberately no per-range font size: GPUI `TextLayout`
/// lays out at one uniform size.
#[derive(Clone, Debug, PartialEq)]
pub struct TextRunOverride {
    /// Byte range addressed against the rendered text.
    pub range: Range<usize>,
    /// Replacement font family for the range, if any.
    pub font_family: Option<SharedString>,
    /// Replacement letter spacing for the range, if any.
    pub letter_spacing: Option<Pixels>,
}

/// Compiles highlight and font-override ranges into exact-coverage [`TextRun`]s.
///
/// The caller merges the selection wash into `highlights` first (see
/// [`crate::selectable_text::merge_selection_highlight`], which only
/// touches foreground/background), then this function unions the
/// highlight and override boundaries and emits one run per atomic
/// segment as `default.highlight(segment).to_run(len)` with the active
/// family and spacing overrides applied. Adjacent runs with identical
/// shaping properties are coalesced.
///
/// Highlight contract: callers pass sorted, non-overlapping highlight
/// ranges, as produced by the Markdown seam and by
/// [`crate::selectable_text::merge_selection_highlight`]. Anything else
/// is still accepted without panicking: unsorted input is normalized by
/// position, and overlapping ranges resolve through the existing vendor
/// [`combine_highlights`](gpui::combine_highlights) sweep. Overlaps that
/// agree on discrete properties (weight, style) merge deterministically;
/// overlaps with conflicting discrete properties resolve in vendor fold
/// order, so which one wins is unspecified — keep conflicting highlight
/// ranges disjoint.
///
/// Guarantees: run lengths sum to `text.len()` so plaintext and copied
/// bytes stay exact; every boundary is a character boundary; weights,
/// styles, underlines, and link effects survive both overrides and
/// selection, so glyph metrics never change while selecting.
///
/// Fail-closed and never panics on caller ranges: a range is used only
/// when it is ordered, fully in-bounds, and has both endpoints on
/// character boundaries — anything else (reversed, empty, out-of-bounds,
/// or mid-character) is dropped entirely, never clamped, so a bad caller
/// range cannot shift surviving text. Overlapping overrides keep the
/// earliest range and drop the later one after sorting by start. Empty
/// text yields zero runs.
#[must_use]
pub fn compile_text_runs(
    text: &str,
    default: &TextStyle,
    highlights: &[(Range<usize>, HighlightStyle)],
    overrides: &[TextRunOverride],
) -> Vec<TextRun> {
    if text.is_empty() {
        return Vec::new();
    }
    let sanitized: Vec<(Range<usize>, HighlightStyle)> = highlights
        .iter()
        .filter(|(range, _)| is_usable_range(text, range))
        .map(|(range, style)| (range.clone(), *style))
        .collect();
    let normalized: Vec<(Range<usize>, HighlightStyle)> =
        combine_highlights(sanitized, Vec::new()).collect();

    let disjoint = disjoint_overrides(text, overrides);

    let mut bounds = Vec::with_capacity(normalized.len() * 2 + disjoint.len() * 2 + 2);
    bounds.extend([0, text.len()]);
    for (range, _) in &normalized {
        bounds.push(range.start);
        bounds.push(range.end);
    }
    for (range, _, _) in &disjoint {
        bounds.push(range.start);
        bounds.push(range.end);
    }
    bounds.sort_unstable();
    bounds.dedup();

    let mut runs: Vec<TextRun> = Vec::new();
    let mut highlight_ix = 0_usize;
    let mut override_ix = 0_usize;
    for window in bounds.windows(2) {
        let &[start, end] = window else {
            continue;
        };
        if start >= end {
            continue;
        }
        while normalized
            .get(highlight_ix)
            .is_some_and(|(range, _)| range.end <= start)
        {
            highlight_ix += 1;
        }
        while disjoint
            .get(override_ix)
            .is_some_and(|(range, _, _)| range.end <= start)
        {
            override_ix += 1;
        }
        let active_highlight = match normalized.get(highlight_ix) {
            Some((range, style)) if range.start <= start && start < range.end => Some(*style),
            _ => None,
        };
        let (active_family, active_spacing) = match disjoint.get(override_ix) {
            Some((range, family, spacing)) if range.start <= start && start < range.end => {
                (family.clone(), *spacing)
            }
            _ => (None, None),
        };
        let mut style = match active_highlight {
            Some(highlight) => default.clone().highlight(highlight),
            None => default.clone(),
        };
        if let Some(family) = active_family {
            style.font_family = family;
        }
        if let Some(spacing) = active_spacing {
            style.letter_spacing = Some(spacing);
        }
        let run = style.to_run(end - start);
        if let Some(last) = runs.last_mut()
            && runs_equal(last, &run)
        {
            last.len += run.len;
            continue;
        }
        runs.push(run);
    }
    runs
}

/// Sanitizes caller override ranges, sorts them by start, and keeps the
/// earliest of any overlapping ranges.
fn disjoint_overrides(
    text: &str,
    overrides: &[TextRunOverride],
) -> Vec<(Range<usize>, Option<SharedString>, Option<Pixels>)> {
    let mut cleaned: Vec<(Range<usize>, Option<SharedString>, Option<Pixels>)> = overrides
        .iter()
        .filter(|override_value| is_usable_range(text, &override_value.range))
        .map(|override_value| {
            (
                override_value.range.clone(),
                override_value.font_family.clone(),
                override_value.letter_spacing,
            )
        })
        .collect();
    cleaned.sort_by(|left, right| {
        left.0
            .start
            .cmp(&right.0.start)
            .then_with(|| left.0.end.cmp(&right.0.end))
    });
    let mut disjoint: Vec<(Range<usize>, Option<SharedString>, Option<Pixels>)> =
        Vec::with_capacity(cleaned.len());
    for item in cleaned {
        let overlaps = disjoint
            .last()
            .is_some_and(|last| item.0.start < last.0.end);
        if !overlaps {
            disjoint.push(item);
        }
    }
    disjoint
}

/// Returns whether `range` is usable against `text` exactly as given.
///
/// Ordered, fully in-bounds, with both endpoints on character
/// boundaries. Anything else is dropped entirely by the caller — never
/// clamped — so invalid ranges cannot shift surviving text.
fn is_usable_range(text: &str, range: &Range<usize>) -> bool {
    range.start < range.end
        && range.end <= text.len()
        && text.is_char_boundary(range.start)
        && text.is_char_boundary(range.end)
}

/// Returns whether two runs shape identically apart from their length.
fn runs_equal(left: &TextRun, right: &TextRun) -> bool {
    left.font == right.font
        && left.color == right.color
        && left.background_color == right.background_color
        && left.underline == right.underline
        && left.strikethrough == right.strikethrough
        && left.letter_spacing == right.letter_spacing
}
