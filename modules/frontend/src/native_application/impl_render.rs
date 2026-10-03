//! Root render assembly for [`NativeApplication`].
//!
//! Extracted verbatim from `native_application.rs` during the phase-5 module
//! split; trait-impl visibility is unchanged.

use super::*;

impl Render for NativeApplication {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        #[cfg(feature = "flight-recorder")]
        let _trace = artisan_tracing::span!("ui", "application.render");
        self.sync_composer_controls(cx);
        self.sync_model_selector_blocked_engines(cx);
        self.sync_profile_actions();
        // The sidebar fills its shell slot and renders only when notified,
        // so a transcript or composer frame reuses its previous paint.
        let sidebar = crate::view_boundary::cached_view(
            self.sidebar.clone(),
            gpui::StyleRefinement::default().size_full(),
            cx,
        );
        let body = self.desktop_route_body(window, cx);
        // Read after the route body so the thread screen has already taken
        // this frame's gate and content width: the shell's right junction
        // then appears and disappears with the inspector column itself.
        let inspector_width = match self.route() {
            NativeRoute::Thread { .. } => self
                .thread_screen
                .as_ref()
                .and_then(|screen| screen.read(cx).visible_inspector_width()),
            _ => None,
        };
        // The meter shows the same validated reading as the composer's ring:
        // the controls snapshot was synced at the top of this render.
        let context_meter = match self.route() {
            NativeRoute::Thread { .. } => {
                let snapshot = self.composer_controls.read(cx).snapshot();
                snapshot
                    .context_usage
                    .as_ref()
                    .and_then(|usage| usage.presentation(snapshot.run_id.as_deref()))
                    .map(|presentation| {
                        crate::native_context_usage::render_context_meter(
                            &presentation,
                            ArtisanTheme::for_mode(ThemeMode::Dark),
                        )
                        .into_any_element()
                    })
            }
            _ => None,
        };
        let brand = self.desktop_brand(cx).into_any_element();
        let header = self
            .desktop_header_cluster()
            .unwrap_or_else(|| div().into_any_element());
        // The modal belongs to the full window, not the clipped titlebar.
        let search = div().into_any_element();
        let shell = desktop_shell(
            self.desktop_theme,
            self.desktop_shell_style(window),
            brand,
            header,
            search,
            sidebar,
            body,
            inspector_width,
            context_meter,
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
                    let action = if visible { "shown" } else { "hidden" };
                    eprintln!("frame counter overlay could not be {action}: {error}");
                }
            })
            .size_full()
            .debug_selector(|| NATIVE_ROOT_SELECTOR.to_string())
            .relative()
            .child(shell)
            .child(self.command_menu.clone())
            .children(self.window_error.clone().map(|message| {
                div()
                    .id("window-error")
                    .debug_selector(|| "window-error".to_owned())
                    .absolute()
                    .top_8()
                    .right_4()
                    .p_3()
                    .bg(self.desktop_theme.chrome)
                    .text_color(self.desktop_theme.foreground)
                    .child(message)
                    .child("  ×")
                    .on_click(cx.listener(|app, _, _, cx| {
                        app.window_error = None;
                        cx.notify();
                    }))
            }))
            .children(self.host_switch_banner())
            .child(self.message_images.clone())
            .child(self.machine_dropdown(window, cx))
    }
}
