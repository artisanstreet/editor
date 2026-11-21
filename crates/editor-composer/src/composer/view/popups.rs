use super::{
    ComposerAction, ComposerOwner, PickerTooltipPresentation, PickerTooltipSurface, PopupKind,
};
use aqua_ui::{
    element::Element,
    style::{Length, Style},
};
use editor_ui::{
    kit::{Surface, TextRole, Tooltip},
    theme::ArtisanTheme,
};
use std::rc::Rc;

fn picker_tooltip_surface(
    theme: &ArtisanTheme,
    tooltip: &super::super::picker::TooltipContent,
    width: f32,
    fixed: bool,
) -> Element {
    use aqua_text::StyledSpan;
    let style = TextRole::Caption.style(theme);
    let mut spans = Vec::new();
    if let Some(advisory) = tooltip.advisory.as_ref().filter(|text| !text.is_empty()) {
        let color = if advisory == "Additional info:" {
            theme.colors.banner_info.text
        } else {
            theme.colors.destructive_text
        }
        .to_reference_text_color();
        spans.push(
            StyledSpan::new(advisory.clone(), style.clone()).gpui_source_color([
                color.hue_degrees,
                color.saturation,
                color.lightness,
                color.alpha,
            ]),
        );
        if !tooltip.text.is_empty() {
            spans.push(StyledSpan::new(" ", style.clone()));
        }
    }
    if !tooltip.text.is_empty() {
        spans.push(StyledSpan::new(tooltip.text.clone(), style));
    }
    let mut layout = Style::new().max_width(Length::Px(width));
    if fixed {
        layout = layout.width(Length::Px(width));
    }
    Surface::Tooltip(Tooltip::Brief)
        .build(
            theme,
            vec![
                TextRole::Caption
                    .text(theme, "")
                    .gpui_source_color(theme.colors.text_muted_glass.to_reference_text_color())
                    .spans(spans)
                    .wrap(true)
                    .build(),
            ],
        )
        .style(layout)
        .with_key("composer-picker-tooltip")
}

impl ComposerOwner {
    pub fn tooltip_deadline(&self) -> Option<std::time::Instant> {
        self.tooltip.borrow().deadline()
    }

    pub fn advance_tooltip(&self, now: std::time::Instant) -> bool {
        self.tooltip.borrow_mut().show_due(now)
    }

    pub fn shown_tooltip(&self) -> Option<super::super::picker::TooltipContent> {
        self.tooltip.borrow().shown().cloned()
    }

    pub fn tooltip_surface_factory(&self, theme: &ArtisanTheme) -> Option<PickerTooltipSurface> {
        use super::super::picker::TooltipPlacement;
        let tooltip = self.shown_tooltip()?;
        let placement = match tooltip.placement {
            TooltipPlacement::Above => {
                editor_ui::hover_tooltip::TooltipPlacement::Above { max_width: 240.0 }
            }
            TooltipPlacement::Beside => {
                editor_ui::hover_tooltip::TooltipPlacement::Beside { max_width: 320.0 }
            }
        };
        let theme = *theme;
        Some(PickerTooltipSurface {
            placement,
            surface: Rc::new(move |width, fixed| {
                picker_tooltip_surface(&theme, &tooltip, width, fixed)
            }),
        })
    }

    pub fn tooltip_presentation(
        &self,
        anchor: aqua_core::Rect,
        viewport: aqua_core::Size,
    ) -> Option<PickerTooltipPresentation> {
        use super::super::picker::TooltipPlacement;
        use aqua_ui::overlay::Side;
        let tooltip = self.shown_tooltip()?;
        let (side, snap_to_window) = match tooltip.placement {
            TooltipPlacement::Above => (Side::Above, true),
            TooltipPlacement::Beside => {
                let right = viewport.w.max(1.0) - anchor.right() - 8.0;
                let left = anchor.x - 8.0;
                let use_right = right >= 320.0 || right >= left;
                (if use_right { Side::After } else { Side::Before }, false)
            }
        };
        Some(PickerTooltipPresentation {
            side,
            snap_to_window,
        })
    }

    pub fn dispose_popups(&mut self, app: &mut aqua::UiApp) {
        self.tooltip.borrow_mut().clear();
        if let Some(popup) = self.mention_popup.take() {
            popup.dispose(app);
        }
        if let Some(popup) = self.model_popup.take() {
            popup.dispose(app);
        }
        if let Some(popup) = self.project_popup.take() {
            popup.dispose(app);
        }
    }

    pub fn open_popup_content(
        &mut self,
        app: &mut aqua::UiApp,
        anchor: aqua_core::NodeId,
        kind: PopupKind,
        initial: Element,
        now: f64,
    ) -> Result<(), String> {
        use aqua_ui::popup::PopupFocus;
        use editor_ui::popover::{AnimatedPickerHost, PickerEntrance};
        if self.popup_is_open(kind, app) {
            return self.refresh_popup_content(kind, app, initial, now);
        }
        let entrance = PickerEntrance::Scale {
            origin: aqua_core::Point::new(0.0, 1.0),
        };
        let open_duration = editor_ui::picker_motion::DROPDOWN_OPEN;
        let close_duration = editor_ui::picker_motion::DROPDOWN_CLOSE;
        let action = Rc::clone(&self.dispatch);
        let popup = AnimatedPickerHost::open_content(
            app,
            anchor,
            initial,
            kind.gap(),
            PopupFocus::Preserve,
            entrance,
            if self.reduced_motion {
                std::time::Duration::ZERO
            } else {
                open_duration
            },
            if self.reduced_motion {
                std::time::Duration::ZERO
            } else {
                close_duration
            },
            now,
            move |_| {
                action(match kind {
                    PopupKind::Mentions => ComposerAction::DismissMentions,
                    PopupKind::Model | PopupKind::Project => ComposerAction::ClosePicker,
                });
            },
        )?;
        let slot = match kind {
            PopupKind::Model => &mut self.model_popup,
            PopupKind::Mentions => &mut self.mention_popup,
            PopupKind::Project => &mut self.project_popup,
        };
        if let Some(old) = slot.replace(popup) {
            old.dispose(app);
        }
        Ok(())
    }

    pub fn popup_root(&self, kind: PopupKind, app: &aqua::UiApp) -> Option<aqua_core::NodeId> {
        match kind {
            PopupKind::Model => self.model_popup.as_ref().and_then(|popup| popup.root(app)),
            PopupKind::Mentions => self
                .mention_popup
                .as_ref()
                .and_then(|popup| popup.root(app)),
            PopupKind::Project => self
                .project_popup
                .as_ref()
                .and_then(|popup| popup.root(app)),
        }
    }

    pub fn popup_is_open(&self, kind: PopupKind, app: &aqua::UiApp) -> bool {
        self.popup_root(kind, app).is_some()
    }

    pub fn refresh_popup_content(
        &mut self,
        kind: PopupKind,
        app: &mut aqua::UiApp,
        content: Element,
        now: f64,
    ) -> Result<(), String> {
        match kind {
            PopupKind::Model => self
                .model_popup
                .as_mut()
                .filter(|popup| popup.is_open(app))
                .map_or(Ok(()), |popup| {
                    if popup.is_closing() {
                        popup.begin_open(app, now)?;
                    }
                    popup.refresh_content(app, content)
                }),
            PopupKind::Mentions => self
                .mention_popup
                .as_mut()
                .filter(|popup| popup.is_open(app))
                .map_or(Ok(()), |popup| {
                    if popup.is_closing() {
                        popup.begin_open(app, now)?;
                    }
                    popup.refresh_content(app, content)
                }),
            PopupKind::Project => self
                .project_popup
                .as_mut()
                .filter(|popup| popup.is_open(app))
                .map_or(Ok(()), |popup| {
                    if popup.is_closing() {
                        popup.begin_open(app, now)?;
                    }
                    popup.refresh_content(app, content)
                }),
        }
    }

    pub fn close_popup(
        &mut self,
        app: &mut aqua::UiApp,
        kind: PopupKind,
        now: f64,
    ) -> Result<(), String> {
        match kind {
            PopupKind::Model => {
                if let Some(popup) = &mut self.model_popup {
                    popup.begin_close(app, now)?;
                }
            }
            PopupKind::Mentions => {
                if let Some(popup) = &mut self.mention_popup {
                    popup.begin_close(app, now)?;
                }
            }
            PopupKind::Project => {
                if let Some(popup) = &mut self.project_popup {
                    popup.begin_close(app, now)?;
                }
            }
        }
        Ok(())
    }

    pub fn advance_popups(&mut self, app: &mut aqua::UiApp, now: f64) -> Result<(), String> {
        for popup in [
            &mut self.model_popup,
            &mut self.mention_popup,
            &mut self.project_popup,
        ] {
            if let Some(popup) = popup
                && popup.is_open(app)
            {
                popup.advance(app, now)?;
            }
        }
        Ok(())
    }

    pub fn dismiss_popups_for_event(
        &mut self,
        app: &mut aqua::UiApp,
        event: &aqua_core::InputEvent,
    ) -> Result<bool, String> {
        for (popup, close) in [
            (&mut self.model_popup, ComposerAction::ClosePicker),
            (&mut self.mention_popup, ComposerAction::DismissMentions),
            (&mut self.project_popup, ComposerAction::ClosePicker),
        ] {
            if let Some(popup) = popup
                && popup.is_open(app)
                && !popup.is_closing()
                && popup.request_dismissal(app, event)?
            {
                (self.dispatch)(close);
                return Ok(true);
            }
        }
        Ok(false)
    }
}
