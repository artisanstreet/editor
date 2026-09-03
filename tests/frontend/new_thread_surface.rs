//! Mounted coverage for the native new-thread surface.
//!
//! A probe view renders the sentence heading plus recent rows; tests pin
//! the stable selectors and the empty state through the GPUI harness.

use artisan_frontend::native_new_thread_surface::{
    NEW_THREAD_EMPTY_RECENTS, NEW_THREAD_RECENTS_SELECTOR, NEW_THREAD_SENTENCE_SELECTOR,
    NEW_THREAD_SURFACE_SELECTOR, NewThreadRecentRow, recent_row_selector,
    render_new_thread_surface,
};
use artisan_ui::theme::{ArtisanTheme, ThemeMode};
use gpui::{
    Context, IntoElement, ParentElement as _, Render, Styled as _, TestAppContext, Window, div,
};

struct SurfaceProbe {
    sentence: String,
    rows: Vec<NewThreadRecentRow>,
}

impl Render for SurfaceProbe {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let theme = ArtisanTheme::for_mode(ThemeMode::Light);
        div().size_full().child(render_new_thread_surface(
            theme,
            &self.sentence,
            &self.rows,
            NEW_THREAD_SURFACE_SELECTOR,
        ))
    }
}

fn rows() -> Vec<NewThreadRecentRow> {
    vec![
        NewThreadRecentRow::new("First thread", "2h", "row-0"),
        NewThreadRecentRow::new("Second thread", "3d", "row-1"),
    ]
}

#[gpui::test]
fn sentence_and_rows_mount_with_stable_selectors(cx: &mut TestAppContext) {
    let (_view, cx) = cx.add_window_view(|_, _| SurfaceProbe {
        sentence: "What are we building today?".to_owned(),
        rows: rows(),
    });
    assert!(cx.debug_bounds(NEW_THREAD_SURFACE_SELECTOR).is_some());
    assert!(cx.debug_bounds(NEW_THREAD_SENTENCE_SELECTOR).is_some());
    assert!(cx.debug_bounds(NEW_THREAD_RECENTS_SELECTOR).is_some());
    assert!(
        cx.debug_bounds("native-new-thread-surface-recent-row-0")
            .is_some()
    );
    assert!(
        cx.debug_bounds("native-new-thread-surface-recent-row-1")
            .is_some()
    );
    assert_eq!(
        recent_row_selector(NEW_THREAD_SURFACE_SELECTOR, "row-0"),
        "native-new-thread-surface-recent-row-0".to_owned()
    );
}

#[gpui::test]
fn empty_recents_show_the_empty_copy(cx: &mut TestAppContext) {
    let (_view, cx) = cx.add_window_view(|_, _| SurfaceProbe {
        sentence: "Where to start?".to_owned(),
        rows: Vec::new(),
    });
    assert!(cx.debug_bounds(NEW_THREAD_SURFACE_SELECTOR).is_some());
    assert!(cx.debug_bounds(NEW_THREAD_SENTENCE_SELECTOR).is_some());
    assert!(cx.debug_bounds(NEW_THREAD_RECENTS_SELECTOR).is_some());
    assert!(
        cx.debug_bounds("native-new-thread-surface-recent-row-0")
            .is_none()
    );
    assert!(!NEW_THREAD_EMPTY_RECENTS.is_empty());
}
