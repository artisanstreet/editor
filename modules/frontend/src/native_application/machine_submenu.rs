//! Side-anchored machine submenu using the same glass material as the profile menu.
use super::*;

pub(super) fn submenu_bounds(
    trigger: Bounds<gpui::Pixels>,
    parent: Bounds<gpui::Pixels>,
    viewport: gpui::Size<gpui::Pixels>,
    rows: usize,
) -> Bounds<gpui::Pixels> {
    let width = px(300.0).min((viewport.width - px(16.0)).max(px(0.0)));
    let height = px(64.0 * f32::from(u16::try_from(rows).unwrap_or(u16::MAX)) + 8.0)
        .min(px(400.0))
        .min((viewport.height - px(16.0)).max(px(0.0)));
    let right = parent.right() + px(6.0);
    let left = if right + width <= viewport.width - px(8.0) {
        right
    } else {
        parent.left() - width - px(6.0)
    };
    Bounds::new(
        gpui::point(
            left.max(px(8.0))
                .min((viewport.width - width - px(8.0)).max(px(0.0))),
            trigger
                .top()
                .max(px(8.0))
                .min((viewport.height - height - px(8.0)).max(px(0.0))),
        ),
        size(width, height),
    )
}

impl NativeApplication {
    pub(super) fn machine_dropdown(&self, window: &Window, cx: &Context<Self>) -> AnyElement {
        if !self.machine_menu.open {
            return div().into_any_element();
        }
        let trigger = self.machine_menu.bounds.get();
        let parent = self.profile_tip_surface_bounds.borrow().unwrap_or(trigger);
        let bounds = submenu_bounds(
            trigger,
            parent,
            window.viewport_size(),
            self.machine_menu.entries.len(),
        );
        let radius = RadiusTokens::value(RadiusStep::X2l);
        let panel = div()
            .id("machine-dropdown")
            .debug_selector(|| "machine-dropdown".into())
            .absolute()
            .top(bounds.top())
            .left(bounds.left())
            .w(bounds.size.width)
            .max_h(bounds.size.height)
            .font_family(self.theme.typography.body().family)
            .flex()
            .flex_col()
            .p_1()
            .rounded(radius)
            .border_1()
            .border_color(self.desktop_theme.line)
            .backdrop_blur(glass_blur_radius(GlassStrength::Quiet))
            .bg(glass_foreground_base(&self.theme))
            .shadow(glass_card_shadows())
            .child(glass_material_layer(GlassStrength::Quiet, radius))
            .child(glass_highlight_layer(GlassStrength::Quiet, radius))
            .block_mouse_except_scroll()
            .track_focus(&self.machine_menu.focus)
            .on_key_down(cx.listener(Self::handle_machine_key))
            .on_click(cx.listener(|_, _: &ClickEvent, _, cx| cx.stop_propagation()))
            .on_mouse_down_out(
                cx.listener(|app, event: &gpui::MouseDownEvent, window, cx| {
                    if app.machine_menu.bounds.get().contains(&event.position) {
                        return;
                    }
                    app.close_machines(window, cx);
                    if !app
                        .profile_tip_surface_bounds
                        .borrow()
                        .is_some_and(|bounds| bounds.contains(&event.position))
                    {
                        let _ = app.profile_menu.dismiss();
                        app.begin_profile_menu_close(cx);
                        app.profile_focus.focus(window, cx);
                    }
                }),
            );
        gpui::deferred(panel.child(self.machine_rows(cx))).into_any_element()
    }

    fn machine_rows(&self, cx: &Context<Self>) -> AnyElement {
        let surface = Rc::clone(&self.machine_menu.hover_surface);
        let probe = canvas(
            |_, _, _| {},
            move |bounds, (), window, cx| {
                if surface.replace(Some(bounds)) != Some(bounds) {
                    window.defer(cx, |window, _| window.refresh());
                }
            },
        )
        .absolute()
        .top_0()
        .left_0()
        .size_full();
        let mut rows = div()
            .id("machine-submenu-rows")
            .track_scroll(&self.machine_menu.scroll)
            .relative()
            .min_h(px(0.0))
            .overflow_y_scroll()
            .flex()
            .flex_col();
        for (index, entry) in self.machine_menu.entries.iter().enumerate() {
            let mut item = div().flex_shrink_0();
            if matches!(entry.action, CommandMenuAction::AddHost) {
                item = item.child(
                    div()
                        .debug_selector(|| "machine-add-separator".into())
                        .h(px(1.0))
                        .my(px(4.0))
                        .bg(self.theme.colors.foreground.with_alpha(0.2).to_paint()),
                );
            }
            rows = rows.child(item.child(self.machine_row(index, entry, cx)));
        }
        div()
            .id("machine-hover-surface")
            .relative()
            .min_h(px(0.0))
            .flex()
            .flex_col()
            .overflow_hidden()
            .child(probe)
            .child(render_picker_hover_pill(
                &self.theme,
                &self.machine_menu.hover,
                "machines",
                RadiusTokens::value(RadiusStep::Xl),
                cx.reduce_motion(),
            ))
            .child(rows)
            .on_hover(cx.listener(|app, hovered: &bool, window, cx| {
                if !*hovered && !window.last_input_was_keyboard() {
                    app.machine_menu.hover.borrow_mut().clear();
                    cx.notify();
                }
            }))
            .into_any_element()
    }

    fn machine_row(
        &self,
        index: usize,
        entry: &CommandMenuEntry,
        cx: &Context<Self>,
    ) -> Stateful<Div> {
        let theme = self.desktop_theme;
        let selected = matches!(&entry.action, CommandMenuAction::OpenHost { home }
            if crate::native_hosts::same_host(home.as_deref(), self.machine_home.as_deref()));
        // The account name is the connected Forge's, so it names the local
        // tile only while that Forge is this computer's.
        let name = if matches!(&entry.action, CommandMenuAction::OpenHost { home: None })
            && self.machine_home.is_none()
        {
            self.profile_name
                .as_ref()
                .or(self.profile_hostname.as_ref())
                .cloned()
                .unwrap_or_else(|| entry.title.clone())
        } else {
            entry.title.clone()
        };
        let selector = format!("machine-option-{}", entry.id);
        let mut row = div()
            .id(("machine-option", index))
            .debug_selector(move || selector.clone())
            .role(gpui::Role::Button)
            .aria_label(entry.title.clone())
            .cursor_pointer()
            .relative()
            .px_2()
            .py_2()
            .rounded(RadiusTokens::value(RadiusStep::Xl))
            .flex()
            .items_center()
            .gap(px(10.0))
            .text_color(theme.foreground)
            .on_hover(cx.listener(move |app, hovered: &bool, _, cx| {
                if *hovered {
                    app.machine_menu.highlighted = index;
                    app.activate_machine_hover();
                    cx.notify();
                }
            }))
            .child(sidebar_hover_probe(
                Rc::clone(&self.machine_menu.hover),
                Rc::clone(&self.machine_menu.hover_surface),
                entry.id.clone(),
            ))
            .on_click(cx.listener(move |app, _: &ClickEvent, window, cx| {
                cx.stop_propagation();
                app.choose_machine(index, window, cx);
            }));
        if let Some(detail) = self.machine_menu.details.get(&entry.id) {
            row = row
                .child(crate::shell::profile_avatar(
                    &self.theme,
                    crate::shell::RailIdentity::new(Some(&detail.avatar_seed), None),
                ))
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.0))
                        .flex()
                        .flex_col()
                        .child(
                            div()
                                .truncate()
                                .text_size(px(14.0))
                                .line_height(px(20.0))
                                .child(name),
                        )
                        .child(
                            div()
                                .truncate()
                                .text_size(px(12.0))
                                .line_height(px(16.0))
                                .text_color(theme.secondary)
                                .child(detail.subtitle.clone()),
                        ),
                );
        } else {
            if matches!(entry.action, CommandMenuAction::AddHost) {
                row = row.child(
                    div()
                        .w(px(16.0))
                        .text_center()
                        .text_size(px(18.0))
                        .child("+"),
                );
            }
            row = row.child(
                div()
                    .flex_1()
                    .text_size(px(13.0))
                    .child(entry.title.clone()),
            );
        }
        if selected {
            row = row.child(desktop_nav_glyph(AssetId::TABLER_CHECK, theme));
        }
        row
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn submenu_opens_beside_header_and_flips_at_right_edge() {
        let viewport = size(px(1000.0), px(800.0));
        let parent = Bounds::new(gpui::point(px(20.0), px(500.0)), size(px(256.0), px(250.0)));
        let bounds = submenu_bounds(parent, parent, viewport, 3);
        assert_eq!(bounds.left(), parent.right() + px(6.0));
        assert_eq!(bounds.top(), parent.top());
        let parent = Bounds::new(gpui::point(px(740.0), px(500.0)), parent.size);
        assert_eq!(
            submenu_bounds(parent, parent, viewport, 3).right(),
            parent.left() - px(6.0)
        );
    }
}
