//! The picker's delayed glass tooltips: a policy option's description
//! beside its row, an unavailable engine's reason above its tab. Delay,
//! placement and the glass frame are the shared `editor_ui::hover_card`
//! tooltip.

use super::*;
use editor_ui::hover_card::{TooltipPlacement, glass_tooltip, place_tooltip};

/// A policy option's tooltip is at most this wide.
const OPTION_TOOLTIP_WIDTH_PX: f32 = 320.0;
/// An engine tab's tooltip is at most this wide.
const ENGINE_TOOLTIP_WIDTH_PX: f32 = 240.0;

/// What a picker tooltip says, and where.
#[derive(Clone, Debug)]
pub(super) struct Tooltip {
    /// Painted ahead of `text` in the warning colour.
    advisory: Option<String>,
    text: String,
    placement: TooltipPlacement,
}

/// Beside a policy option, on whichever side has room.
pub(super) const SIDE: TooltipPlacement = TooltipPlacement::Beside {
    max_width: OPTION_TOOLTIP_WIDTH_PX,
};
/// Centred above an engine tab.
pub(super) const ABOVE: TooltipPlacement = TooltipPlacement::Above {
    max_width: ENGINE_TOOLTIP_WIDTH_PX,
};

impl Composer {
    /// Mounts a tooltip hidden and shows it once the pointer rests.
    pub(super) fn begin_tooltip(
        &mut self,
        key: String,
        advisory: Option<String>,
        text: String,
        placement: TooltipPlacement,
        cx: &mut Context<Self>,
    ) {
        let tooltip = Tooltip {
            advisory,
            text,
            placement,
        };
        self.picker
            .tooltip
            .begin(key, tooltip, cx, |this: &mut Self| &mut this.picker.tooltip);
    }

    pub(super) fn clear_tooltip(&mut self) {
        self.picker.tooltip.clear();
    }

    pub(super) fn clear_tooltip_for(&mut self, key: &str) {
        self.picker.tooltip.clear_for(key);
    }

    /// The shown tooltip (`render_option_tooltip`), placed against the
    /// element it belongs to.
    pub(crate) fn render_tooltip(
        &self,
        theme: &ArtisanTheme,
        viewport: gpui::Size<gpui::Pixels>,
    ) -> Option<AnyElement> {
        if self.menu != Some(Menu::Model) {
            return None;
        }
        let tooltip = self.picker.tooltip.shown()?.clone();
        let anchor = self.picker.tooltip_anchor.get()?;
        let theme = *theme;
        Some(place_tooltip(
            anchor,
            viewport,
            tooltip.placement,
            move |width, fixed| {
                tooltip_surface(&theme, width, fixed, tooltip.advisory, &tooltip.text)
            },
        ))
    }
}

/// A tooltip's glass surface (`render_option_tooltip_surface`): the
/// advisory first, in the warning colour, then the description.
fn tooltip_surface(
    theme: &ArtisanTheme,
    width: gpui::Pixels,
    fixed: bool,
    advisory: Option<String>,
    text: &str,
) -> AnyElement {
    let advisory = advisory.filter(|advisory| !advisory.is_empty());
    let advisory_color = if advisory.as_deref() == Some("Additional info:") {
        theme.colors.banner_info
    } else {
        theme.colors.destructive
    };
    let mut body = String::new();
    let mut advisory_end = 0;
    if let Some(advisory) = advisory {
        body.push_str(&advisory);
        advisory_end = body.len();
        if !text.is_empty() {
            body.push(' ');
        }
    }
    body.push_str(text);
    let body =
        StyledText::new(SharedString::from(body)).with_highlights((advisory_end > 0).then(|| {
            (
                0..advisory_end,
                HighlightStyle {
                    color: Some(advisory_color.to_paint()),
                    font_weight: Some(FontWeight::MEDIUM),
                    ..HighlightStyle::default()
                },
            )
        }));
    glass_tooltip("composer-picker-tooltip", theme, width, fixed)
        .debug_selector(|| "composer-picker-tooltip".to_owned())
        .child(body)
        .into_any_element()
}
