//! The model selector's trigger and the card's project tab.

use super::*;

impl Composer {
    // Rendering.

    /// The model selector's trigger: engine glyph, model name, its muted
    /// details (context, effort) and the selector mark.
    #[expect(
        clippy::too_many_lines,
        reason = "one element tree for the trigger, ported from the old Editor"
    )]
    pub(crate) fn render_model_trigger(
        &self,
        theme: &ArtisanTheme,
        cx: &Context<Self>,
    ) -> impl IntoElement {
        let theme = *theme;
        let config = self.effective_config(cx);
        let host = self.host(cx);
        let model: Option<Model> = config.as_ref().and_then(|config| {
            let id = config.model.as_ref()?;
            host.as_ref()?
                .read(cx)
                .engines
                .get(&config.engine)?
                .models
                .iter()
                .find(|model| &model.id == id)
                .cloned()
        });
        let muted = theme.colors.text_muted.to_paint();
        let token = || {
            div()
                .truncate()
                .text_size(TypographyTokens::size(TextStep::Sm))
                .line_height(px(20.0))
        };
        let mut body = div()
            .flex()
            .items_center()
            .gap(px(4.0))
            .min_w(px(0.0))
            .overflow_hidden();
        match &config {
            Some(config) => {
                let name = model.as_ref().map_or_else(
                    || {
                        config
                            .model
                            .clone()
                            .unwrap_or_else(|| "No model".to_owned())
                    },
                    |model| model.name.clone(),
                );
                body = body.child(token().child(name));
                if let Some(context) = model
                    .as_ref()
                    .and_then(|model| config.context_window_override().or(model.context_window))
                {
                    body = body.child(token().text_color(muted).child(context_label(context)));
                }
                if config.engine != EngineId::OpenCode
                    && let Some(effort) = &config.effort
                {
                    body = body.child(token().text_color(muted).child(humanize(effort)));
                }
                body = body.children(variant_and_speed_tokens(
                    config,
                    model.as_ref(),
                    muted,
                    &theme.colors,
                ));
            }
            None => body = body.child(token().child("No model")),
        }
        let trigger = div()
            .id("composer-model")
            .role(gpui::Role::Button)
            .aria_label("Choose a model")
            .aria_expanded(self.menu == Some(Menu::Model))
            .focusable()
            .tab_index(0)
            .flex()
            .items_center()
            .gap(px(8.0))
            .h(px(COMPACT_CONTROL_HEIGHT_PX))
            .max_w(px(360.0))
            .px(px(8.0))
            .rounded(RadiusTokens::value(RadiusStep::Lg))
            .cursor_pointer()
            .hover(move |style| hover_fill(style, &theme, HoverFill::Plain))
            .text_color(theme.colors.text.to_paint())
            .when_some(config.as_ref(), |trigger, config| {
                trigger.child(
                    asset_glyph(engine_icon(&theme, config.engine))
                        .size(px(16.0))
                        .flex_shrink_0(),
                )
            })
            .child(body)
            .child(
                asset_glyph(AssetId::TABLER_SELECTOR)
                    .size(px(14.0))
                    .flex_shrink_0()
                    .text_color(muted),
            )
            .on_click(cx.listener(|this, _, _, cx| {
                if this.menu == Some(Menu::Model) {
                    this.close_menu(cx);
                } else {
                    this.close_menu(cx);
                    this.open_menu(Menu::Model, cx);
                }
            }));
        let mut root = div()
            .relative()
            .flex()
            .flex_col()
            .min_w(px(0.0))
            .child(trigger);
        if self.picker.shown == Some(Menu::Model) {
            root = root.child(above_trigger(
                px(MENU_GAP_PX),
                1,
                self.model_menu(&theme, cx),
            ));
        }
        root
    }

    /// The card's tab: a new thread's project picker (a ghost select), or
    /// the project the thread belongs to as a quiet label.
    pub(crate) fn render_tab(
        &self,
        theme: &ArtisanTheme,
        desktop: &ColorTokens,
        cx: &Context<Self>,
    ) -> Option<AnyElement> {
        let theme = *theme;
        let host = self.host(cx)?;
        let host = host.read(cx);
        let project_name =
            |id: &artisan_api::ProjectId| host.projects.get(id).map(|project| project.name.clone());
        let face = |label: SharedString| {
            div()
                .h(px(28.0))
                .px(px(8.0))
                .flex()
                .items_center()
                .gap(px(6.0))
                .text_size(TypographyTokens::size(TextStep::XsMid))
                .text_color(desktop.text_muted.to_paint())
                .child(asset_glyph(AssetId::TABLER_FOLDER).size(px(15.0)))
                .child(div().truncate().max_w(px(240.0)).child(label))
        };
        match &self.target {
            crate::ComposerTarget::Thread { thread, .. } => {
                let thread = host.threads.get(thread)?;
                let name = project_name(&thread.project_id)?;
                Some(face(name.into()).into_any_element())
            }
            // A new thread's project is always a choice; the one it was
            // opened for is only where it starts (master's
            // `home_project_picker`).
            crate::ComposerTarget::NewThread { .. } => {
                let open = self.menu == Some(Menu::Project);
                let label: SharedString = self
                    .project(cx)
                    .as_ref()
                    .and_then(project_name)
                    .map_or_else(|| "Choose a project".into(), Into::into);
                let foreground = desktop.text.to_paint();
                let trigger = div()
                    .id("composer-project")
                    .role(gpui::Role::Button)
                    .aria_label(label.clone())
                    .aria_expanded(open)
                    .focusable()
                    .tab_index(0)
                    .debug_selector(|| "composer-project".to_owned())
                    .h(px(28.0))
                    .px(px(8.0))
                    .flex()
                    .items_center()
                    .gap(px(6.0))
                    .rounded(RadiusTokens::value(RadiusStep::Lg))
                    .text_size(TypographyTokens::size(TextStep::XsMid))
                    .text_color(if open {
                        desktop.text.to_paint()
                    } else {
                        desktop.text_muted.to_paint()
                    })
                    .cursor_pointer()
                    .hover(move |style| style.text_color(foreground))
                    .on_click(cx.listener(|this, _, _, cx| {
                        if this.menu == Some(Menu::Project) {
                            this.close_menu(cx);
                        } else {
                            this.close_menu(cx);
                            this.open_menu(Menu::Project, cx);
                        }
                    }))
                    .child(asset_glyph(AssetId::TABLER_FOLDER).size(px(15.0)))
                    .child(div().truncate().max_w(px(240.0)).child(label))
                    .child(asset_glyph(AssetId::TABLER_SELECTOR).size(px(14.0)));
                let mut root = div().relative().child(trigger);
                if self.picker.shown == Some(Menu::Project) {
                    root = root.child(above_trigger(
                        px(PROJECT_MENU_GAP_PX),
                        1,
                        self.project_menu(&theme, desktop, cx),
                    ));
                }
                Some(root.into_any_element())
            }
        }
    }
}

/// The trigger's `<variant> <speed>` tokens: the chosen variant, muted, and
/// a speed tier other than the model's default, in its gradient.
fn variant_and_speed_tokens(
    config: &EngineConfig,
    model: Option<&Model>,
    muted: gpui::Hsla,
    colors: &ColorTokens,
) -> Vec<AnyElement> {
    let token = || {
        div()
            .truncate()
            .text_size(TypographyTokens::size(TextStep::Sm))
            .line_height(px(20.0))
    };
    let mut tokens = Vec::new();
    let Some(model) = model else {
        return tokens;
    };
    if let Some(variant) = model
        .variant_for(config.variant.as_deref())
        .and_then(|id| model.variants.iter().find(|variant| variant.id == id))
    {
        tokens.push(
            token()
                .text_color(muted)
                .child(variant.label.clone())
                .into_any_element(),
        );
    }
    if let Some(speed) = config.speed.as_ref().and_then(|id| {
        model
            .speeds
            .iter()
            .find(|speed| &speed.id == id && !speed.default)
    }) {
        let label = speed.label.clone();
        tokens.push(
            token()
                .debug_selector(|| "composer-model-speed".to_owned())
                .child(match speed_gradient(speed, colors) {
                    Some(gradient) => gradient_text(label, gradient),
                    None => StyledText::new(label),
                })
                .into_any_element(),
        );
    }
    tokens
}

/// The glyph gradient of an accelerated tier (`speed_label_gradient`):
/// amber into yellow for Fast, green into pink for Superfast.
fn speed_gradient(speed: &Speed, colors: &ColorTokens) -> Option<(Oklch, Oklch)> {
    if speed.id == "superfast" {
        Some((colors.palette[7], colors.palette[3]))
    } else if speed.id == "fast" || speed.label.eq_ignore_ascii_case("fast") {
        Some((colors.palette[1], colors.palette[4]))
    } else {
        None
    }
}

/// `text` with its glyphs walking from one token to the other, left to
/// right (`gradient_label_text`): a two-token gradient laid across glyphs,
/// which a background gradient can't colour.
#[expect(
    clippy::cast_precision_loss,
    reason = "a label has far fewer glyphs than f32 counts exactly"
)]
fn gradient_text(text: String, (start, end): (Oklch, Oklch)) -> StyledText {
    let count = text.chars().count();
    let mut highlights = Vec::with_capacity(count);
    let mut at = 0;
    for (index, character) in text.chars().enumerate() {
        let progress = if count > 1 {
            index as f32 / (count - 1) as f32
        } else {
            0.0
        };
        let next = at + character.len_utf8();
        highlights.push((
            at..next,
            HighlightStyle {
                color: Some(start.animate_to(end, progress).to_paint()),
                ..HighlightStyle::default()
            },
        ));
        at = next;
    }
    StyledText::new(text).with_highlights(highlights)
}
