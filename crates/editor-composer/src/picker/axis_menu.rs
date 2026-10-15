//! The preview's policy controls and their glass dropdowns.

use super::*;
use editor_ui::hover_card::highlight_fill;

/// One dropdown row: label, and the check on the chosen one.
fn option_row(
    id: SharedString,
    label: impl Into<SharedString>,
    chosen: bool,
    theme: &ArtisanTheme,
) -> gpui::Stateful<gpui::Div> {
    let theme = *theme;
    let label = label.into();
    let mut row = div()
        .id(id)
        .role(gpui::Role::MenuItem)
        .aria_label(label.clone())
        .aria_selected(chosen)
        .focusable()
        .tab_index(0)
        .relative()
        .flex()
        .flex_row()
        .items_center()
        .w_full()
        .min_w(px(0.0))
        .gap(px(10.0))
        .pl(px(12.0))
        .pr(px(32.0))
        .py(px(8.0))
        .rounded(RadiusTokens::value(RadiusStep::Xl))
        .text_size(TypographyTokens::size(TextStep::Sm))
        .line_height(px(20.0))
        .cursor_pointer()
        .child(div().flex_1().min_w(px(0.0)).truncate().child(label));
    if chosen {
        row = row.child(
            div()
                .absolute()
                .right(px(8.0))
                .top_0()
                .bottom(px(0.0))
                .w(px(14.0))
                .flex()
                .items_center()
                .justify_center()
                .child(
                    asset_glyph(AssetId::TABLER_CHECK)
                        .size(px(14.0))
                        .text_color(theme.colors.muted_foreground.to_paint()),
                ),
        );
    }
    row
}

impl Composer {
    /// One policy control and, while open or closing, its glass dropdown
    /// with the shared hover pill and option tooltips.
    #[expect(
        clippy::too_many_lines,
        reason = "one GPUI builder keeps the trigger, the dropdown, its rows and their probes in visual order"
    )]
    pub(super) fn axis_control(
        &self,
        axis: Axis,
        options: Vec<AxisOption>,
        theme: &ArtisanTheme,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let theme = *theme;
        let muted = theme.colors.muted_foreground.to_paint();
        let value = options
            .iter()
            .find(|option| option.selected)
            .or(options.first())
            .map(|option| option.label.clone())
            .unwrap_or_default();
        let trigger = div()
            .id(SharedString::from(format!("composer-axis-{axis:?}")))
            .role(gpui::Role::Button)
            .aria_label(format!("{}: {value}", axis.title()))
            .aria_expanded(self.axis == Some(axis))
            .focusable()
            .tab_index(0)
            .debug_selector(move || format!("composer-axis-{axis:?}"))
            .flex()
            .items_center()
            .gap(px(7.0))
            .h(px(POLICY_CONTROL_HEIGHT_PX))
            .px(px(8.0))
            .rounded(RadiusTokens::value(RadiusStep::Md))
            .bg(control_gradient())
            .shadow(card_shadows(&theme))
            .cursor_pointer()
            .relative()
            .child(
                div()
                    .absolute()
                    .size_full()
                    .debug_selector(move || format!("composer-axis-{axis:?}-trigger")),
            )
            .on_click(cx.listener(move |this, _, window, cx| {
                this.toggle_axis(axis, cx);
                if let Some(focus) = &this.picker.axis_focus {
                    focus.focus(window, cx);
                }
            }))
            .child(
                asset_glyph(axis.icon())
                    .size(px(14.0))
                    .flex_shrink_0()
                    .text_color(muted),
            )
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_w(px(0.0))
                    .justify_center()
                    .truncate()
                    .text_size(TypographyTokens::size(TextStep::Xs))
                    .text_color(theme.colors.foreground.to_paint())
                    .child(value),
            )
            .child(
                asset_glyph(AssetId::TABLER_SELECTOR)
                    .size(px(14.0))
                    .text_color(muted),
            );
        let mut control = div()
            .relative()
            .flex()
            .flex_col()
            .flex_shrink_0()
            .child(trigger);
        if self.picker.shown_axis != Some(axis) {
            return control;
        }
        let ids: Vec<String> = (0..options.len()).map(|index| index.to_string()).collect();
        self.picker.axis_hover.clear_if_missing(&ids);
        let closing = self.picker.axis.borrow().phase() == MenuPhase::Closing;
        let mut rows = div()
            .id(SharedString::from(format!("composer-axis-rows-{axis:?}")))
            .relative()
            .flex()
            .flex_col()
            .w_full()
            .min_w(px(0.0))
            .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                if !*hovered {
                    this.picker.axis_hover.clear();
                    this.clear_tooltip();
                    cx.notify();
                }
            }));
        let header = |label: &'static str, special: bool| {
            div()
                .debug_selector(move || {
                    if special {
                        format!("composer-axis-{axis:?}-special-header")
                    } else {
                        format!("composer-axis-{axis:?}-header")
                    }
                })
                .px(px(12.0))
                .py(px(4.0))
                .text_size(TypographyTokens::size(TextStep::Xs))
                .line_height(px(16.0))
                .font_weight(FontWeight::MEDIUM)
                .text_color(muted)
                .child(label)
        };
        rows = rows.child(header(axis.title(), false));
        let keyboard_options: Vec<_> = options.iter().map(|option| option.apply.clone()).collect();
        let chosen = options
            .iter()
            .position(|option| option.selected)
            .unwrap_or(0);
        let keyed = match self.picker.axis_cursor {
            Some(AxisCursor::Keys(index)) => Some(index),
            _ => None,
        };
        let mut special_shown = false;
        for (index, option) in options.into_iter().enumerate() {
            if option.special && !special_shown {
                special_shown = true;
                rows = rows
                    .child(
                        div()
                            .h(px(1.0))
                            .mx(px(8.0))
                            .my(px(4.0))
                            .bg(theme.colors.line.to_paint()),
                    )
                    .child(header("Special Efforts", true));
            }
            let config = option.apply;
            let key = format!("axis:{axis:?}:{index}");
            let hover_id = index.to_string();
            let (advisory, description) = (option.advisory, option.description);
            let tooltip_key = key.clone();
            rows = rows.child(
                option_row(
                    format!("composer-axis-{axis:?}-{index}").into(),
                    option.label,
                    option.selected,
                    &theme,
                )
                .debug_selector(move || format!("composer-axis-{axis:?}-{index}"))
                .when(keyed == Some(index), |row| highlight_fill(row, &theme))
                .child(self.picker.axis_hover.row_probe(hover_id.clone()))
                .when(self.picker.tooltip.pending() == Some(key.as_str()), |row| {
                    row.child(self.picker.tooltip_anchor.track())
                })
                .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                    if *hovered {
                        if this.axis != Some(axis) {
                            return;
                        }
                        this.picker.axis_cursor = Some(AxisCursor::Pointer(index));
                        this.picker.axis_hover.set_active(hover_id.clone());
                        if advisory.is_some() || description.is_some() {
                            this.begin_tooltip(
                                tooltip_key.clone(),
                                advisory.clone(),
                                description.clone().unwrap_or_default(),
                                tooltip::SIDE,
                                cx,
                            );
                        } else {
                            this.clear_tooltip();
                        }
                    } else {
                        this.clear_tooltip_for(&tooltip_key);
                    }
                    cx.notify();
                }))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.close_axis(cx);
                    this.choose_config(config.clone(), cx);
                })),
            );
        }
        let panel = glass_menu(
            format!("composer-axis-options-{axis:?}"),
            &theme,
            RadiusTokens::value(RadiusStep::X2l),
        )
        .when_some(self.picker.axis_focus.as_ref(), |panel, focus| {
            panel.track_focus(focus)
        })
        .on_key_down(cx.listener(move |this, event: &gpui::KeyDownEvent, _, cx| {
            if keyboard_options.is_empty() {
                return;
            }
            let current = this.picker.axis_cursor.map_or(chosen, AxisCursor::index);
            let next = match event.keystroke.key.as_str() {
                "home" => 0,
                "end" => keyboard_options.len() - 1,
                "up" => current.saturating_sub(1),
                "down" => (current + 1).min(keyboard_options.len() - 1),
                "enter" | "space" => {
                    this.choose_config(keyboard_options[current].clone(), cx);
                    this.close_axis(cx);
                    cx.stop_propagation();
                    return;
                }
                "escape" => {
                    if this.axis.is_some() {
                        this.close_axis(cx);
                    } else {
                        this.close_menu(cx);
                    }
                    cx.stop_propagation();
                    return;
                }
                _ => return,
            };
            // The keys' row wears the highlight; the hover pill stays
            // with the pointer.
            this.picker.axis_cursor = Some(AxisCursor::Keys(next));
            cx.stop_propagation();
            cx.notify();
        }))
        .w(px(MODEL_PREVIEW_WIDTH_PX - 20.0))
        .min_w(px(DROPDOWN_MIN_WIDTH_PX))
        .p(px(4.0))
        .when(!closing, |panel| {
            panel.on_mouse_down_out(cx.listener(|this, _, _, cx| this.close_axis(cx)))
        })
        .child(self.picker.axis_hover.surface_probe())
        .child(self.picker.axis_hover.pill(
            "composer-axis-hover",
            &theme,
            RadiusTokens::value(RadiusStep::Xl),
            cx.reduce_motion(),
        ))
        .child(rows);
        control = control.child(above_trigger(
            px(DROPDOWN_GAP_PX),
            2,
            animate_menu(
                panel,
                &self.picker.axis,
                "composer-axis-options",
                MenuEntrance::Slide,
                MENU_MOTION,
                MENU_MOTION,
            ),
        ));
        control
    }
}
