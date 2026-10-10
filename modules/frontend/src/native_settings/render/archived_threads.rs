//! The Threads page's archive: a table of archived threads, the soonest to
//! be erased first, each with when it was archived, when it is erased, and
//! a button that restores it to the sidebar.

use artisan_domain::{ARCHIVE_RETENTION_MS, ArchivedThread, UnixMillis};

use super::chrome::{settings_card, settings_section_shell};
use super::*;
use crate::native_settings::archived_threads::span_until;

/// Widths of the table's fixed columns.
const ARCHIVED_COLUMN_PX: f32 = 112.0;
const ERASED_COLUMN_PX: f32 = 152.0;
const ACTION_COLUMN_PX: f32 = 96.0;

/// Below this, the erasure countdown reads in the destructive colour.
const ERASE_WARNING_MS: i64 = 3 * 24 * 60 * 60 * 1_000;

impl SettingsScreen {
    /// Renders the archived threads section.
    pub(super) fn render_archived_threads(
        &self,
        theme: &artisan_ui::theme::ArtisanTheme,
        cx: &mut Context<Self>,
    ) -> Div {
        let days = ARCHIVE_RETENTION_MS / (24 * 60 * 60 * 1_000);
        let intro = format!(
            "Archived threads leave the sidebar. Each is erased for good {days} days after you archive it, unless you restore it."
        );
        let page = &self.archived_threads;
        let muted = |text: String| {
            div()
                .w_full()
                .py(px(16.0))
                .text_sm()
                .text_color(theme.colors.muted_foreground.to_paint())
                .child(text)
        };
        let body = match (&page.notice, &page.listing) {
            (Some(notice), _) => settings_card(theme, vec![muted(notice.clone())]),
            (None, None) => settings_card(
                theme,
                vec![muted("Loading archived threads\u{2026}".to_owned())],
            ),
            (None, Some(listing)) if listing.threads().is_empty() => {
                settings_card(theme, vec![muted("No archived threads.".to_owned())])
            }
            (None, Some(listing)) => {
                let now = wall_clock();
                let mut blocks = vec![archive_header(theme)];
                blocks.extend(
                    listing
                        .threads()
                        .iter()
                        .map(|row| self.archived_row(theme, row, now, cx)),
                );
                settings_card(theme, blocks)
            }
        };
        settings_section_shell(theme, "archived", "Archived", Some(&intro), None, body)
    }

    fn archived_row(
        &self,
        theme: &artisan_ui::theme::ArtisanTheme,
        row: &ArchivedThread,
        now: UnixMillis,
        cx: &mut Context<Self>,
    ) -> Div {
        let thread_id = row.thread.thread.thread_id.clone();
        let selector = format!("settings-archived-{}", thread_id.as_str());
        let restoring = self.archived_threads.restoring.contains(&thread_id);
        let erases_at = row.erases_at();
        let soon = erases_at.as_millis().saturating_sub(now.as_millis()) < ERASE_WARNING_MS;
        let countdown = if soon {
            theme.colors.destructive.to_paint()
        } else {
            theme.colors.muted_foreground.to_paint()
        };
        let button = Button::new(
            SharedString::from(format!("{selector}-unarchive")),
            self.focus.control.clone(),
            *theme,
            MotionPolicy::Reduced,
            ButtonVariant::Outline,
            ButtonSize::Small,
            ButtonContent::text(if restoring { "Restoring" } else { "Unarchive" }),
        )
        .ok()
        .map(|button| {
            button
                .disabled(restoring)
                .debug_selector(format!("{selector}-unarchive"))
                .on_activate(cx.listener(move |_, _, _, cx| {
                    cx.emit(SettingsScreenEvent::UnarchiveThread {
                        thread_id: thread_id.clone(),
                    });
                }))
        });
        div()
            .w_full()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(16.0))
            .py(px(10.0))
            .debug_selector(move || selector.clone())
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(px(2.0))
                    .child(
                        div()
                            .truncate()
                            .text_sm()
                            .text_color(theme.colors.foreground.to_paint())
                            .child(row.thread.thread.title.as_str().to_owned()),
                    )
                    .child(
                        div()
                            .truncate()
                            .text_xs()
                            .text_color(theme.colors.muted_foreground.to_paint())
                            .child(row.thread.subtitle.as_str().to_owned()),
                    ),
            )
            .child(
                div()
                    .w(px(ARCHIVED_COLUMN_PX))
                    .flex_shrink_0()
                    .text_sm()
                    .text_color(theme.colors.muted_foreground.to_paint())
                    .child(format!("{} ago", span_until(row.archived_at, now))),
            )
            .child(
                div()
                    .w(px(ERASED_COLUMN_PX))
                    .flex_shrink_0()
                    .text_sm()
                    .text_color(countdown)
                    .child(format!("Deleting in {}", span_until(now, erases_at))),
            )
            .child(
                div()
                    .w(px(ACTION_COLUMN_PX))
                    .flex_shrink_0()
                    .flex()
                    .justify_end()
                    .children(button),
            )
    }
}

/// The table's column headings.
fn archive_header(theme: &artisan_ui::theme::ArtisanTheme) -> Div {
    let heading = |text: &'static str| {
        div()
            .text_xs()
            .font_weight(FontWeight::MEDIUM)
            .text_color(theme.colors.muted_foreground.to_paint())
            .child(text)
    };
    div()
        .w_full()
        .flex()
        .flex_row()
        .items_center()
        .gap(px(16.0))
        .py(px(8.0))
        .debug_selector(|| "settings-archived-header".to_owned())
        .child(div().flex_1().min_w_0().child(heading("Thread")))
        .child(
            div()
                .w(px(ARCHIVED_COLUMN_PX))
                .flex_shrink_0()
                .child(heading("Archived")),
        )
        .child(
            div()
                .w(px(ERASED_COLUMN_PX))
                .flex_shrink_0()
                .child(heading("Deletion")),
        )
        .child(div().w(px(ACTION_COLUMN_PX)).flex_shrink_0())
}

/// The wall clock the countdowns are measured against.
fn wall_clock() -> UnixMillis {
    let millis = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| {
            i64::try_from(elapsed.as_millis()).unwrap_or(i64::MAX)
        });
    UnixMillis::from_millis(millis)
}
