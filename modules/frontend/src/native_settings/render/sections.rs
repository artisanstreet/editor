//! Per-section settings rendering for `SettingsScreen`: the nav rail, every
//! `routes/settings` section body, and the `Render` frame behind the parent
//! re-exports.
//!
//! Split from `native_settings/render.rs`; shared chrome comes from the
//! sibling `chrome` child.

use super::chrome::{
    anchor_list, nav_group_label, nav_heading, nav_link, settings_card, settings_header,
    settings_row, settings_section_shell,
};
use super::*;

impl SettingsScreen {
    /// Builds one disabled static button (`button.svelte` wrappers).
    ///
    /// Returns `None` only when the static label is rejected (empty or
    /// whitespace-only); callers omit the control instead of panicking.
    fn settings_button(
        &self,
        theme: &artisan_ui::theme::ArtisanTheme,
        id: &'static str,
        label: String,
        variant: ButtonVariant,
        selector: &'static str,
    ) -> Option<Button> {
        Button::new(
            id,
            self.focus.control.clone(),
            *theme,
            MotionPolicy::Reduced,
            variant,
            ButtonSize::Small,
            ButtonContent::text(label),
        )
        .ok()
        .map(|button| button.disabled(true).debug_selector(selector))
    }

    /// Builds one disabled static switch (`switch.svelte` wrappers).
    fn settings_switch(
        &self,
        theme: &artisan_ui::theme::ArtisanTheme,
        id: &'static str,
        checked: bool,
        selector: &'static str,
    ) -> Switch {
        Switch::new(
            id,
            self.focus.control.clone(),
            *theme,
            artisan_ui::switch::SwitchSize::Default,
            checked,
        )
        .disabled(true)
        .debug_selector(selector)
    }

    /// Builds one disabled static segmented group (`toggle-group` wrappers).
    fn segmented(
        &self,
        theme: &artisan_ui::theme::ArtisanTheme,
        id: &'static str,
        options: &[(&'static str, &'static str)],
        selected: &str,
        selector: &'static str,
    ) -> ToggleGroup<SharedString> {
        let mut group =
            ToggleGroup::single(id, *theme, Some(SharedString::from(selected.to_owned())))
                .variant(ToggleGroupVariant::Outline)
                .disabled(true)
                .debug_selector(selector);
        for (value, label) in options {
            group = group.item(
                SharedString::from((*value).to_owned()),
                *label,
                self.focus.group.clone(),
            );
        }
        group
    }

    /// Renders the nav rail for the mounted section.
    ///
    /// Every row navigates through [`SettingsScreenEvent::Navigate`]: the
    /// rail is live chrome, not a static fixture. The engine row keeps the
    /// mounted engine id so engine pages switch sections without losing it.
    fn render_nav(&self, theme: &artisan_ui::theme::ArtisanTheme, cx: &mut Context<Self>) -> Div {
        let active = settings_section_for_route(self.section);
        let mut rail = div()
            .flex()
            .flex_col()
            .flex_shrink_0()
            .w(px(SETTINGS_NAV_RAIL_WIDTH_PX))
            .gap(px(4.0))
            .debug_selector(|| SETTINGS_NAV_SELECTOR.to_owned());
        rail = rail.child(nav_heading(theme));
        rail = rail.child(nav_group_label(theme, "ARTISAN"));
        for section in [
            SettingsSection::Models,
            SettingsSection::Threads,
            SettingsSection::Appearance,
            SettingsSection::Notifications,
            SettingsSection::Privacy,
            SettingsSection::About,
        ] {
            let selector = format!("settings-nav-{}", section.label().to_lowercase());
            let target = section_route(section);
            rail = rail.child(
                nav_link(theme, section.label().to_owned(), active == section)
                    .id(selector.clone())
                    .debug_selector(move || selector.clone())
                    .on_click(cx.listener(move |_, _, _, cx| {
                        cx.emit(SettingsScreenEvent::Navigate {
                            section: target,
                            engine: None,
                        });
                    })),
            );
            if active == section {
                rail = rail.child(anchor_list(
                    theme,
                    visible_anchors(section, self.engine_enabled),
                ));
            }
        }
        rail = rail.child(nav_group_label(theme, "ENGINES"));
        if self.engines.is_empty() {
            let engine_active = active == SettingsSection::Engines;
            let engine_selector = format!(
                "settings-nav-engines-{}",
                self.engine_id.as_deref().unwrap_or(FIXTURE_ENGINE_ID)
            );
            let engine_id = self.engine_id.clone();
            rail = rail.child(
                nav_link(theme, self.engine_label(), engine_active)
                    .id(engine_selector.clone())
                    .debug_selector(move || engine_selector.clone())
                    .on_click(cx.listener(move |_, _, _, cx| {
                        cx.emit(SettingsScreenEvent::Navigate {
                            section: SettingsRoute::Engines,
                            engine: engine_id.clone(),
                        });
                    })),
            );
            if engine_active {
                rail = rail.child(anchor_list(
                    theme,
                    visible_anchors(SettingsSection::Engines, self.engine_enabled),
                ));
            }
            return rail;
        }
        for entry in self.engines.clone() {
            let row_active = active == SettingsSection::Engines
                && self.engine_id.as_deref() == Some(entry.id.as_str());
            let selector = format!("settings-nav-engines-{}", entry.id);
            let target = entry.id.clone();
            rail = rail.child(
                nav_link(theme, entry.label.clone(), row_active)
                    .id(selector.clone())
                    .debug_selector(move || selector.clone())
                    .on_click(cx.listener(move |_, _, _, cx| {
                        cx.emit(SettingsScreenEvent::Navigate {
                            section: SettingsRoute::Engines,
                            engine: Some(target.clone()),
                        });
                    })),
            );
            if row_active {
                rail = rail.child(anchor_list(theme, SettingsSection::Engines.anchors()));
            }
        }
        rail
    }

    /// Renders the models section (`models.svelte`, `compaction-model.svelte`).
    ///
    /// The compaction picker popover and the favorite toggles need the
    /// session-defaults controller; the trigger paints the fixture
    /// `Curated` value and favorites paint the legacy empty state.
    fn render_models(&self, theme: &artisan_ui::theme::ArtisanTheme) -> Div {
        let compaction = settings_card(
            theme,
            vec![settings_row(
                theme,
                "Cross-transfer compaction model",
                "Choose who writes a hand-off summary when a thread moves to another engine or model.",
                self.settings_button(
                    theme,
                    "settings-compaction-trigger",
                    "Curated \u{25be}".to_owned(),
                    ButtonVariant::Outline,
                    "settings-compaction-trigger",
                )
                .map(IntoElement::into_any_element),
            )],
        );
        let favorites = settings_card(
            theme,
            vec![
                div()
                    .w_full()
                    .flex()
                    .flex_col()
                    .items_center()
                    .px(px(16.0))
                    .py(px(28.0))
                    .child(
                        div()
                            .text_xs()
                            .text_color(theme.colors.muted_foreground.to_paint())
                            .child(
                                "No favorites yet. Starred models float to the top of every model picker; star one from the composer's picker or an engine page.",
                            ),
                    ),
            ],
        );
        div()
            .flex()
            .flex_col()
            .child(settings_header(
                theme,
                SettingsSection::Models.title().to_owned(),
                SettingsSection::Models.description(),
            ))
            .child(settings_section_shell(
                theme,
                "compaction",
                "Compaction",
                Some("Compaction choices are not yet configurable in the native app."),
                None,
                compaction,
            ))
            .child(settings_section_shell(
                theme,
                "favorites",
                "Favorites",
                None,
                None,
                favorites,
            ))
    }

    /// Renders the appearance app-icon and typography sections.
    fn render_appearance_top(&self, theme: &artisan_ui::theme::ArtisanTheme) -> Div {
        let app_icon = settings_card(
            theme,
            vec![
                Self::app_icon_option(theme, APPEARANCE_APP_ICON_DEFAULT_LABEL, "Default", true),
                Self::app_icon_option(
                    theme,
                    APPEARANCE_APP_ICON_ALTERNATE_LABEL,
                    "Alternate",
                    false,
                ),
            ],
        );
        let restore = self
            .settings_button(
                theme,
                "settings-typography-restore",
                "Restore defaults".to_owned(),
                ButtonVariant::Ghost,
                "settings-typography-restore",
            )
            .map_or_else(|| div().into_any_element(), IntoElement::into_any_element);
        let typography = settings_card(
            theme,
            vec![
                Self::typography_preview(theme),
                settings_row(
                    theme,
                    "Text",
                    "Interface controls, page titles, and reading text.",
                    Some(self.font_trigger(theme, "text", &self.text_font)),
                ),
                settings_row(
                    theme,
                    "Code",
                    "Code, terminals, and the editor.",
                    Some(self.font_trigger(theme, "code", &self.code_font)),
                ),
            ],
        );
        div().flex().flex_col()
            .child(settings_section_shell(theme, "app-icon", "App icon", None, None, app_icon))
            .child(
                div()
                    .mt(px(10.0))
                    .text_xs()
                    .text_color(theme.colors.muted_foreground.to_paint())
                    .child("Open Appearance in the Artisan desktop app to switch its runtime icon."),
            )
            .child(settings_section_shell(
                theme,
                "typography",
                "Typography",
                Some("Font choices are not yet configurable in the native app."),
                Some(restore),
                typography,
            ))
            .child(
                div()
                    .mt(px(10.0))
                    .text_xs()
                    .text_color(theme.colors.muted_foreground.to_paint())
                    .child(
                        "Local font names are requested only when you open a picker. The list stays on this device; Artisan saves only the two family names you choose.",
                    ),
            )
    }

    /// Paints one app-icon option as a disabled static row.
    fn app_icon_option(
        theme: &artisan_ui::theme::ArtisanTheme,
        label: &str,
        caption: &str,
        selected: bool,
    ) -> Div {
        let mut row = div()
            .w_full()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(12.0))
            .rounded(px(12.0))
            .px(px(12.0))
            .py(px(12.0))
            .border_1()
            .border_color(theme.colors.border.to_paint());
        if selected {
            row = row.bg(theme.colors.muted.to_paint());
        }
        row.child(
            div()
                .flex()
                .flex_col()
                .flex_1()
                .min_w_0()
                .gap(px(4.0))
                .child(
                    div()
                        .text_sm()
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(theme.colors.foreground.to_paint())
                        .child(label.to_owned()),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(theme.colors.muted_foreground.to_paint())
                        .child(caption.to_owned()),
                ),
        )
    }

    /// Paints the text/code preview panes of the typography card.
    fn typography_preview(theme: &artisan_ui::theme::ArtisanTheme) -> Div {
        div()
            .w_full()
            .flex()
            .flex_row()
            .gap(px(16.0))
            .py(px(16.0))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .gap(px(8.0))
                    .child(
                        div()
                            .text_xs()
                            .text_color(theme.colors.muted_foreground.to_paint())
                            .child("TEXT"),
                    )
                    .child(
                        div()
                            .text_sm()
                            .text_color(theme.colors.foreground.to_paint())
                            .child("Quiet tools, clear decisions."),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .gap(px(8.0))
                    .child(
                        div()
                            .text_xs()
                            .text_color(theme.colors.muted_foreground.to_paint())
                            .child("CODE"),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(theme.colors.foreground.to_paint())
                            .child("craft = deliberate"),
                    ),
            )
    }

    /// Paints one font-picker trigger (`font-picker.svelte`).
    ///
    /// The full command popover needs browser font discovery; the trigger
    /// paints the fixture family with the legacy selector glyph.
    fn font_trigger(
        &self,
        theme: &artisan_ui::theme::ArtisanTheme,
        role: &str,
        family: &str,
    ) -> AnyElement {
        let id = if role == "code" {
            "settings-font-code"
        } else {
            "settings-font-text"
        };
        let label = format!("{family} \u{25be}");
        self.settings_button(theme, id, label.clone(), ButtonVariant::Outline, id)
            .map_or_else(
                || div().child(label).into_any_element(),
                IntoElement::into_any_element,
            )
    }

    /// Renders the appearance formatting, glass, and reading sections.
    #[expect(
        clippy::too_many_lines,
        reason = "one GPUI render builder keeps the three appearance sub-sections in visual order; splitting it would scatter the sheet layout"
    )]
    fn render_appearance_bottom(&self, theme: &artisan_ui::theme::ArtisanTheme) -> Div {
        let time_options: [(&'static str, &'static str); 2] =
            [("12-hour", "12-hour"), ("24-hour", "24-hour")];
        let separator_options: [(&'static str, &'static str); 2] =
            [("backslash", "\\"), ("forward-slash", "/")];
        let width_options: [(&'static str, &'static str); 3] = [
            ("tight", "Tight"),
            ("balanced", "Balanced"),
            ("loose", "Loose"),
        ];
        let formatting = settings_card(
            theme,
            vec![
                settings_row(
                    theme,
                    "Time format",
                    "How local times are written throughout Artisan.",
                    Some(
                        self.segmented(
                            theme,
                            "settings-time-format",
                            &time_options,
                            self.time_format.as_str(),
                            "settings-time-format",
                        )
                        .into_any_element(),
                    ),
                ),
                settings_row(
                    theme,
                    "Path separator",
                    "Which separator file and folder paths use when displayed.",
                    Some(
                        self.segmented(
                            theme,
                            "settings-path-separator",
                            &separator_options,
                            self.path_separator.as_str(),
                            "settings-path-separator",
                        )
                        .into_any_element(),
                    ),
                ),
            ],
        );
        let glass = settings_card(
            theme,
            vec![settings_row(
                theme,
                "Shader under glass",
                "Lights glass surfaces with the animated shader they were designed around. Turning it off leaves the glass itself intact \u{2014} the material, highlight, and depth stay \u{2014} and only the moving light stops.",
                Some(
                    self.settings_switch(
                        theme,
                        "settings-switch-shader",
                        self.shader_enabled,
                        "settings-switch-shader",
                    )
                    .into_any_element(),
                ),
            )],
        );
        let reading = settings_card(
            theme,
            vec![settings_row(
                theme,
                "Prose width",
                "How wide the transcript's reading column runs. Balanced is the width Artisan was designed at; Tight shortens the line for focus, Loose spends more of the window on text.",
                Some(
                    self.segmented(
                        theme,
                        "settings-prose-width",
                        &width_options,
                        self.prose_width.as_str(),
                        "settings-prose-width",
                    )
                    .into_any_element(),
                ),
            )],
        );
        div()
            .flex()
            .flex_col()
            .child(settings_section_shell(
                theme,
                "formatting",
                "Formatting",
                Some("Time and path display choices are not yet configurable in the native app."),
                None,
                formatting,
            ))
            .child(settings_section_shell(
                theme,
                "glass",
                "Glass",
                Some("The shader choice is not yet configurable in the native app."),
                None,
                glass,
            ))
            .child(settings_section_shell(
                theme,
                "reading",
                "Reading",
                Some("The prose-width choice is not yet configurable in the native app."),
                None,
                reading,
            ))
    }

    /// Renders the appearance section (`appearance.svelte`).
    fn render_appearance(
        &self,
        theme: &artisan_ui::theme::ArtisanTheme,
        cx: &mut Context<Self>,
    ) -> Div {
        div()
            .flex()
            .flex_col()
            .child(settings_header(
                theme,
                SettingsSection::Appearance.title().to_owned(),
                SettingsSection::Appearance.description(),
            ))
            .child(self.render_appearance_top(theme))
            .child(self.render_frame_rate(theme, cx))
            .child(self.render_appearance_bottom(theme))
    }

    fn render_frame_rate(
        &self,
        theme: &artisan_ui::theme::ArtisanTheme,
        cx: &mut Context<Self>,
    ) -> Div {
        use crate::native_frame_rate::{self, FrameRateLimit};
        use artisan_ui::select::{Select, SelectEntry};
        let on_open = cx.listener(|screen, open: &bool, _, cx| {
            screen.frame_rate_control.open = *open;
            cx.notify();
        });
        let on_change = cx.listener(|screen, limit: &FrameRateLimit, window, cx| {
            screen.frame_rate_control.error = native_frame_rate::apply(*limit, window, cx).err();
            screen.frame_rate_control.open = false;
            cx.notify();
        });
        let select = Select::new(
            "settings-frame-rate-limit",
            self.focus.control.clone(),
            *theme,
            Some(native_frame_rate::current(cx)),
            FrameRateLimit::OPTIONS
                .iter()
                .map(|limit| SelectEntry::item(*limit, limit.label()))
                .collect(),
        )
        .open(self.frame_rate_control.open)
        .with_interaction_state(self.frame_rate_control.interaction.clone())
        .with_scroll_handle(self.frame_rate_control.scroll.clone())
        .debug_selector("settings-frame-rate-limit")
        .on_open_change(move |open, window, cx| on_open(&open, window, cx))
        .on_change(move |limit, _, window, cx| on_change(&limit, window, cx));
        let on_overlay = cx.listener(|screen, visible: &bool, window, cx| {
            screen.frame_rate_control.error =
                native_frame_rate::apply_overlay(*visible, window, cx).err();
            cx.notify();
        });
        let overlay = Switch::new(
            "settings-fps-overlay",
            self.focus.fps_overlay.clone(),
            *theme,
            artisan_ui::switch::SwitchSize::Default,
            native_frame_rate::overlay_visible(cx),
        )
        .debug_selector("settings-fps-overlay")
        .on_change(move |visible, _, window, cx| on_overlay(&visible, window, cx));
        settings_section_shell(
            theme,
            "performance",
            "Performance",
            self.frame_rate_control.error.as_deref(),
            None,
            settings_card(
                theme,
                vec![
                    settings_row(
                        theme,
                        "FPS limit",
                        "Limit animation and scrolling redraws. Unlimited disables VSync and the frame cap.",
                        Some(
                            div()
                                .w(px(144.0))
                                .flex_shrink_0()
                                .child(select)
                                .into_any_element(),
                        ),
                    ),
                    settings_row(
                        theme,
                        "Show FPS overlay",
                        "Display FPS and frame timings in the top-right corner.",
                        Some(overlay.into_any_element()),
                    ),
                ],
            ),
        )
    }

    /// Renders the engines page (`engine.svelte`).
    ///
    /// With a live [`SettingsEngineSnapshot`] the page paints the actual
    /// catalog, account, registry, and thread configuration with working
    /// refresh and model-choice actions; without one it keeps the static
    /// fixture copy with every action disabled. The switched-off branch of
    /// the fixture hides account and models exactly like legacy.
    fn render_engine(
        &self,
        theme: &artisan_ui::theme::ArtisanTheme,
        cx: &mut Context<Self>,
    ) -> Div {
        let engine_id = self.engine_id.as_deref().unwrap_or(FIXTURE_ENGINE_ID);
        if !self.engine_known {
            return div().flex().flex_col().child(settings_header(
                theme,
                SETTINGS_UNKNOWN_ENGINE_TITLE.to_owned(),
                &unknown_engine_description(engine_id),
            ));
        }
        if let Some(snapshot) = self.engine_snapshot() {
            return self.render_live_engine(theme, snapshot, cx);
        }
        let label = self.engine_label();
        let description = format!(
            "Choose where {label} appears, manage its installation, and inspect its account and models."
        );
        let availability = settings_card(
            theme,
            vec![settings_row(
                theme,
                &format!("Enable {label}"),
                "Whether this engine is represented as available at all. Off, its models leave the model picker and its account is never asked for usage.",
                Some(
                    self.settings_switch(
                        theme,
                        "settings-switch-engine",
                        self.engine_enabled,
                        "settings-switch-engine",
                    )
                    .into_any_element(),
                ),
            )],
        );
        let mut page = div()
            .flex()
            .flex_col()
            .child(settings_header(theme, label.clone(), &description))
            .child(settings_section_shell(
                theme,
                "availability",
                "Availability",
                None,
                None,
                availability,
            ))
            .child(self.render_engine_installation(theme));
        if self.engine_enabled {
            page = page
                .child(self.render_engine_account(theme))
                .child(Self::render_engine_models(theme, engine_id, &label));
        } else {
            page = page.child(
                div()
                    .mt(px(32.0))
                    .text_sm()
                    .text_color(theme.colors.muted_foreground.to_paint())
                    .child(format!(
                        "{label} is switched off. Its models are hidden everywhere until it is enabled again."
                    )),
            );
        }
        page
    }

    /// Renders one live engine page from its orchestrator snapshot.
    ///
    /// Availability reads out the probed account verdict with a state badge
    /// instead of the fixture switch: native engines follow their installed
    /// CLI and account state and expose no separate enable control.
    /// Installation, account, and models paint the live check state with
    /// working refresh and model-choice actions; model configuration stays
    /// explicitly thread-scoped.
    fn render_live_engine(
        &self,
        theme: &artisan_ui::theme::ArtisanTheme,
        snapshot: &SettingsEngineSnapshot,
        cx: &mut Context<Self>,
    ) -> Div {
        let label = self.engine_label();
        let description = format!(
            "Choose where {label} appears, manage its installation, and inspect its account and models."
        );
        let availability = settings_card(
            theme,
            vec![settings_row(
                theme,
                "Availability",
                "Whether this engine can run models. Native engines follow their installed CLI and account state; there is no separate switch.",
                Some(
                    div()
                        .flex_shrink_0()
                        .child(outline_badge(
                            BadgeStyle::resolve(*theme),
                            snapshot.availability_badge(),
                        ))
                        .into_any_element(),
                ),
            )],
        );
        div()
            .flex()
            .flex_col()
            .child(settings_header(theme, label, &description))
            .child(settings_section_shell(
                theme,
                "availability",
                "Availability",
                None,
                None,
                availability,
            ))
            .child(Self::render_live_engine_installation(theme, snapshot, cx))
            .child(Self::render_live_engine_account(theme, snapshot, cx))
            .child(self.render_live_engine_models(theme, snapshot, cx))
    }

    /// Builds one live ghost-style action trigger emitting a screen event.
    fn live_action(
        selector: String,
        label: &'static str,
        event: SettingsScreenEvent,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        div()
            .id(selector.clone())
            .debug_selector(move || selector.clone())
            .on_click(cx.listener(move |_, _, _, cx| {
                cx.emit(event.clone());
            }))
            .p(px(4.0))
            .text_sm()
            .child(label.to_owned())
            .into_any_element()
    }

    /// Builds the engine refresh trigger for one live section.
    fn refresh_action(
        snapshot: &SettingsEngineSnapshot,
        selector: &'static str,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let engine_id = snapshot.engine_id.clone();
        Self::live_action(
            selector.to_owned(),
            if snapshot.refreshing {
                "Checking…"
            } else {
                "Refresh"
            },
            SettingsScreenEvent::RefreshEngine { engine_id },
            cx,
        )
    }

    /// Renders the live installation section from the probed verdict.
    fn render_live_engine_installation(
        theme: &artisan_ui::theme::ArtisanTheme,
        snapshot: &SettingsEngineSnapshot,
        cx: &mut Context<Self>,
    ) -> Div {
        let body = settings_card(
            theme,
            vec![
                div()
                    .w_full()
                    .flex()
                    .flex_col()
                    .gap(px(4.0))
                    .py(px(20.0))
                    .child(
                        div()
                            .text_sm()
                            .text_color(theme.colors.foreground.to_paint())
                            .child(snapshot.installation_state()),
                    ),
            ],
        );
        settings_section_shell(
            theme,
            "installation",
            "Installation",
            None,
            Some(Self::refresh_action(
                snapshot,
                "settings-installation-refresh",
                cx,
            )),
            body,
        )
    }

    /// Renders the live account section from the probed verdict.
    fn render_live_engine_account(
        theme: &artisan_ui::theme::ArtisanTheme,
        snapshot: &SettingsEngineSnapshot,
        cx: &mut Context<Self>,
    ) -> Div {
        let body = settings_card(
            theme,
            vec![
                div()
                    .w_full()
                    .flex()
                    .flex_col()
                    .gap(px(4.0))
                    .py(px(20.0))
                    .child(
                        div()
                            .text_sm()
                            .text_color(theme.colors.foreground.to_paint())
                            .child(snapshot.account_state()),
                    ),
            ],
        );
        settings_section_shell(
            theme,
            "account",
            "Account",
            None,
            Some(Self::refresh_action(snapshot, "settings-usage-refresh", cx)),
            body,
        )
    }

    /// Renders the live model rows with working choice actions.
    ///
    /// Every supported row chooses its model through
    /// [`SettingsScreenEvent::SelectEngineModel`], which the orchestrator
    /// serves through the existing `SelectPolicy` plus shared typed-save
    /// flow — the same path as the composer picker, with the same
    /// compare-and-swap save and acknowledgment. Catalog-disabled rows stay
    /// unclickable with their honest reason. Model configuration is
    /// thread-scoped: without a selected thread the rows name that instead
    /// of pretending to save.
    fn render_live_engine_models(
        &self,
        theme: &artisan_ui::theme::ArtisanTheme,
        snapshot: &SettingsEngineSnapshot,
        cx: &mut Context<Self>,
    ) -> Div {
        let label = self.engine_label();
        let mut blocks = Vec::new();
        if let Some(notice) = snapshot.choice_notice.as_deref() {
            blocks.push(
                div()
                    .w_full()
                    .py(px(12.0))
                    .text_sm()
                    .text_color(theme.colors.muted_foreground.to_paint())
                    .child(notice.to_owned()),
            );
        }
        if snapshot.save_failed {
            blocks.push(
                div()
                    .w_full()
                    .py(px(12.0))
                    .text_sm()
                    .text_color(theme.colors.destructive.to_paint())
                    .child(
                        "The model configuration was not saved. Retry the choice or the save action.",
                    ),
            );
        }
        if snapshot.models.is_empty() {
            blocks.push(
                div()
                    .w_full()
                    .py(px(20.0))
                    .text_sm()
                    .text_color(theme.colors.muted_foreground.to_paint())
                    .child(format!("No models are listed for {label} yet.")),
            );
        }
        for model in &snapshot.models {
            blocks.push(self.live_engine_model_row(theme, snapshot, model, cx));
        }
        if snapshot.selected_thread.is_none() {
            blocks.push(
                div()
                    .w_full()
                    .py(px(12.0))
                    .text_xs()
                    .text_color(theme.colors.muted_foreground.to_paint())
                    .child(format!(
                        "Engine models save to the selected thread. Select a thread to configure {label}."
                    )),
            );
        } else if let (Some(saved), Some(profile)) = (
            snapshot.saved_model.as_deref(),
            snapshot.saved_profile.as_deref(),
        ) {
            blocks.push(
                div()
                    .w_full()
                    .py(px(12.0))
                    .text_xs()
                    .text_color(theme.colors.muted_foreground.to_paint())
                    .child(format!(
                        "Saved for this thread: {saved} on profile {profile}."
                    )),
            );
        }
        let action = if snapshot.pending_save {
            Some(
                div()
                    .p(px(4.0))
                    .text_sm()
                    .text_color(theme.colors.muted_foreground.to_paint())
                    .child("Saving…")
                    .into_any_element(),
            )
        } else if snapshot.can_save_displayed {
            let engine_id = snapshot.engine_id.clone();
            Some(Self::live_action(
                "settings-engine-save-model".to_owned(),
                "Save",
                SettingsScreenEvent::SaveDisplayedModel { engine_id },
                cx,
            ))
        } else {
            None
        };
        settings_section_shell(
            theme,
            "models",
            "Models",
            None,
            action,
            settings_card(theme, blocks),
        )
    }

    /// Paints one live engine model row with its state badges.
    ///
    /// Selectable rows (supported model, thread selected) emit
    /// [`SettingsScreenEvent::SelectEngineModel`] on activation; disabled
    /// rows and thread-less pages paint without an action.
    fn live_engine_model_row(
        &self,
        theme: &artisan_ui::theme::ArtisanTheme,
        snapshot: &SettingsEngineSnapshot,
        model: &SettingsEngineModel,
        cx: &mut Context<Self>,
    ) -> Div {
        let mut row = div()
            .w_full()
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .gap(px(16.0))
            .py(px(10.0))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .child(
                        div()
                            .text_sm()
                            .text_color(theme.colors.foreground.to_paint())
                            .child(model.id.clone()),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(theme.colors.muted_foreground.to_paint())
                            .child(self.engine_label()),
                    ),
            );
        if model.saved {
            row = row.child(
                div()
                    .flex_shrink_0()
                    .child(outline_badge(BadgeStyle::resolve(*theme), "Saved")),
            );
        } else if model.displayed {
            row = row.child(
                div()
                    .flex_shrink_0()
                    .child(outline_badge(BadgeStyle::resolve(*theme), "Selected")),
            );
        }
        if let Some(reason) = model.disabled_reason.as_deref() {
            row = row.child(
                div()
                    .flex()
                    .flex_col()
                    .flex_shrink_0()
                    .items_end()
                    .child(outline_badge(BadgeStyle::resolve(*theme), "Disabled"))
                    .child(
                        div()
                            .text_xs()
                            .text_color(theme.colors.muted_foreground.to_paint())
                            .child(reason.to_owned()),
                    ),
            );
            return row;
        }
        if snapshot.selected_thread.is_some() {
            let engine_id = snapshot.engine_id.clone();
            let model_id = model.id.clone();
            let selector = format!("settings-engine-model-{model_id}");
            row = div().w_full().child(
                row.id(selector.clone())
                    .debug_selector(move || selector.clone())
                    .on_click(cx.listener(move |_, _, _, cx| {
                        cx.emit(SettingsScreenEvent::SelectEngineModel {
                            engine_id: engine_id.clone(),
                            model_id: model_id.clone(),
                        });
                    })),
            );
        }
        row
    }

    /// Renders the engine installation section with fixture status copy.
    fn render_engine_installation(&self, theme: &artisan_ui::theme::ArtisanTheme) -> Div {
        let action = self
            .settings_button(
                theme,
                "settings-installation-check",
                "Check for updates".to_owned(),
                ButtonVariant::Ghost,
                "settings-installation-check",
            )
            .map_or_else(|| div().into_any_element(), IntoElement::into_any_element);
        let body = settings_card(
            theme,
            vec![
                div()
                    .w_full()
                    .flex()
                    .flex_col()
                    .gap(px(4.0))
                    .py(px(20.0))
                    .child(
                        div()
                            .text_sm()
                            .text_color(theme.colors.foreground.to_paint())
                            .child("Managed installation ready."),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(theme.colors.muted_foreground.to_paint())
                            .child("Managed by Artisan with an isolated provider home."),
                    ),
            ],
        );
        settings_section_shell(
            theme,
            "installation",
            "Installation",
            None,
            Some(action),
            body,
        )
    }

    /// Renders the engine account section with the honest unknown state.
    fn render_engine_account(&self, theme: &artisan_ui::theme::ArtisanTheme) -> Div {
        let action = self
            .settings_button(
                theme,
                "settings-usage-refresh",
                "Refresh".to_owned(),
                ButtonVariant::Ghost,
                "settings-usage-refresh",
            )
            .map_or_else(|| div().into_any_element(), IntoElement::into_any_element);
        let body = settings_card(
            theme,
            vec![
                div().w_full().flex().flex_col().py(px(20.0)).child(
                    div()
                        .text_sm()
                        .text_color(theme.colors.muted_foreground.to_paint())
                        .child("Sign-in status unknown"),
                ),
            ],
        );
        settings_section_shell(theme, "account", "Account", None, Some(action), body)
    }

    /// Renders the engine model rows from the fixture catalog.
    fn render_engine_models(
        theme: &artisan_ui::theme::ArtisanTheme,
        engine_id: &str,
        label: &str,
    ) -> Div {
        let models = models_for_fixture_engine(engine_id);
        let mut blocks = Vec::new();
        if models.is_empty() {
            blocks.push(
                div()
                    .w_full()
                    .py(px(20.0))
                    .text_sm()
                    .text_color(theme.colors.muted_foreground.to_paint())
                    .child(format!("No models are listed for {label} yet.")),
            );
        }
        for model in &models {
            blocks.push(Self::engine_model_row(theme, model, label));
        }
        settings_section_shell(
            theme,
            "models",
            "Models",
            None,
            None,
            settings_card(theme, blocks),
        )
    }

    /// Paints one engine model row with its badges (`engine.svelte`).
    ///
    /// Variant counts and the compaction-default mark need the full catalog
    /// snapshot; the `Disabled` badge renders from the fixture definition
    /// today.
    fn engine_model_row(
        theme: &artisan_ui::theme::ArtisanTheme,
        model: &crate::model_selection_presentation::ModelChoice,
        label: &str,
    ) -> Div {
        let mut row = div()
            .w_full()
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .gap(px(16.0))
            .py(px(10.0))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .child(
                        div()
                            .text_sm()
                            .text_color(theme.colors.foreground.to_paint())
                            .child(model.id.clone()),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(theme.colors.muted_foreground.to_paint())
                            .child(label.to_owned()),
                    ),
            );
        if model.definition.disabled.is_some() {
            row = row.child(
                div()
                    .flex_shrink_0()
                    .child(outline_badge(BadgeStyle::resolve(*theme), "Disabled")),
            );
        }
        row
    }

    /// Renders the notifications section (`notifications.svelte`).
    ///
    /// The enable switch and re-check button need the system-notifications
    /// service; the gap notice renders from the injected snapshot so wired
    /// states already show the legacy blocked/unprompted copy.
    fn render_notifications(&self, theme: &artisan_ui::theme::ArtisanTheme) -> Div {
        let gap = system_notification_gap_for(self.notifications);
        let switch_control = if gap == SystemNotificationGap::Unsupported {
            settings_row(
                theme,
                "Notify me",
                "This host exposes no notification API, so there is nothing for Artisan to post to.",
                Some(
                    self.settings_switch(
                        theme,
                        "settings-switch-notify",
                        false,
                        "settings-switch-notify",
                    )
                    .into_any_element(),
                ),
            )
        } else {
            settings_row(
                theme,
                "Notify me",
                "Posts a system notification when a thread finishes, fails, or needs an answer from you. Artisan appears in your operating system's notification settings, where you can silence or restyle it.",
                Some(
                    self.settings_switch(
                        theme,
                        "settings-switch-notify",
                        self.notifications.enabled,
                        "settings-switch-notify",
                    )
                    .into_any_element(),
                ),
            )
        };
        let mut blocks = vec![switch_control];
        if let Some((title, description)) = notification_gap_notice(gap) {
            let retry = self
                .settings_button(
                    theme,
                    "settings-notifications-retry",
                    "Check again".to_owned(),
                    ButtonVariant::Outline,
                    "settings-notifications-retry",
                )
                .map(IntoElement::into_any_element);
            blocks.push(settings_row(theme, title, description, retry));
        }
        blocks.push(settings_row(
            theme,
            "Clears itself",
            "A notification you don't answer disappears after a few seconds. Nothing is lost by letting it go \u{2014} an approval or a question keeps waiting in its thread \u{2014} so a notification can never pile up into something you have to go and dismiss.",
            None,
        ));
        div()
            .flex()
            .flex_col()
            .child(settings_header(
                theme,
                SettingsSection::Notifications.title().to_owned(),
                SettingsSection::Notifications.description(),
            ))
            .child(settings_section_shell(
                theme,
                "system",
                "System",
                Some("Notification controls are not yet connected in the native app."),
                None,
                settings_card(theme, blocks),
            ))
    }

    /// Renders the privacy section (`privacy.svelte`).
    fn render_privacy(&self, theme: &artisan_ui::theme::ArtisanTheme) -> Div {
        let telemetry = settings_card(
            theme,
            vec![
                self.telemetry_row(
                    theme,
                    "settings-switch-usage-analytics",
                    "Usage analytics",
                    "Sends allowlisted product events to PostHog using a random installation ID. Artisan never sends prompts, responses, source code, diffs, terminal activity, names, or paths.",
                    self.telemetry.usage_analytics,
                ),
                self.telemetry_row(
                    theme,
                    "settings-switch-crash-reports",
                    "Crash reports",
                    "Sends sanitized exceptions, crash reasons, release information, and coarse performance diagnostics to Sentry. Attachments, replay, console logs, request data, environment variables, and process arguments stay off.",
                    self.telemetry.crash_reports,
                ),
            ],
        );
        let never = settings_card(
            theme,
            vec![settings_row(
                theme,
                "Your work stays local",
                "Prompts, model responses, source code, file contents, diffs, terminal commands and output, repository and project names, paths, credentials, headers, request bodies, process arguments, and environment variables are prohibited from both systems.",
                None,
            )],
        );
        div()
            .flex()
            .flex_col()
            .child(settings_header(
                theme,
                SettingsSection::Privacy.title().to_owned(),
                SettingsSection::Privacy.description(),
            ))
            .child(settings_section_shell(
                theme,
                "telemetry",
                "Observability",
                Some("Telemetry choices are not yet configurable in the native app."),
                None,
                telemetry,
            ))
            .child(settings_section_shell(
                theme,
                "never-collected",
                "Never collected",
                None,
                None,
                never,
            ))
    }

    /// Paints one telemetry row with its choice caption and switch.
    fn telemetry_row(
        &self,
        theme: &artisan_ui::theme::ArtisanTheme,
        id: &'static str,
        title: &str,
        description: &str,
        choice: TelemetryPreference,
    ) -> Div {
        let control = div()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(8.0))
            .child(
                div()
                    .text_xs()
                    .text_color(theme.colors.muted_foreground.to_paint())
                    .child(telemetry_choice_caption(choice).to_owned()),
            )
            .child(
                self.settings_switch(theme, id, choice == TelemetryPreference::Enabled, id)
                    .into_any_element(),
            );
        settings_row(theme, title, description, Some(control.into_any_element()))
    }

    /// Renders the threads section (`threads.svelte` plus `thread-titles`,
    /// `usage-recovery`, and `agent-names`).
    ///
    /// Retention, titles, recovery, and the name set need the retention and
    /// session-defaults controllers. The threshold paints as static text
    /// because the numeric input has no wired commit path yet; the agent
    /// name set uses the real closed `Select`.
    fn render_threads(&self, theme: &artisan_ui::theme::ArtisanTheme) -> Div {
        div()
            .flex()
            .flex_col()
            .child(settings_header(
                theme,
                SettingsSection::Threads.title().to_owned(),
                SettingsSection::Threads.description(),
            ))
            .child(self.render_thread_retention(theme))
            .child(self.render_thread_titles(theme))
            .child(self.render_usage_recovery(theme))
            .child(self.render_agent_names(theme))
    }

    /// Renders the retention section with its loading and unverified states.
    fn render_thread_retention(&self, theme: &artisan_ui::theme::ArtisanTheme) -> Div {
        let mut column = div().flex().flex_col();
        if matches!(
            self.retention.policy_state(),
            ThreadRetentionPolicyState::Unverified
        ) {
            column = column.child(self.retention_unverified_banner(theme));
        }
        column.child(settings_section_shell(
            theme,
            "retention",
            "Retention",
            Some("Retention controls are not yet connected in the native app."),
            None,
            self.retention_body(theme),
        ))
    }

    /// Paints the unverified-policy warning banner above the retention card.
    fn retention_unverified_banner(&self, theme: &artisan_ui::theme::ArtisanTheme) -> Div {
        div()
            .w_full()
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .gap(px(16.0))
            .mt(px(12.0))
            .px(px(12.0))
            .py(px(12.0))
            .rounded(px(12.0))
            .border_1()
            .border_color(theme.colors.banner_warning.to_paint())
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .gap(px(2.0))
                    .child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(theme.colors.foreground.to_paint())
                            .child(self.retention.failure_title().to_owned()),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(theme.colors.muted_foreground.to_paint())
                            .child(
                                "Forge could not confirm the durable policy. Controls are disabled.",
                            ),
                    ),
            )
            .child(
                self.settings_button(
                    theme,
                    "settings-retention-retry",
                    "Retry".to_owned(),
                    ButtonVariant::Outline,
                    "settings-retention-retry",
                )
                .map_or_else(
                    || div().into_any_element(),
                    IntoElement::into_any_element,
                ),
            )
    }

    /// Paints the retention card: loading copy or the two policy rows.
    fn retention_body(&self, theme: &artisan_ui::theme::ArtisanTheme) -> Div {
        match self.retention.policy() {
            None => settings_card(
                theme,
                vec![
                    div()
                        .w_full()
                        .py(px(16.0))
                        .text_sm()
                        .text_color(theme.colors.muted_foreground.to_paint())
                        .child("Loading retention policy\u{2026}"),
                ],
            ),
            Some(policy) => settings_card(
                theme,
                vec![
                    settings_row(
                        theme,
                        "Erase inactive threads",
                        "Permanently erases a thread \u{2014} conversation, checkpoints, and lineage \u{2014} once it has been untouched for the configured number of days. This is deletion, not archival.",
                        Some(
                            self.settings_switch(
                                theme,
                                "settings-switch-retention",
                                policy.enabled,
                                "settings-switch-retention",
                            )
                            .into_any_element(),
                        ),
                    ),
                    settings_row(
                        theme,
                        "Inactivity threshold",
                        "Days a thread must be untouched before it is erased. Between 1 and 3650.",
                        Some(
                            div()
                                .text_sm()
                                .text_color(theme.colors.muted_foreground.to_paint())
                                .child(format!("{} days", policy.inactivity_days))
                                .into_any_element(),
                        ),
                    ),
                ],
            ),
        }
    }

    /// Renders the summary-titles section (`thread-titles.svelte`).
    fn render_thread_titles(&self, theme: &artisan_ui::theme::ArtisanTheme) -> Div {
        let mut blocks = vec![settings_row(
            theme,
            "Summary titles",
            "Name threads with the harness's own generated summary. When off, a thread is named by the latest message you sent. A rename of your own always wins, and threads without a summary keep the message title.",
            Some(
                self.settings_switch(
                    theme,
                    "settings-switch-titles",
                    self.thread_title.summarized(),
                    "settings-switch-titles",
                )
                .into_any_element(),
            ),
        )];
        if !self.thread_title.message.is_empty() {
            blocks.push(
                div()
                    .w_full()
                    .py(px(12.0))
                    .text_sm()
                    .text_color(theme.colors.destructive.to_paint())
                    .child(self.thread_title.message.clone()),
            );
        }
        settings_section_shell(
            theme,
            "thread-titles",
            "Titles",
            Some("Title choices are not yet configurable in the native app."),
            None,
            settings_card(theme, blocks),
        )
    }

    /// Renders the usage-recovery section (`usage-recovery.svelte`).
    fn render_usage_recovery(&self, theme: &artisan_ui::theme::ArtisanTheme) -> Div {
        let mut blocks = vec![settings_row(
            theme,
            "Automatically continue after usage resets",
            "New turns interrupted by a provider limit continue once Forge verifies the usage window has reset. You can still change this on each interruption card.",
            Some(
                self.settings_switch(
                    theme,
                    "settings-switch-recovery",
                    self.usage_recovery.auto_continue_usage_limits,
                    "settings-switch-recovery",
                )
                .into_any_element(),
            ),
        )];
        if !self.usage_recovery.message.is_empty() {
            blocks.push(
                div()
                    .w_full()
                    .py(px(12.0))
                    .text_sm()
                    .text_color(theme.colors.destructive.to_paint())
                    .child(self.usage_recovery.message.clone()),
            );
        }
        settings_section_shell(
            theme,
            "usage-recovery",
            "Usage recovery",
            Some("Usage-recovery choices are not yet configurable in the native app."),
            None,
            settings_card(theme, blocks),
        )
    }

    /// Renders the agent name-set section (`agent-names.svelte`).
    ///
    /// The dataset select paints the fixture value through the mountable
    /// `NativeSelect`. Note: the richer `Select` listbox used by the
    /// font-picker popovers has no `IntoElement` impl in `artisan-ui`, so it
    /// cannot mount here; those triggers stay disabled buttons (see
    /// [`SettingsScreen::font_trigger`]).
    fn render_agent_names(&self, theme: &artisan_ui::theme::ArtisanTheme) -> Div {
        let options = AGENT_NAME_DATASETS
            .into_iter()
            .filter_map(|(id, label, description)| {
                NativeSelectOption::new(id, format!("{label} \u{2014} {description}")).ok()
            })
            .collect::<Vec<_>>();
        let select = NativeSelect::new(
            "settings-select-agent-names",
            self.focus.control.clone(),
            *theme,
            self.agent_dataset.clone(),
            options,
        )
        .ok()
        .map(|select| {
            select
                .disabled(true)
                .semantic_label("Agent name set")
                .debug_selector("settings-select-agent-names")
        });
        let body = settings_card(
            theme,
            vec![settings_row(
                theme,
                "Name set",
                "The catalog Artisan uses when it names a new delegated agent.",
                select.map(IntoElement::into_any_element),
            )],
        );
        settings_section_shell(
            theme,
            "agents",
            "Agents",
            Some("The name set is not yet configurable in the native app."),
            None,
            body,
        )
    }

    /// Renders the scrollable content column for the mounted section.
    fn render_main(&self, theme: &artisan_ui::theme::ArtisanTheme, cx: &mut Context<Self>) -> Div {
        let section = match self.section {
            SettingsRoute::Models => self.render_models(theme),
            SettingsRoute::Appearance => self.render_appearance(theme, cx),
            SettingsRoute::Engines => self.render_engine(theme, cx),
            SettingsRoute::Notifications => self.render_notifications(theme),
            SettingsRoute::Privacy => self.render_privacy(theme),
            SettingsRoute::Threads => self.render_threads(theme),
            SettingsRoute::About => Self::render_about(theme),
        };
        div().flex().flex_col().flex_1().min_w_0().child(section)
    }
}

impl Render for SettingsScreen {
    /// Renders the settings frame: centered row of nav rail plus section.
    ///
    /// Legacy (`+layout.svelte`): `max-w-4xl` row with `md:gap-14`, sticky
    /// `md:w-44` aside, growing `main`. The native window is always wide, so
    /// only the desktop arrangement is painted; the mobile top-bar variant
    /// and the hash-scroll effect are orchestrator gaps.
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = &self.theme;
        let selector = settings_screen_selector(self.section);
        let nav = self.render_nav(theme, cx);
        let main = self.render_main(theme, cx);
        div()
            .id("settings-screen")
            .track_focus(&self.focus.root)
            .debug_selector(move || selector.clone())
            .size_full()
            .overflow_y_scroll()
            .bg(theme.colors.background.to_paint())
            .child(
                div().w_full().flex().flex_row().justify_center().child(
                    div()
                        .w_full()
                        .flex()
                        .flex_row()
                        .max_w(px(SETTINGS_CONTENT_MAX_WIDTH_PX))
                        .px(px(24.0))
                        .py(px(48.0))
                        .gap(px(SETTINGS_NAV_CONTENT_GAP_PX))
                        .child(nav)
                        .child(main),
                ),
            )
    }
}
