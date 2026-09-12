//! Root render assembly for [`NativeApplication`].
//!
//! Extracted verbatim from `native_application.rs` during the phase-5 module
//! split; trait-impl visibility is unchanged.

use super::*;

impl Render for NativeApplication {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.sync_composer_controls(cx);
        let sidebar = self.desktop_sidebar(window, cx).into_any_element();
        let body = self.desktop_route_body(window, cx);
        let brand = self.desktop_brand(cx).into_any_element();
        let header = self
            .desktop_header_cluster(cx)
            .unwrap_or_else(|| div().into_any_element());
        let search = self.command_menu.clone().into_any_element();
        let shell = desktop_shell(
            self.desktop_theme,
            self.sidebar_collapsed,
            brand,
            header,
            search,
            sidebar,
            body,
            window.scale_factor(),
            window.is_maximized(),
        );
        div()
            .id("artisan-desktop-application-root")
            .track_focus(&self.focus_handle)
            .key_context(NATIVE_KEY_CONTEXT)
            .on_click(cx.listener(Self::dismiss_command_menu))
            .on_action(|_: &NextTabStop, window, cx| window.focus_next(cx))
            .on_action(|_: &PreviousTabStop, window, cx| window.focus_prev(cx))
            .on_action(cx.listener(Self::activate_command_menu))
            .size_full()
            .debug_selector(|| NATIVE_ROOT_SELECTOR.to_string())
            .relative()
            .child(shell)
            .child(self.message_images.clone())
    }
}
