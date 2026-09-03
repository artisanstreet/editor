//! New-thread surface composition: sentence heading plus recent rows.
//!
//! Native counterpart of the `new-thread-route.svelte` surface minus its
//! Effect controllers and streams, which the application owns: given a
//! resolved sentence and caller-formatted recent rows, this module renders
//! the centered surface. The composer is mounted separately by the
//! application; data fetching, draft control, and navigation stay out.

use artisan_ui::list_row::{
    ListRowContent, ListRowGeometry, ListRowSlots, ListRowStyle, ListRowTone, list_row,
};
use artisan_ui::theme::ArtisanTheme;
use gpui::{
    Div, FontWeight, InteractiveElement as _, SharedString, div,
    prelude::{ParentElement as _, Styled as _},
};

/// Debug selector for the surface root.
pub const NEW_THREAD_SURFACE_SELECTOR: &str = "native-new-thread-surface";
/// Debug selector for the sentence heading.
pub const NEW_THREAD_SENTENCE_SELECTOR: &str = "native-new-thread-sentence";
/// Debug selector for the recents list container.
pub const NEW_THREAD_RECENTS_SELECTOR: &str = "native-new-thread-recents";
/// Muted copy shown when there are no recent threads.
pub const NEW_THREAD_EMPTY_RECENTS: &str = "No recent threads yet.";

/// One caller-formatted recent-thread row.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NewThreadRecentRow {
    /// Display title for the row.
    pub title: SharedString,
    /// Relative-time detail (already formatted by the caller).
    pub detail: SharedString,
    /// Stable per-row selector suffix.
    pub selector_suffix: String,
}

impl NewThreadRecentRow {
    /// Builds a row from display parts.
    #[must_use]
    pub fn new(
        title: impl Into<SharedString>,
        detail: impl Into<SharedString>,
        selector_suffix: impl Into<String>,
    ) -> Self {
        Self {
            title: title.into(),
            detail: detail.into(),
            selector_suffix: selector_suffix.into(),
        }
    }
}

/// Builds the stable selector for one recent row under `root`.
#[must_use]
pub fn recent_row_selector(root: &str, suffix: &str) -> String {
    format!("{root}-recent-{suffix}")
}

/// Renders the centered new-thread surface: sentence heading plus recent
/// rows (or the empty-recents copy). Resolves all paint, spacing, and
/// typography from the one shared theme argument.
#[must_use]
pub fn render_new_thread_surface(
    theme: ArtisanTheme,
    sentence: &str,
    rows: &[NewThreadRecentRow],
    root_selector: &str,
) -> Div {
    let row_style = ListRowStyle::resolve(
        theme,
        ListRowGeometry::Rail,
        ListRowTone::Foreground,
        FontWeight::MEDIUM,
    );
    let sentence_row = div()
        .text_size(theme.typography.dialog_title_text)
        .text_color(theme.colors.foreground.to_paint())
        .debug_selector(|| NEW_THREAD_SENTENCE_SELECTOR.to_string())
        .child(SharedString::from(sentence.to_owned()));

    let mut recents = div()
        .flex()
        .flex_col()
        .w_full()
        .debug_selector(|| NEW_THREAD_RECENTS_SELECTOR.to_string());
    if rows.is_empty() {
        recents = recents.child(
            div()
                .text_size(theme.typography.label_text)
                .text_color(theme.colors.muted_foreground.to_paint())
                .child(NEW_THREAD_EMPTY_RECENTS),
        );
    }
    for row in rows {
        let selector = recent_row_selector(root_selector, &row.selector_suffix);
        recents = recents.child(
            list_row(
                row_style,
                ListRowContent::one_line(row.title.clone()),
                ListRowSlots::new().trailing_caption(row.detail.clone()),
            )
            .debug_selector(move || selector.clone()),
        );
    }

    div()
        .flex()
        .flex_col()
        .items_center()
        .w_full()
        .gap(theme.spacing.steps(4.0))
        .py(theme.spacing.steps(6.0))
        .debug_selector(|| root_selector.to_owned())
        .child(sentence_row)
        .child(recents)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn row_selectors_nest_under_root() {
        assert_eq!(
            recent_row_selector(NEW_THREAD_SURFACE_SELECTOR, "abc"),
            "native-new-thread-surface-recent-abc".to_owned()
        );
    }

    #[test]
    fn rows_retain_display_parts() {
        let row = NewThreadRecentRow::new("Title", "2h", "row-0");
        assert_eq!(row.title.as_ref(), "Title");
        assert_eq!(row.detail.as_ref(), "2h");
        assert_eq!(row.selector_suffix, "row-0");
    }

    #[test]
    fn empty_copy_is_stable() {
        assert!(!NEW_THREAD_EMPTY_RECENTS.is_empty());
    }
}
