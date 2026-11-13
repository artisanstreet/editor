use serde::Deserialize;
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

use aqua_core::geometry::CornerRadii;
use aqua_core::{DisplayItem, InputEvent, Insets, PointerButton};
use aqua_text::StyledSpan;
use aqua_ui::element::{Element, ui};
use aqua_ui::input::{EventPhase, InputKind, InputResponse, RoutedInput};
use aqua_ui::style::{Length, Style};
use editor_assets::AssetId;

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
        std::mem::size_of::<Self>()
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

pub(crate) type GlyphFactory = Rc<dyn Fn(AssetId, f32, aqua_core::GpuiHsla) -> Element>;

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
    pub(crate) fn view(
        &self,
        id: String,
        theme: &ArtisanTheme,
        states: CalloutState,
        glyph: GlyphFactory,
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
        if self.collapsed {
            header = header
                .child(ui::Box::new().style(Style::new().flex_grow(1.0)).build())
                .child(glyph(
                    if open {
                        AssetId::TABLER_CHEVRON_UP
                    } else {
                        AssetId::TABLER_CHEVRON_DOWN
                    },
                    IconSize::Md.px(),
                    theme.colors.text_muted.to_reference_paint(),
                ));
        }
        let mut header = header.build();
        if self.collapsed {
            let pressed = Rc::new(Cell::new(false));
            let down = pressed.clone();
            header = header.cursor(aqua_core::CursorIcon::Pointer).on_input(
                InputKind::PointerDown,
                EventPhase::Bubble,
                move |event| {
                    if matches!(
                        &event.input,
                        RoutedInput::Native(InputEvent::PointerDown(_, PointerButton::Primary))
                    ) {
                        down.set(true);
                        InputResponse::capture()
                    } else {
                        InputResponse::default()
                    }
                },
            );
            let state = states.clone();
            let key = id.clone();
            header = header.on_input(InputKind::PointerUp, EventPhase::Bubble, move |event| {
                let should_toggle = matches!(&event.input, RoutedInput::Native(InputEvent::PointerUp(pointer, PointerButton::Primary)) if event.bounds.contains(pointer.position));
                if pressed.replace(false) && should_toggle {
                    if let Some(open) = state.0.borrow_mut().get_mut(&key) { *open = !*open; }
                    invalidate();
                }
                InputResponse { pointer_capture: aqua_ui::input::CaptureRequest::Release, ..InputResponse::default() }
            });
        }
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
        }
        surface
            .build(theme, vec![stripe, content.build()])
            .style(Style::new().width(Length::Pct(100.0)))
            .with_key(id)
    }
}
