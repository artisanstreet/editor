//! Model-row, preview-pane, and policy-axis rendering for the model selector,
//! including the option tooltip and the shared hover pill.
//!
//! Split from `native_model_selector/render.rs`; the trigger, menu shell, and
//! their surface helpers come from the sibling `menu` child.

use super::menu::{
    animate_picker_menu, engine_asset, engine_light_smooth_out, source_card_shadows,
    source_control_gradient, source_focus_ring,
};
use super::*;

impl NativeModelSelector {
    #[expect(
        clippy::too_many_lines,
        reason = "one GPUI list builder composes the groups, sections, and scroll anchor that share reactive state"
    )]
    pub(super) fn render_model_list(&self, cx: &Context<Self>) -> Stateful<Div> {
        let groups = self.state.model_groups();
        let visible_ids = groups
            .iter()
            .flat_map(|group| group.models.iter().map(|model| model.id.clone()))
            .collect::<Vec<_>>();
        self.model_hover.borrow_mut().clear_if_missing(&visible_ids);
        let model_surface_bounds = Rc::clone(&self.model_hover_surface_bounds);
        let surface_probe = canvas(
            |_, _, _| {},
            move |bounds, (), window, cx| {
                let changed = {
                    let mut surface = model_surface_bounds.borrow_mut();
                    if *surface == Some(bounds) {
                        false
                    } else {
                        *surface = Some(bounds);
                        true
                    }
                };
                if changed {
                    window.defer(cx, |window, _| window.refresh());
                }
            },
        )
        .absolute()
        .top_0()
        .left_0()
        .size_full();
        let mut list = div()
            .id("artisan-native-model-selector-model-list")
            .relative()
            .flex()
            .flex_col()
            .w_full()
            .min_w(px(0.0))
            .h(px(MODEL_PANEL_HEIGHT_PX))
            .min_h(px(0.0))
            .overflow_y_scroll()
            .scrollbar_width(px(0.0))
            .track_scroll(&self.menu_scroll)
            .child(surface_probe)
            .child(render_picker_hover_pill(
                &self.theme,
                &self.model_hover,
                "model",
                px(14.0),
                cx.reduce_motion(),
            ))
            .gap(px(3.0));
        if groups.is_empty() {
            return list;
        }
        let show_group_headers =
            groups.len() > 1 || groups.first().is_some_and(|group| group.id != "default");
        let mut content = div()
            .relative()
            .flex()
            .flex_col()
            .w_full()
            .min_w(px(0.0))
            .flex_shrink_0()
            .gap(px(3.0))
            .on_scroll_wheel(cx.listener(Self::handle_model_scroll_wheel));
        for group in groups.iter() {
            let mut section = div().flex().flex_col().gap(px(2.0));
            if show_group_headers {
                let header = div()
                    .flex()
                    .items_center()
                    .gap(px(4.0))
                    .px(px(10.0))
                    .py(px(4.0))
                    .rounded(px(10.0))
                    .text_size(px(12.0))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(self.theme.colors.muted_foreground.to_paint())
                    .child(
                        icon(IconStyle::resolve(
                            self.theme,
                            AssetId::TABLER_CHEVRON_RIGHT,
                            IconSize::Compact,
                            IconTint::Muted,
                        ))
                        .size(px(14.0)),
                    )
                    .child(group.label.clone());
                section = section.child(header);
            }
            for model in &group.models {
                section = section.child(self.render_model_row(model, cx));
            }
            content = content.child(section);
        }
        list = list.child(content);
        let scroll = self.menu_scroll.clone();
        let top = self
            .theme
            .surfaces
            .value(SurfaceStep::S800)
            .with_alpha(0.14);
        let bottom = self
            .theme
            .surfaces
            .value(SurfaceStep::S800)
            .with_alpha(0.14);
        let fade = canvas(
            |_, _, _| {},
            move |bounds, (), window, _| {
                let offset = f32::from(scroll.offset().y);
                let maximum = f32::from(scroll.max_offset().y);
                let above = (-offset).clamp(0.0, 24.0);
                let below = (maximum + offset).clamp(0.0, 24.0);
                if above > 0.0 {
                    window.paint_quad(gpui::fill(
                        Bounds::new(bounds.origin, gpui::size(bounds.size.width, px(above))),
                        vertical_gradient(top, top.with_alpha(0.0)),
                    ));
                }
                if below > 0.0 {
                    window.paint_quad(gpui::fill(
                        Bounds::new(
                            point(bounds.left(), bounds.bottom() - px(below)),
                            gpui::size(bounds.size.width, px(below)),
                        ),
                        vertical_gradient(bottom.with_alpha(0.0), bottom),
                    ));
                }
            },
        )
        .absolute()
        .top_0()
        .left_0()
        .size_full();
        div()
            .id("artisan-model-list-frame")
            .relative()
            .flex_1()
            .min_w(px(0.0))
            .h(px(MODEL_PANEL_HEIGHT_PX))
            .child(list)
            .child(fade)
    }

    #[expect(
        clippy::too_many_lines,
        reason = "one GPUI row builder keeps the model definition, favorite control, and measurement probes in visual order"
    )]
    fn render_model_row(&self, model: &NativeModelView, cx: &Context<Self>) -> Stateful<Div> {
        let definition_disabled = self.state.model_definition_disabled(&model.id);
        let disabled_reason = self.state.model_definition_disabled_reason(&model.id);
        let model_id = model.id.clone();
        let hover_model_id = model.id.clone();
        let favorite_id = model.id.clone();
        let measured_model_id = model.id.clone();
        let model_hover = Rc::clone(&self.model_hover);
        let model_surface_bounds = Rc::clone(&self.model_hover_surface_bounds);
        let row_probe = canvas(
            |_, _, _| {},
            move |bounds, (), window, cx| {
                let Some(surface) = *model_surface_bounds.borrow() else {
                    return;
                };
                let rect = HoverRect {
                    left: f32::from(bounds.left() - surface.left()),
                    top: f32::from(bounds.top() - surface.top()),
                    width: f32::from(bounds.size.width),
                    height: f32::from(bounds.size.height),
                };
                if model_hover.borrow_mut().measure(&measured_model_id, rect) {
                    window.defer(cx, |window, _| window.refresh());
                }
            },
        )
        .absolute()
        .top_0()
        .left_0()
        .size_full();
        let mut row = div()
            .id(format!(
                "{NATIVE_MODEL_SELECTOR_ROW_SELECTOR_PREFIX}-{}",
                model.id
            ))
            .debug_selector({
                let selector = format!("{NATIVE_MODEL_SELECTOR_ROW_SELECTOR_PREFIX}-{}", model.id);
                move || selector.clone()
            })
            .relative()
            .flex()
            .items_center()
            .gap(px(8.0))
            .h(px(MODEL_ROW_HEIGHT_PX))
            .px(px(10.0))
            .rounded(px(14.0))
            .when(definition_disabled, |row| row.opacity(0.58));
        row = row.on_hover(cx.listener(move |view: &mut Self, hovered: &bool, _, cx| {
            if *hovered && view.menu_is_interactive() {
                view.state.preview_model(&hover_model_id);
                view.model_hover
                    .borrow_mut()
                    .set_active(hover_model_id.clone());
                cx.notify();
            }
        }));
        if !definition_disabled {
            row = row.on_click(
                cx.listener(move |view: &mut Self, _: &ClickEvent, window, cx| {
                    view.choose_model(&model_id, window, cx);
                }),
            );
        }
        row = row.child(row_probe);
        row = row.child(
            icon(IconStyle::resolve(
                self.theme,
                provider_asset(&model.provider_id, &model.engine_id),
                IconSize::Default,
                IconTint::Inherit,
            ))
            .size(px(20.0))
            .flex_shrink_0(),
        );
        let mut text = div()
            .flex()
            .flex_col()
            .flex_1()
            .min_w(px(0.0))
            .overflow_hidden()
            .child(
                div()
                    .truncate()
                    .text_size(px(14.0))
                    .line_height(px(20.0))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(model.name.clone()),
            )
            .child(
                div()
                    .truncate()
                    .text_size(px(12.0))
                    .line_height(px(16.0))
                    .text_color(self.theme.colors.muted_foreground.to_paint())
                    .child(model.lab.clone()),
            );
        if let Some(reason) = disabled_reason {
            text = text.child(
                div()
                    .truncate()
                    .text_size(px(10.0))
                    .line_height(px(12.0))
                    .text_color(self.theme.colors.muted_foreground.to_paint())
                    .child(reason),
            );
        }
        row = row.child(text);
        let mut favorite = div()
            .id(format!(
                "artisan-native-model-selector-favorite-{}",
                model.id
            ))
            .flex()
            .items_center()
            .justify_center()
            .size(px(28.0))
            .rounded_full()
            .role(gpui::Role::Button)
            .aria_label(if model.favorite {
                format!("Remove {} from favorites", model.name)
            } else {
                format!("Add {} to favorites", model.name)
            })
            .hover(|style| style.bg(hover_fill_gradient(self.theme)))
            .text_color(if model.favorite {
                self.theme.colors.favorite.to_paint()
            } else {
                self.theme.colors.muted_foreground.to_paint()
            })
            .child(
                icon(IconStyle::resolve(
                    self.theme,
                    if model.favorite {
                        AssetId::TABLER_STAR_FILLED
                    } else {
                        AssetId::TABLER_STAR
                    },
                    IconSize::Compact,
                    IconTint::Inherit,
                ))
                .size(px(16.0)),
            );
        if !definition_disabled {
            favorite =
                favorite.on_click(cx.listener(move |view: &mut Self, _: &ClickEvent, _, cx| {
                    view.toggle_favorite(&favorite_id, cx);
                    cx.stop_propagation();
                }));
        }
        row = row.child(favorite);
        row
    }

    pub(super) fn render_preview(
        &self,
        viewport: Size<Pixels>,
        cx: &Context<Self>,
    ) -> Stateful<Div> {
        let mut preview = div()
            .id("artisan-native-model-selector-preview")
            .flex()
            .flex_col()
            .justify_between()
            .h(px(MODEL_PANEL_HEIGHT_PX))
            .w(px(MODEL_PREVIEW_WIDTH_PX))
            .flex_shrink_0()
            .min_w(px(0.0))
            .min_h(px(0.0))
            .overflow_y_scroll()
            .scrollbar_width(px(0.0))
            .gap(px(8.0))
            .p(px(10.0));
        let Some(model_id) = self.state.previewed_model_id() else {
            return preview;
        };
        let Some(model) = self.state.snapshot().manifest.model(model_id).cloned() else {
            return preview;
        };
        let view = self.preview_view();
        let mut summary = div().flex().flex_col().gap(px(4.0)).child(
            div()
                .flex()
                .items_baseline()
                .justify_between()
                .gap(px(8.0))
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.0))
                        .truncate()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_size(px(14.0))
                        .line_height(px(20.0))
                        .child(model.name.clone()),
                ),
        );
        if let Some(description) = model.description.clone() {
            summary = summary.child(
                div()
                    .text_size(px(12.0))
                    .line_height(px(16.0))
                    .text_color(self.theme.colors.muted_foreground.to_paint())
                    .child(description),
            );
        }
        preview = preview.child(summary);
        if self.state.preview_policy().is_some() {
            preview = preview.child(self.render_policy_controls(&view, viewport, cx));
        }
        preview
    }

    fn render_policy_controls(
        &self,
        model: &NativeModelView,
        viewport: Size<Pixels>,
        cx: &Context<Self>,
    ) -> Div {
        let mut controls = div().flex().flex_col().gap(px(6.0));
        for (axis, label) in [
            (NativePolicyAxis::Variant, "Variant"),
            (NativePolicyAxis::Thinking, "Thinking"),
            (NativePolicyAxis::Speed, "Speed"),
            (NativePolicyAxis::ContextWindow, "Context"),
            (NativePolicyAxis::Permission, "Permission"),
        ] {
            let options = self.axis_options(axis, model);
            let should_show = match axis {
                NativePolicyAxis::Thinking | NativePolicyAxis::ContextWindow => !options.is_empty(),
                NativePolicyAxis::Speed
                | NativePolicyAxis::Permission
                | NativePolicyAxis::Variant => options.len() > 1,
            };
            if !should_show {
                continue;
            }
            let value = options
                .iter()
                .find(|option| option.selected)
                .map_or_else(|| options[0].label.clone(), |option| option.label.clone());
            controls = controls.child(self.render_axis_control(
                axis,
                label,
                value,
                self.state.model_definition_disabled(&model.id),
                viewport,
                cx,
            ));
        }
        controls
    }

    #[expect(
        clippy::too_many_lines,
        reason = "one GPUI control builder composes the trigger, popover anchor, and option viewport that share reactive state"
    )]
    fn render_axis_control(
        &self,
        axis: NativePolicyAxis,
        label: &str,
        value: String,
        disabled: bool,
        viewport: Size<Pixels>,
        cx: &Context<Self>,
    ) -> Div {
        let axis_bounds = Rc::clone(&self.axis_trigger_bounds);
        let probe = canvas(
            |_, _, _| {},
            move |bounds, (), _, _| {
                axis_bounds.borrow_mut()[axis as usize] = Some(bounds);
            },
        )
        .absolute()
        .top_0()
        .left_0()
        .size_full();
        let axis_open = self.state.is_axis_open(axis);
        let mut control = div()
            .relative()
            .flex()
            .flex_col()
            .flex_shrink_0()
            .child(probe);
        let mut trigger = div()
            .id(format!("artisan-native-model-selector-axis-{axis:?}"))
            .debug_selector(move || format!("artisan-model-policy-{axis:?}"))
            .role(gpui::Role::Button)
            .aria_label(format!("{label}: {value}"))
            .flex()
            .items_center()
            .gap(px(7.0))
            .h(px(POLICY_CONTROL_HEIGHT_PX))
            .px(px(8.0))
            .rounded(px(8.0))
            .bg(source_control_gradient(&self.theme))
            .shadow(source_card_shadows(&self.theme))
            .focus_visible(move |style| {
                let mut shadows = source_card_shadows(&self.theme);
                shadows.extend(source_focus_ring(&self.theme));
                style
                    .border_1()
                    .border_color(self.theme.colors.ring.to_paint())
                    .shadow(shadows)
            })
            .when(disabled, |trigger| trigger.opacity(0.58))
            .when(axis_open, |trigger| {
                let mut shadows = source_card_shadows(&self.theme);
                shadows.extend(source_focus_ring(&self.theme));
                trigger.shadow(shadows)
            })
            .child(
                icon(IconStyle::resolve(
                    self.theme,
                    axis_icon(axis),
                    IconSize::Compact,
                    IconTint::Muted,
                ))
                .size(px(14.0))
                .flex_shrink_0(),
            )
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_w(px(0.0))
                    .justify_center()
                    .truncate()
                    .text_size(px(12.0))
                    .text_color(self.theme.colors.foreground.to_paint())
                    .child(value),
            )
            .child(
                icon(IconStyle::resolve(
                    self.theme,
                    AssetId::TABLER_SELECTOR,
                    IconSize::Compact,
                    IconTint::Muted,
                ))
                .size(px(14.0)),
            );
        if !disabled {
            trigger = trigger.on_click(cx.listener(
                move |view: &mut Self, _: &ClickEvent, window, cx| {
                    view.toggle_axis(axis, window, cx);
                },
            ));
        }
        control = control.child(trigger);
        let axis_popup_visible = self.axis_menu_motion_axis == Some(axis)
            && self.axis_menu_motion.borrow().phase() != PickerMenuPhase::Hidden;
        if axis_popup_visible
            && let Some(trigger_bounds) = self.axis_trigger_bounds.borrow()[axis as usize]
        {
            let popup_bounds = Rc::clone(&self.axis_menu_bounds);
            let axis_surface_bounds = Rc::clone(&self.axis_hover_surface_bounds);
            let popup_probe = canvas(
                |_, _, _| {},
                move |bounds, (), window, cx| {
                    let popup_changed = {
                        let mut popup = popup_bounds.borrow_mut();
                        if *popup == Some(bounds) {
                            false
                        } else {
                            *popup = Some(bounds);
                            true
                        }
                    };
                    let surface_changed = {
                        let mut surface = axis_surface_bounds.borrow_mut();
                        if *surface == Some(bounds) {
                            false
                        } else {
                            *surface = Some(bounds);
                            true
                        }
                    };
                    if popup_changed || surface_changed {
                        window.defer(cx, |window, _| window.refresh());
                    }
                },
            )
            .absolute()
            .top_0()
            .left_0()
            .size_full();
            let mut options = div()
                .id(format!(
                    "artisan-native-model-selector-axis-options-{axis:?}"
                ))
                .debug_selector(|| "artisan-model-policy-options".to_owned())
                .relative()
                .occlude()
                .child(popup_probe)
                .w(trigger_bounds.size.width)
                .min_w(px(DROPDOWN_MIN_WIDTH_PX))
                .flex()
                .flex_col()
                .p(px(4.0))
                .max_h(dropdown_max_height_for_viewport(viewport, trigger_bounds))
                .overflow_hidden()
                .rounded(px(18.0))
                .backdrop_blur(glass_blur_radius(GlassStrength::Strong))
                .bg(glass_foreground_base(&self.theme))
                .shadow(glass_card_shadows());
            options = options.child(glass_material_layer(GlassStrength::Strong, px(18.0)));
            options = options.child(glass_highlight_layer(GlassStrength::Strong, px(18.0)));
            options = options.child(render_picker_hover_pill(
                &self.theme,
                &self.axis_hover,
                "axis",
                px(14.0),
                cx.reduce_motion(),
            ));
            let dropdown_max_height = dropdown_max_height_for_viewport(viewport, trigger_bounds);
            let viewport_max_height = px((f32::from(dropdown_max_height) - 8.0).max(0.0));
            let mut viewport = div()
                .id(format!(
                    "artisan-native-model-selector-axis-viewport-{axis:?}"
                ))
                .relative()
                .w_full()
                .min_w(px(0.0))
                .min_h(px(0.0))
                .flex_shrink_0()
                .max_h(viewport_max_height)
                .overflow_y_scroll()
                .scrollbar_width(px(0.0))
                .track_scroll(&self.axis_menu_scroll);
            let mut content = div()
                .relative()
                .flex()
                .flex_col()
                .w_full()
                .min_w(px(0.0))
                .flex_shrink_0()
                .id("artisan-model-policy-hover-surface")
                .on_hover(cx.listener(|view: &mut Self, hovered: &bool, _, cx| {
                    if !*hovered {
                        view.axis_hover.borrow_mut().clear();
                        view.clear_option_tooltip();
                        cx.notify();
                    }
                }))
                .on_scroll_wheel(cx.listener(Self::handle_axis_scroll_wheel));
            let mut current_thinking_group: Option<String> = None;
            let axis_options = self.axis_options(axis, &self.preview_view());
            let visible_option_ids = axis_options
                .iter()
                .map(|option| option.id.clone())
                .collect::<Vec<_>>();
            self.axis_hover
                .borrow_mut()
                .clear_if_missing(&visible_option_ids);
            for option in axis_options {
                let group = option.group.clone();
                if axis == NativePolicyAxis::Thinking && group != current_thinking_group {
                    if current_thinking_group.is_some() {
                        content = content.child(
                            div().mx(px(8.0)).my(px(4.0)).h(px(1.0)).bg(self
                                .theme
                                .colors
                                .border
                                .with_alpha(self.theme.colors.border.a * 0.4)
                                .to_paint()),
                        );
                    }
                    if let Some(group) = group.as_deref() {
                        content = content.child(
                            div()
                                .px(px(12.0))
                                .pt(px(6.0))
                                .pb(px(4.0))
                                .text_size(px(10.0))
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(
                                    self.theme
                                        .colors
                                        .muted_foreground
                                        .with_alpha(0.75)
                                        .to_paint(),
                                )
                                .child(group.to_owned()),
                        );
                    }
                    current_thinking_group = group;
                }
                content = content.child(self.render_axis_option(axis, &option, cx));
            }
            viewport = viewport.child(content);
            options = options.child(viewport);
            control = control.child(
                deferred(
                    anchored()
                        .anchor(Anchor::BottomLeft)
                        .position(trigger_bounds.origin)
                        .offset(point(px(0.0), px(-DROPDOWN_GAP_PX)))
                        .child(animate_picker_menu(
                            options,
                            self.axis_menu_motion.clone(),
                            *self.axis_menu_motion.borrow(),
                            "axis-options",
                        )),
                )
                .with_priority(2),
            );
        }
        control
    }

    #[expect(
        clippy::too_many_lines,
        reason = "one GPUI option builder keeps the label, description highlights, and disabled treatment in visual order"
    )]
    fn render_axis_option(
        &self,
        axis: NativePolicyAxis,
        option: &SelectorOption,
        cx: &Context<Self>,
    ) -> Stateful<Div> {
        let option_id = option.id.clone();
        let option_disabled = option.disabled;
        let option_label = option.label.clone();
        let option_selector = format!("artisan-model-policy-option-{axis:?}-{}", option.id);
        let option_description = option.description.clone();
        let option_advisory = option.advisory.clone();
        let measured_option_id = option.id.clone();
        let measured_tooltip_key = option_tooltip_key(axis, &option.id);
        let axis_hover = Rc::clone(&self.axis_hover);
        let axis_surface_bounds = Rc::clone(&self.axis_hover_surface_bounds);
        let option_tooltip = Rc::clone(&self.option_tooltip);
        let row_probe = canvas(
            |_, _, _| {},
            move |bounds, (), window, cx| {
                let Some(surface) = *axis_surface_bounds.borrow() else {
                    return;
                };
                let rect = HoverRect {
                    left: f32::from(bounds.left() - surface.left()),
                    top: f32::from(bounds.top() - surface.top()),
                    width: f32::from(bounds.size.width),
                    height: f32::from(bounds.size.height),
                };
                if axis_hover.borrow_mut().measure(&measured_option_id, rect) {
                    window.defer(cx, |window, _| window.refresh());
                }
                let tooltip_changed = {
                    let mut tooltip = option_tooltip.borrow_mut();
                    tooltip.as_mut().is_some_and(|target| {
                        if target.key != measured_tooltip_key {
                            return false;
                        }
                        if target.row_bounds == Some(bounds) {
                            false
                        } else {
                            target.row_bounds = Some(bounds);
                            true
                        }
                    })
                };
                if tooltip_changed {
                    window.defer(cx, |window, _| window.refresh());
                }
            },
        )
        .absolute()
        .top_0()
        .left_0()
        .size_full();
        let tooltip_description =
            option_tooltip_text(option_advisory.as_deref(), option_description.as_deref());
        let tooltip_key = option_tooltip_key(axis, &option.id);
        let tooltip_option = option.clone();
        let mut row = div()
            .id(format!(
                "artisan-native-model-selector-option-{axis:?}-{}",
                option.id
            ))
            .debug_selector(move || option_selector.clone())
            .role(gpui::Role::Button)
            .aria_label(format!("{axis:?}: {option_label}"))
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
            .rounded(px(14.0))
            .text_size(px(14.0))
            .line_height(px(20.0))
            .when(option.disabled, |row| row.opacity(0.5))
            .child(row_probe)
            .child(div().flex_1().min_w(px(0.0)).truncate().child(option_label));
        row = row.on_hover(cx.listener(move |view: &mut Self, hovered: &bool, _, cx| {
            if *hovered {
                if !view.axis_is_interactive(axis) {
                    return;
                }
                if !option_disabled {
                    view.axis_hover
                        .borrow_mut()
                        .set_active(tooltip_option.id.clone());
                }
                view.begin_option_tooltip(axis, &tooltip_option, cx);
            } else {
                view.clear_option_tooltip_for(&tooltip_key);
            }
            cx.notify();
        }));
        if let Some(tooltip_description) = tooltip_description {
            row = row.aria_description(tooltip_description);
        }
        if !option_disabled {
            row = row.on_click(
                cx.listener(move |view: &mut Self, _: &ClickEvent, window, cx| {
                    view.choose_axis(axis, &option_id, window, cx);
                }),
            );
        }
        if option.selected {
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
                        icon(IconStyle::resolve(
                            self.theme,
                            AssetId::TABLER_CHECK,
                            IconSize::Compact,
                            IconTint::Muted,
                        ))
                        .size(px(14.0)),
                    ),
            );
        }
        row
    }

    #[expect(
        clippy::too_many_lines,
        reason = "one mapping per policy axis keeps the catalog-capability-to-option projection together for review"
    )]
    pub(in crate::native_model_selector) fn axis_options(
        &self,
        axis: NativePolicyAxis,
        model: &NativeModelView,
    ) -> Vec<SelectorOption> {
        let policy = self.state.preview_policy();
        let model_definition_disabled = self.state.model_definition_disabled(&model.id);
        match axis {
            NativePolicyAxis::Variant => self
                .state
                .snapshot()
                .models_for_engine(&model.engine_id, "", Some(model.id.as_str()))
                .into_iter()
                .filter(|candidate| same_model_family(model, candidate))
                .map(|candidate| SelectorOption {
                    id: candidate.id.clone(),
                    label: candidate
                        .variant_label
                        .as_deref()
                        .map_or_else(|| candidate.name.clone(), humanize_variant),
                    description: None,
                    advisory: self.state.model_definition_disabled_reason(&candidate.id),
                    group: None,
                    selected: candidate.id == model.id,
                    disabled: self.state.model_definition_disabled(&candidate.id),
                })
                .collect(),
            NativePolicyAxis::Thinking => match &model.capabilities.thinking {
                NativeThinkingCapability::Supported { options, .. } => options
                    .iter()
                    .map(|option| SelectorOption {
                        id: option.id.clone(),
                        label: humanize_variant(&option.id),
                        description: option.description.clone(),
                        advisory: option.advisory.clone(),
                        group: thinking_group_label(&option.presentation_group).map(str::to_owned),
                        selected: policy
                            .as_ref()
                            .and_then(|policy| policy.reasoning_effort.as_ref())
                            .is_some_and(|value| value.id == option.id),
                        disabled: model_definition_disabled,
                    })
                    .collect(),
                NativeThinkingCapability::Unavailable | NativeThinkingCapability::Native { .. } => {
                    Vec::new()
                }
            },
            NativePolicyAxis::Speed => model
                .capabilities
                .speed_options
                .iter()
                .filter(|option| option.disabled.is_none())
                .map(|option| SelectorOption {
                    id: option.id.clone(),
                    label: option.label.clone(),
                    description: (!option.description.is_empty())
                        .then(|| option.description.clone()),
                    advisory: None,
                    group: None,
                    selected: policy
                        .as_ref()
                        .and_then(|policy| policy.speed.as_ref())
                        .is_some_and(|value| value.id == option.id),
                    disabled: model_definition_disabled,
                })
                .collect(),
            NativePolicyAxis::ContextWindow => model
                .capabilities
                .context_window
                .as_ref()
                .map(|capability| {
                    capability
                        .options
                        .iter()
                        .map(|option| SelectorOption {
                            id: option.id.clone(),
                            label: option.label.clone(),
                            description: option.description.clone(),
                            advisory: option.advisory.clone(),
                            group: None,
                            selected: policy
                                .as_ref()
                                .and_then(|policy| policy.context_window.as_ref())
                                .is_some_and(|value| value.id == option.id),
                            disabled: model_definition_disabled,
                        })
                        .collect()
                })
                .unwrap_or_default(),
            NativePolicyAxis::Permission => self
                .state
                .snapshot()
                .manifest
                .harness(&model.engine_id)
                .map(|harness| {
                    harness
                        .permissions
                        .options
                        .iter()
                        .map(|option| SelectorOption {
                            id: option.id.clone(),
                            label: option.label.clone(),
                            description: (!option.description.is_empty())
                                .then(|| option.description.clone()),
                            advisory: None,
                            group: None,
                            selected: policy
                                .as_ref()
                                .and_then(|policy| policy.permission.as_ref())
                                .is_some_and(|value| value.id == option.id),
                            disabled: model_definition_disabled,
                        })
                        .collect()
                })
                .unwrap_or_default(),
        }
    }

    pub(in crate::native_model_selector) fn preview_view(&self) -> NativeModelView {
        let Some(model_id) = self.state.previewed_model_id() else {
            return fallback_model_view_from_state(&self.state);
        };
        let Some(model) = self.state.snapshot().manifest.model(model_id) else {
            return fallback_model_view_from_state(&self.state);
        };
        if let Some(mut row) = self
            .state
            .model_groups()
            .iter()
            .flat_map(|group| &group.models)
            .find(|row| row.id == model_id)
            .cloned()
        {
            row.selected = true;
            return row;
        }
        self.state
            .snapshot()
            .models_for_engine(&model.harness, "", Some(model_id))
            .into_iter()
            .find(|row| row.id == model_id)
            .unwrap_or_else(|| fallback_model_view(model))
    }
}

impl NativeModelSelector {
    pub(super) fn render_option_tooltip(&self, viewport: Size<Pixels>) -> Option<AnyElement> {
        let target = self.option_tooltip.borrow().clone()?;
        let row_bounds = target.row_bounds?;
        if !target.visible {
            return None;
        }

        let viewport_width = f32::from(viewport.width).max(1.0);
        let right_space = viewport_width - f32::from(row_bounds.right()) - OPTION_TOOLTIP_GAP_PX;
        let left_space = f32::from(row_bounds.left()) - OPTION_TOOLTIP_GAP_PX;
        let show_right = right_space >= OPTION_TOOLTIP_WIDTH_PX || right_space >= left_space;
        let available_width = if show_right { right_space } else { left_space };
        let width = OPTION_TOOLTIP_WIDTH_PX.min(available_width.max(1.0));
        let (anchor, position, offset) = if show_right {
            (
                Anchor::LeftCenter,
                point(row_bounds.right(), row_bounds.center().y),
                point(px(OPTION_TOOLTIP_GAP_PX), px(0.0)),
            )
        } else {
            (
                Anchor::RightCenter,
                point(row_bounds.left(), row_bounds.center().y),
                point(px(-OPTION_TOOLTIP_GAP_PX), px(0.0)),
            )
        };
        Some(
            deferred(
                anchored()
                    .anchor(anchor)
                    .position(position)
                    .offset(offset)
                    .child(render_option_tooltip_surface(
                        &self.theme,
                        width,
                        target.advisory,
                        target.description,
                    )),
            )
            .with_priority(3)
            .into_any_element(),
        )
    }
}

fn render_option_tooltip_surface(
    theme: &ArtisanTheme,
    width: f32,
    advisory: Option<String>,
    description: Option<String>,
) -> AnyElement {
    let advisory = advisory.filter(|text| !text.is_empty());
    let description = description.filter(|text| !text.is_empty());
    let mut text = String::new();
    let mut advisory_end = 0;
    if let Some(advisory) = advisory {
        text.push_str(&advisory);
        advisory_end = text.len();
        if description.is_some() {
            text.push(' ');
        }
    }
    if let Some(description) = description {
        text.push_str(&description);
    }
    let body = if advisory_end == 0 {
        StyledText::new(SharedString::from(text))
    } else {
        StyledText::new(SharedString::from(text)).with_highlights([(
            0..advisory_end,
            HighlightStyle {
                color: Some(theme.colors.destructive.to_paint()),
                font_weight: Some(FontWeight::MEDIUM),
                ..Default::default()
            },
        )])
    };
    div()
        .id("artisan-native-model-selector-option-tooltip")
        .debug_selector(|| "artisan-native-model-selector-option-tooltip".to_owned())
        .w(px(width))
        .max_w(px(OPTION_TOOLTIP_WIDTH_PX))
        .px(px(12.0))
        .py(px(8.0))
        .rounded(px(18.0))
        .backdrop_blur(glass_blur_radius(GlassStrength::Strong))
        .bg(glass_foreground_base(theme))
        .shadow(glass_card_shadows())
        .text_size(px(12.0))
        .line_height(px(16.0))
        .whitespace_normal()
        .text_color(theme.colors.muted_foreground.to_paint())
        .relative()
        .overflow_hidden()
        .child(glass_material_layer(GlassStrength::Strong, px(18.0)))
        .child(glass_highlight_layer(GlassStrength::Strong, px(18.0)))
        .child(body)
        .into_any_element()
}

pub(crate) fn render_picker_hover_pill(
    theme: &ArtisanTheme,
    hover: &Rc<RefCell<SlidingHoverState>>,
    surface: &'static str,
    corner_radius: Pixels,
    reduce_motion: bool,
) -> AnyElement {
    let (mut rect, visible, transition) = {
        let hover = hover.borrow();
        (hover.visual_rect(), hover.visible(), hover.transition())
    };
    if reduce_motion && let Some(transition) = transition {
        rect = transition.to;
        hover
            .borrow_mut()
            .apply_progress(transition.generation, 1.0);
    }
    let pill = div()
        .id(format!("artisan-native-model-picker-hover-{surface}"))
        .absolute()
        .left(px(rect.left))
        .top(px(rect.top))
        .w(px(rect.width))
        .h(px(rect.height))
        .rounded(corner_radius)
        .bg(hover_fill_gradient(*theme))
        .shadow(source_hover_highlight_shadow(theme))
        .opacity(if visible { 1.0 } else { 0.0 });
    if !reduce_motion && let Some(transition) = transition {
        let motion = Rc::clone(hover);
        let from = transition.from;
        let to = transition.to;
        let generation = transition.generation;
        let animation_id = ElementId::Name(
            format!("artisan-native-model-picker-hover-{surface}-{generation}").into(),
        );
        return pill
            .with_animation(
                animation_id,
                Animation::new(Duration::from_millis(PICKER_HOVER_MOTION_DURATION_MS))
                    .with_easing(engine_light_smooth_out),
                move |pill, progress| {
                    let rect = from.lerp(to, progress);
                    motion.borrow_mut().apply_progress(generation, progress);
                    pill.left(px(rect.left))
                        .top(px(rect.top))
                        .w(px(rect.width))
                        .h(px(rect.height))
                },
            )
            .into_any_element();
    }
    pill.into_any_element()
}

fn dropdown_max_height_for_viewport(
    viewport: Size<Pixels>,
    trigger_bounds: Bounds<Pixels>,
) -> Pixels {
    let viewport_height = f32::from(viewport.height);
    let available_above = f32::from(trigger_bounds.top()) - DROPDOWN_GAP_PX;
    let available_below = viewport_height - f32::from(trigger_bounds.bottom()) - DROPDOWN_GAP_PX;
    px(available_above.max(available_below).max(0.0))
}

fn source_hover_highlight_shadow(theme: &ArtisanTheme) -> Vec<gpui::BoxShadow> {
    vec![gpui::BoxShadow {
        color: theme.colors.foreground.with_alpha(0.13).to_paint(),
        offset: point(px(0.0), px(0.0)),
        blur_radius: px(0.0),
        spread_radius: px(0.5),
        inset: true,
    }]
}

fn axis_icon(axis: NativePolicyAxis) -> AssetId {
    match axis {
        NativePolicyAxis::Variant | NativePolicyAxis::Thinking => AssetId::TABLER_BRAIN,
        NativePolicyAxis::Speed => AssetId::TABLER_BOLT_FILLED,
        NativePolicyAxis::ContextWindow => AssetId::TABLER_ARROWS_HORIZONTAL,
        NativePolicyAxis::Permission => AssetId::TABLER_LOCK,
    }
}

fn thinking_group_label(group: &str) -> Option<&'static str> {
    match group {
        "base" => Some("Efforts"),
        "special" => Some("Special Efforts"),
        _ => None,
    }
}

fn provider_asset(provider_id: &str, engine_id: &str) -> AssetId {
    match provider_id {
        "openai" => AssetId::SVGL_OPENAI,
        "anthropic" => AssetId::SVGL_CLAUDE_AI,
        "xai" => AssetId::SVGL_GROK,
        "cursor" => AssetId::SVGL_CURSOR,
        "opencode" => AssetId::BRANDS_OPENCODE,
        _ => engine_asset(engine_id),
    }
}
