use serde::Deserialize;
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use aqua_core::geometry::CornerRadii;
use aqua_core::{DisplayItem, Insets};
use aqua_text::StyledSpan;
use aqua_ui::element::{Element, ui};
use aqua_ui::style::{Length, Style};
use editor_assets::AssetId;

use crate::ui::kit::control::focus_ring;
use crate::ui::kit::{IconSize, Space, Spaced, Surface, TextRole, Tone};
use crate::ui::theme::{ArtisanTheme, Oklch};

const STRIPE: f32 = 2.0;

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "lowercase")]
enum Kind {
    #[default]
    Note,
    Tip,
    Warning,
    Risk,
    Decision,
    Assumption,
}

#[derive(Debug, Deserialize)]
pub(crate) struct Callout {
    #[serde(default)]
    kind: Kind,
    #[serde(default)]
    title: Option<String>,
    body: String,
    #[serde(default)]
    collapsed: bool,
}
impl Callout {
    pub(crate) fn resident_bytes(&self) -> usize {
        size_of::<Self>()
            + crate::ui::rich::kit::retained_optional(self.title.as_ref())
            + self.body.capacity()
    }
}

#[derive(Clone, Default)]
pub(crate) struct CalloutState(Rc<RefCell<HashMap<String, bool>>>);

impl CalloutState {
    pub(crate) fn remove(&self, id: &str) {
        self.0.borrow_mut().remove(id);
    }
}

#[derive(Clone, Copy)]
pub(crate) struct CalloutNotes<'a> {
    pub(crate) subtitle: Option<&'a str>,
    pub(crate) caption: Option<&'a str>,
}

pub type GlyphFactory = Rc<dyn Fn(AssetId, f32, aqua_core::GpuiHsla) -> Element>;

impl Kind {
    fn presentation(self, theme: &ArtisanTheme) -> (&'static str, AssetId, Oklch, Tone) {
        match self {
            Self::Note => (
                "Note",
                AssetId::TABLER_MESSAGE_CIRCLE,
                theme.colors.banner_info.text,
                Tone::Info,
            ),
            Self::Tip => (
                "Tip",
                AssetId::TABLER_SPARKLES,
                theme.colors.banner_success.text,
                Tone::Success,
            ),
            Self::Warning => (
                "Warning",
                AssetId::TABLER_ALERT_TRIANGLE,
                theme.colors.banner_warning.text,
                Tone::Warning,
            ),
            Self::Risk => (
                "Risk",
                AssetId::TABLER_ALERT_TRIANGLE,
                theme.colors.banner_error.text,
                Tone::Danger,
            ),
            Self::Decision => (
                "Decision",
                AssetId::TABLER_TARGET_ARROW,
                theme.colors.link,
                Tone::Info,
            ),
            Self::Assumption => (
                "Assumption",
                AssetId::TABLER_QUESTION_MARK,
                theme.colors.text_muted,
                Tone::Neutral,
            ),
        }
    }
}

pub(crate) fn text(theme: &ArtisanTheme, value: &str, role: TextRole, color: Oklch) -> Element {
    text_with_wrap(theme, value, role, color, true)
}

pub(crate) fn text_with_wrap(
    theme: &ArtisanTheme,
    value: &str,
    role: TextRole,
    color: Oklch,
    wrap: bool,
) -> Element {
    let style = role.style(theme);
    let paint = color.to_reference_text_color();
    ui::Text::new(value.to_owned())
        .text_style(style.clone())
        .gpui_source_color(paint)
        .spans(vec![StyledSpan::new(value, style).gpui_source_color([
            paint.hue_degrees,
            paint.saturation,
            paint.lightness,
            paint.alpha,
        ])])
        .wrap(wrap)
        .build()
}

impl Callout {
    #[expect(
        clippy::too_many_lines,
        reason = "one callout view: tone, glyph, title and body"
    )]
    pub(crate) fn view(
        &self,
        id: String,
        theme: &ArtisanTheme,
        notes: CalloutNotes<'_>,
        states: CalloutState,
        glyph: &GlyphFactory,
        invalidate: Rc<dyn Fn()>,
    ) -> Element {
        let open = *states
            .0
            .borrow_mut()
            .entry(id.clone())
            .or_insert(!self.collapsed);
        let (label, icon, color, tone) = self.kind.presentation(theme);
        let mut header = ui::Box::new()
            .layout("row")
            .spaced(Space::S8)
            .style(Style::new().align(aqua_ui::AlignItems::CENTER))
            .child(glyph(icon, IconSize::Md.px(), color.to_reference_paint()))
            .child(text(theme, label, TextRole::Title, color));
        if let Some(title) = &self.title {
            header = header.child(text(theme, title, TextRole::Title, theme.colors.text));
        }
        let header = if self.collapsed {
            let name = match &self.title {
                Some(title) => format!("{label}: {title}"),
                None => label.to_owned(),
            };
            let state = states;
            let key = id.clone();
            ui::Button::new()
                .layout("row")
                .style(
                    Style::new()
                        .width(Length::Pct(100.0))
                        .align(aqua_ui::AlignItems::CENTER),
                )
                .on_activate(move || {
                    if let Some(open) = state.0.borrow_mut().get_mut(&key) {
                        *open = !*open;
                    }
                    invalidate();
                })
                .child(header.style(Style::new().flex_grow(1.0)).build())
                .child(glyph(
                    if open {
                        AssetId::TABLER_CHEVRON_UP
                    } else {
                        AssetId::TABLER_CHEVRON_DOWN
                    },
                    IconSize::Md.px(),
                    theme.colors.text_muted.to_reference_paint(),
                ))
                .build()
                .role(aqua_core::Role::Button)
                .aria_label(name)
                .aria_expanded(open)
                .cursor(aqua_core::CursorIcon::Pointer)
                .focus_visible_style(focus_ring(theme))
                .with_key(format!("{id}-header"))
        } else {
            header.build()
        };
        let surface = Surface::Callout(tone);
        let radius = surface.radius_px();
        let edge = color.to_reference_paint();
        let stripe = ui::Canvas::new()
            .style(Style::new().absolute_at(Some(0.0), Some(0.0), Some(0.0), Some(0.0)))
            .scroll_stable()
            .on_paint(move |bounds| {
                vec![DisplayItem::GpuiBorder {
                    bounds,
                    corners: CornerRadii::uniform(radius),
                    widths: Insets {
                        left: STRIPE,
                        right: 0.0,
                        top: 0.0,
                        bottom: 0.0,
                    },
                    color: edge,
                    dashed: false,
                }]
            })
            .build()
            .exclude_from_hit_testing();
        let mut content = ui::Box::new()
            .layout("column")
            .spaced(Space::S8)
            .child(header);
        if let Some(subtitle) = notes.subtitle {
            content = content.child(text(
                theme,
                subtitle,
                TextRole::Label,
                theme.colors.text_muted,
            ));
        }
        if open {
            let mut paragraphs = ui::Box::new().layout("column").spaced(Space::S8);
            for paragraph in self
                .body
                .split("\n\n")
                .map(str::trim)
                .filter(|paragraph| !paragraph.is_empty())
            {
                paragraphs =
                    paragraphs.child(text(theme, paragraph, TextRole::Label, theme.colors.text));
            }
            content = content.child(paragraphs.build());
            if let Some(caption) = notes.caption {
                content = content.child(text(
                    theme,
                    caption,
                    TextRole::Caption,
                    theme.colors.text_muted,
                ));
            }
        }
        surface
            .build(theme, vec![stripe, content.build()])
            .style(Style::new().width(Length::Pct(100.0)))
            .with_key(id)
    }
}

#[cfg(test)]
#[path = "../../../../tests/ui/rich/narrative/callout.rs"]
mod tests;
