//! Root render assembly for [`NativeApplication`].
//!
//! Extracted verbatim from `native_application.rs` during the phase-5 module
//! split; trait-impl visibility is unchanged.

use super::*;

impl Render for NativeApplication {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.sync_composer_controls(cx);
        self.sync_profile_actions();
        let sidebar = self.desktop_sidebar(window, cx).into_any_element();
        let body = self.desktop_route_body(window, cx);
        let brand = self.desktop_brand(cx).into_any_element();
        let header = self
            .desktop_header_cluster(cx)
            .unwrap_or_else(|| div().into_any_element());
        // The modal belongs to the full window, not the clipped titlebar.
        let search = div().into_any_element();
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
            .on_action(
                cx.listener(|app, _: &OpenMachines, window, cx| app.open_machines(window, cx)),
            )
            .on_action(|_: &ToggleFrameCounter, window, cx| {
                let visible = !crate::native_frame_rate::overlay_visible(cx);
                if let Err(error) = crate::native_frame_rate::apply_overlay(visible, window, cx) {
                    eprintln!("{error}");
                }
            })
            .size_full()
            .debug_selector(|| NATIVE_ROOT_SELECTOR.to_string())
            .relative()
            .child(shell)
            .child(self.command_menu.clone())
            .children(self.machine_error.clone().map(|message| {
                div()
                    .id("machine-error")
                    .absolute()
                    .top_8()
                    .right_4()
                    .p_3()
                    .bg(self.desktop_theme.chrome)
                    .text_color(self.desktop_theme.foreground)
                    .child(message)
                    .child("  ×")
                    .on_click(cx.listener(|app, _, _, cx| {
                        app.machine_error = None;
                        cx.notify();
                    }))
            }))
            .child(self.message_images.clone())
            .child(self.machine_dropdown(window, cx))
    }
}
