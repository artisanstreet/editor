//! The glass model menu: engine tabs, the model list and its preview.

use super::*;

impl Composer {
    /// The glass model menu: engine tabs over the model list and the
    /// previewed model's details and policy controls. It slides in, and
    /// out again before it unmounts.
    pub(super) fn model_menu(&self, theme: &ArtisanTheme, cx: &Context<Self>) -> AnyElement {
        let theme = *theme;
        let width = MENU_WIDTH_PX.min(self.viewport_width - MENU_VIEWPORT_INSET_X_PX);
        let current = self.effective_config(cx);
        // Only engines with models to pick get a tab; the thread's own
        // engine stays so its sign-in or install prompt is reachable.
        let engines: Vec<EngineStatus> = self
            .host(cx)
            .map(|host| {
                host.read(cx)
                    .engines
                    .values()
                    .filter(|status| {
                        Gate::of(status).is_none()
                            || current.as_ref().map(|config| config.engine) == Some(status.engine)
                    })
                    .cloned()
                    .collect()
            })
            .unwrap_or_default();
        let browsing = self
            .browsing
            .filter(|engine| engines.iter().any(|status| status.engine == *engine))
            .or_else(|| current.as_ref().map(|config| config.engine))
            .or_else(|| engines.first().map(|status| status.engine));
        let preview = self.preview_config(cx);
        let closing = self.picker.menu.borrow().phase() == MenuPhase::Closing;
        let panel = glass_menu(
            "composer-model-menu",
            &theme,
            RadiusTokens::value(RadiusStep::X3l),
        )
        .w(px(width))
        .max_h(px(MENU_MAX_HEIGHT_PX))
        .p(px(8.0))
        .gap(px(8.0))
        .when(!closing, |panel| {
            panel.on_mouse_down_out(cx.listener(|this, _, _, cx| {
                if this.axis.is_none() {
                    this.close_menu(cx);
                }
            }))
        })
        .child(self.engine_tabs(&engines, browsing, &theme, cx))
        .child(
            div()
                .flex()
                .flex_row()
                .h(px(MODEL_PANEL_HEIGHT_PX))
                .flex_shrink_0()
                .gap(px(8.0))
                .child(self.model_list(&engines, browsing, current.as_ref(), &theme, cx))
                .child(self.model_preview(&engines, preview.as_ref(), &theme, cx)),
        );
        animate_menu(
            panel,
            &self.picker.menu,
            "composer-model-menu",
            MenuEntrance::Slide,
            MENU_MOTION,
            MENU_MOTION,
        )
    }

    /// The engine tabs, with the light under the active one sliding from
    /// tab to tab. Only the thread's own engine can be without models; its
    /// tab is greyed and says why on hover.
    pub(super) fn engine_tabs(
        &self,
        engines: &[EngineStatus],
        browsing: Option<EngineId>,
        theme: &ArtisanTheme,
        cx: &Context<Self>,
    ) -> impl IntoElement {
        let theme = *theme;
        let mut tabs = div()
            .id("composer-engine-tabs")
            .relative()
            .flex()
            .flex_row()
            .w_full()
            .h(px(40.0))
            .flex_shrink_0()
            .gap(px(4.0))
            .p(px(4.0))
            .rounded(RadiusTokens::value(RadiusStep::Lg))
            .overflow_hidden()
            .bg(control_gradient(&theme))
            .shadow(card_shadows(&theme))
            .child(self.picker.tabs_surface.track());
        // The light goes to the active tab once that tab has been measured:
        // the probe records it while painting, and repaints when it moved.
        let measured_engine = self.picker.light_tab_engine.replace(browsing);
        if let (Some(engine), Some(surface), Some(tab)) = (
            browsing.filter(|engine| measured_engine == Some(*engine)),
            self.picker.tabs_surface.get(),
            self.picker.light_tab.get(),
        ) {
            self.picker.light.borrow_mut().measure(
                engine,
                f32::from(tab.left() - surface.left()),
                f32::from(tab.size.width),
            );
        }
        if browsing.is_some() && self.picker.light.borrow().lit.is_some() {
            tabs = tabs.child(engine_light(&theme, &self.picker.light.borrow()));
        }
        for status in engines {
            let engine = status.engine;
            let gate = Gate::of(status);
            let active = browsing == Some(engine);
            let key = format!("engine:{engine}");
            tabs = tabs.child(
                div()
                    .id(SharedString::from(format!("composer-engine-{engine}")))
                    .role(gpui::Role::Button)
                    .aria_label(engine.display_name())
                    .aria_selected(active)
                    .focusable()
                    .tab_index(0)
                    .relative()
                    .flex()
                    .items_center()
                    .justify_center()
                    .size(px(32.0))
                    .flex_shrink_0()
                    .rounded(RadiusTokens::value(RadiusStep::Xl))
                    .cursor_pointer()
                    .text_color(theme.colors.text.to_paint())
                    .when(gate.is_some(), |tab| {
                        tab.opacity(Opacity::Unavailable.alpha())
                    })
                    .when(active, |tab| tab.child(self.picker.light_tab.track()))
                    .when(self.picker.tooltip.pending() == Some(key.as_str()), |tab| {
                        tab.child(self.picker.tooltip_anchor.track())
                    })
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.reset_model_scroll();
                        this.browsing = Some(engine);
                        this.refresh_picker_catalog(cx);
                        this.close_axis(cx);
                        cx.notify();
                    }))
                    .when_some(gate, |tab, gate| {
                        let reason = gate.reason(engine);
                        let key = key.clone();
                        tab.on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                            if *hovered && this.axis.is_none() {
                                this.begin_tooltip(
                                    key.clone(),
                                    None,
                                    reason.clone(),
                                    tooltip::ABOVE,
                                    cx,
                                );
                            } else {
                                this.clear_tooltip_for(&key);
                            }
                            cx.notify();
                        }))
                    })
                    .child(
                        asset_glyph(engine_icon(&theme, engine))
                            .size(px(16.0))
                            .flex_shrink_0(),
                    ),
            );
        }
        tabs
    }

    /// The model list: the chosen model first, then the
    /// engine's order; the shared hover pill flies between the rows.
    #[expect(
        clippy::too_many_lines,
        reason = "one GPUI list builder keeps the gate, rows and their wiring in visual order"
    )]
    pub(super) fn model_list(
        &self,
        engines: &[EngineStatus],
        browsing: Option<EngineId>,
        _current: Option<&EngineConfig>,
        theme: &ArtisanTheme,
        cx: &Context<Self>,
    ) -> impl IntoElement {
        let theme = *theme;
        let muted = theme.colors.text_muted.to_paint();
        let mut list = div()
            .id("composer-model-list")
            .debug_selector(|| "composer-model-viewport".into())
            .relative()
            .overflow_hidden()
            .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                if !*hovered {
                    this.picker.model_hover.hide();
                    cx.notify();
                }
            }))
            .flex()
            .flex_col()
            .flex_1()
            .min_w(px(0.0))
            .h(px(MODEL_PANEL_HEIGHT_PX))
            .gap(px(3.0))
            .child(self.picker.model_hover.surface_probe())
            .child(self.picker.model_hover.pill(
                "composer-model-hover",
                &theme,
                RadiusTokens::value(RadiusStep::Xl),
                cx.reduce_motion(),
            ));
        let Some(status) =
            browsing.and_then(|engine| engines.iter().find(|status| status.engine == engine))
        else {
            return list.child(
                div()
                    .p(px(12.0))
                    .text_color(muted)
                    .child("Forge reported no engines."),
            );
        };
        let engine = status.engine;
        match Gate::of(status) {
            Some(gate @ (Gate::NotInstalled | Gate::SignIn)) => {
                let action = if gate == Gate::SignIn {
                    format!("Sign in to {}", engine.display_name())
                } else {
                    format!("Install {}", engine.display_name())
                };
                return list.child(
                    div()
                        .p(px(12.0))
                        .text_size(TypographyTokens::size(TextStep::Sm))
                        .text_color(muted)
                        .child(format!(
                            "{} is not ready: {}.",
                            engine.display_name(),
                            gate.label().to_lowercase()
                        ))
                        .child(
                            div()
                                .id(SharedString::from(format!("composer-sign-in-{engine}")))
                                .role(gpui::Role::Button)
                                .aria_label(action.clone())
                                .focusable()
                                .tab_index(0)
                                .mt(px(8.0))
                                .cursor_pointer()
                                .text_color(theme.colors.text.to_paint())
                                .child(action)
                                .on_click(
                                    cx.listener(move |this, _, _, cx| this.sign_in(engine, cx)),
                                ),
                        ),
                );
            }
            Some(gate) if status.models.is_empty() => {
                return list.child(div().p(px(12.0)).text_color(muted).child(gate.label()));
            }
            _ => {}
        }
        let scroll = self.picker.model_scroll.0.borrow().base_handle.clone();
        // Rows differ in height (slim group headers), so a plain scroller on
        // the list's handle rather than a uniform list; the menu holds tens.
        let rows = div()
            .id("composer-model-rows")
            .flex()
            .flex_col()
            .h(px(MODEL_PANEL_HEIGHT_PX))
            .overflow_y_scroll()
            .track_scroll(&scroll)
            .children(self.picker.models.iter().filter_map(|row| {
                let (id, model_index) = match row {
                    ModelRow::Model(id, model_index) => (id, model_index),
                    ModelRow::Group { name, open } => {
                        return Some(Self::group_row(engine, name, *open, &theme, cx));
                    }
                };
                let model = status
                    .models
                    .get(*model_index)
                    .filter(|model| &model.id == id)?;
                let model_id = model.id.clone();
                let hover_id = model.id.clone();
                Some(
                    div()
                        .pb(px(3.0))
                        .child(
                            div()
                                .id(SharedString::from(format!(
                                    "composer-model-{engine}-{}",
                                    model.id
                                )))
                                .role(gpui::Role::MenuItem)
                                .aria_label(model.name.clone())
                                .focusable()
                                .tab_index(0)
                                .debug_selector({
                                    let id = model.id.clone();
                                    move || format!("composer-model-row-{id}")
                                })
                                .relative()
                                .flex()
                                .flex_shrink_0()
                                .items_center()
                                .gap(px(8.0))
                                .h(px(MODEL_ROW_HEIGHT_PX))
                                .px(px(10.0))
                                .rounded(RadiusTokens::value(RadiusStep::Xl))
                                .cursor_pointer()
                                .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                                    if *hovered
                                        && this.menu == Some(Menu::Model)
                                        && this.picker.shown_axis.is_none()
                                    {
                                        this.picker.preview_model =
                                            Some((engine, hover_id.clone()));
                                        this.picker.model_hover.set_active(hover_id.clone());
                                        cx.notify();
                                    }
                                }))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.choose_model(engine, &model_id, cx);
                                }))
                                .child(self.picker.model_hover.row_probe(model.id.clone()))
                                .child(
                                    asset_glyph(
                                        model
                                            .lab
                                            .as_deref()
                                            .and_then(editor_ui::lab_icon)
                                            .unwrap_or_else(|| engine_icon(&theme, engine)),
                                    )
                                    .size(px(20.0))
                                    .text_color(theme.colors.text.to_paint())
                                    .flex_shrink_0(),
                                )
                                .child(
                                    div()
                                        .flex()
                                        .flex_col()
                                        .flex_1()
                                        .min_w(px(0.0))
                                        .overflow_hidden()
                                        .child(
                                            div()
                                                .truncate()
                                                .text_size(TypographyTokens::size(TextStep::Sm))
                                                .line_height(px(20.0))
                                                .font_weight(FontWeight::SEMIBOLD)
                                                .child(model.name.clone()),
                                        )
                                        .child(
                                            div()
                                                .truncate()
                                                .text_size(TypographyTokens::size(TextStep::Xs))
                                                .line_height(px(16.0))
                                                .text_color(muted)
                                                .child(model.lab.clone().unwrap_or_else(|| {
                                                    engine_lab(engine).to_owned()
                                                })),
                                        ),
                                )
                                .into_any_element(),
                        )
                        .into_any_element(),
                )
            }));
        // Painted after the native list, matching transcript wheel routing.
        // This consumes wheel ticks before native scrolling applies them.
        list = list.child(rows).child(
            div()
                .absolute()
                .inset_0()
                .on_scroll_wheel(cx.listener(Self::model_wheel)),
        );
        list
    }

    /// A gateway's header in the model list: a slim muted label with its
    /// chevron at the far end that brightens on hover; a click opens or
    /// closes its models.
    fn group_row(
        engine: EngineId,
        name: &str,
        open: bool,
        theme: &ArtisanTheme,
        cx: &Context<Self>,
    ) -> AnyElement {
        let muted = theme.colors.text_muted.to_paint();
        let foreground = theme.colors.text.to_paint();
        let toggled = name.to_owned();
        let label = if name.is_empty() { "Other" } else { name }.to_owned();
        div()
            .pb(px(3.0))
            .child(
                div()
                    .id(SharedString::from(format!(
                        "composer-model-group-{engine}-{name}"
                    )))
                    .role(gpui::Role::Button)
                    .aria_label(label.clone())
                    .aria_expanded(open)
                    .focusable()
                    .tab_index(0)
                    .relative()
                    .flex()
                    .flex_shrink_0()
                    .items_center()
                    .justify_between()
                    .gap(px(8.0))
                    .h(px(GROUP_ROW_HEIGHT_PX))
                    .px(px(10.0))
                    .cursor_pointer()
                    .text_color(muted)
                    .hover(move |style| style.text_color(foreground))
                    // No pill: the label brightens instead.
                    .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                        if *hovered {
                            this.picker.model_hover.hide();
                            cx.notify();
                        }
                    }))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.toggle_group(engine, toggled.clone(), open, cx);
                    }))
                    .child(
                        div()
                            .min_w(px(0.0))
                            .truncate()
                            .text_size(TypographyTokens::size(TextStep::Xs))
                            .line_height(px(16.0))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(label),
                    )
                    .child(
                        asset_glyph(if open {
                            AssetId::TABLER_CHEVRON_DOWN
                        } else {
                            AssetId::TABLER_CHEVRON_RIGHT
                        })
                        .size(px(14.0))
                        .flex_shrink_0(),
                    ),
            )
            .into_any_element()
    }

    /// The previewed model's name, description and policy controls.
    pub(super) fn model_preview(
        &self,
        engines: &[EngineStatus],
        current: Option<&EngineConfig>,
        theme: &ArtisanTheme,
        cx: &Context<Self>,
    ) -> impl IntoElement {
        let theme = *theme;
        let mut preview = div()
            .id("composer-model-preview")
            .flex()
            .flex_col()
            .justify_between()
            .h(px(MODEL_PANEL_HEIGHT_PX))
            .w(px(MODEL_PREVIEW_WIDTH_PX))
            .flex_shrink_0()
            .min_w(px(0.0))
            .gap(px(8.0))
            .p(px(10.0));
        let Some(config) = current else {
            return preview;
        };
        let Some(status) = engines.iter().find(|status| status.engine == config.engine) else {
            return preview;
        };
        let model = config
            .model
            .as_ref()
            .and_then(|id| status.models.iter().find(|model| &model.id == id));
        let name = model.map_or_else(
            || config.model.clone().unwrap_or_default(),
            |model| model.name.clone(),
        );
        let mut summary = div().flex().flex_col().gap(px(4.0)).child(
            div()
                .truncate()
                .font_weight(FontWeight::SEMIBOLD)
                .text_size(TypographyTokens::size(TextStep::Sm))
                .line_height(px(20.0))
                .child(name),
        );
        let line = model
            .and_then(|model| model.description.clone())
            .unwrap_or_else(|| {
                let mut facts = vec![format!(
                    "{} by {}",
                    model
                        .and_then(|model| model.group.as_deref())
                        .unwrap_or_else(|| config.engine.display_name()),
                    model
                        .and_then(|model| model.lab.as_deref())
                        .unwrap_or_else(|| engine_lab(config.engine))
                )];
                if let Some(context) = model
                    .and_then(|model| config.context_window_override().or(model.context_window))
                {
                    facts.push(format!("{} context", context_label(context)));
                }
                if model.is_some_and(|model| model.supports_images) {
                    facts.push("Reads images".to_owned());
                }
                facts.join(" · ")
            });
        summary = summary.child(
            div()
                .text_size(TypographyTokens::size(TextStep::Xs))
                .line_height(px(16.0))
                .text_color(theme.colors.text_muted.to_paint())
                .child(line),
        );
        preview = preview.child(summary);
        let mut controls = div().flex().flex_col().gap(px(6.0));
        for axis in Axis::ALL {
            let options = axis_options(axis, config, model);
            if axis.shown(options.len()) {
                controls = controls.child(self.axis_control(axis, options, &theme, cx));
            }
        }
        preview.child(controls)
    }
}
