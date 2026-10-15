//! Application-wide Tab traversal through GPUI's native tab stops.
use gpui::{App, KeyBinding, Window};

gpui::actions!(artisan_focus, [FocusNext, FocusPrevious]);

/// Bind once at application setup. Focused text editors can override Tab.
pub fn bind_actions(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("tab", FocusNext, None),
        KeyBinding::new("shift-tab", FocusPrevious, None),
    ]);
}

pub fn focus_next(_: &FocusNext, window: &mut Window, cx: &mut App) {
    window.focus_next(cx);
    window.prevent_default();
    cx.stop_propagation();
}

pub fn focus_previous(_: &FocusPrevious, window: &mut Window, cx: &mut App) {
    window.focus_prev(cx);
    window.prevent_default();
    cx.stop_propagation();
}

#[cfg(test)]
pub(crate) fn fixture_root_focus<T: 'static>(
    window: &mut Window,
    cx: &mut gpui::Context<T>,
) -> gpui::FocusHandle {
    // Standalone fixtures seed a window root just as Workspace::new does.
    // Tab actions then travel through its real native ancestor path.
    let focus = cx.focus_handle();
    window.focus(&focus, cx);
    focus
}
