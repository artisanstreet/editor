use serde::Deserialize;
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

use aqua_core::geometry::CornerRadii;
use aqua_core::{DisplayItem, GpuiBackground, InputEvent, Insets, PointerButton};
use aqua_text::{FontFamily, FontWeight, StyledSpan, TextStyle};
use aqua_ui::element::{Element, ui};
use aqua_ui::input::{EventPhase, InputKind, InputResponse, RoutedInput};
use aqua_ui::style::{Length, Style};
use editor_assets::AssetId;

use crate::ui::theme::{ArtisanTheme, Oklch, Opacity, RadiusStep, RadiusTokens};

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
    fn presentation(self, theme: &ArtisanTheme) -> (&'static str, AssetId, Oklch) {
        match self {
            Self::Note => (
                "Note",
                AssetId::TABLER_MESSAGE_CIRCLE,
                theme.colors.banner_info.text,
            ),
            Self::Tip => (
                "Tip",
                AssetId::TABLER_SPARKLES,
                theme.colors.banner_success.text,
            ),
            Self::Warning => (
                "Warning",
                AssetId::TABLER_ALERT_TRIANGLE,
                theme.colors.banner_warning.text,
            ),
            Self::Risk => (
                "Risk",
                AssetId::TABLER_ALERT_TRIANGLE,
                theme.colors.banner_error.text,
            ),
            Self::Decision => ("Decision", AssetId::TABLER_TARGET_ARROW, theme.colors.link),
            Self::Assumption => (
                "Assumption",
                AssetId::TABLER_QUESTION_MARK,
                theme.colors.text_muted,
            ),
        }
    }
}

pub(crate) fn text(
    theme: &ArtisanTheme,
    value: &str,
    size: f32,
    line: f32,
    weight: FontWeight,
    color: Oklch,
) -> Element {
    text_with_wrap(theme, value, size, line, weight, color, true)
}

pub(crate) fn text_with_wrap(
    theme: &ArtisanTheme,
    value: &str,
    size: f32,
    line: f32,
    weight: FontWeight,
    color: Oklch,
    wrap: bool,
) -> Element {
    let style = TextStyle {
        families: vec![FontFamily::Named(theme.typography.sans.family.to_owned())],
        size_px: size,
        line_height_px: line,
        weight,
        ..TextStyle::default()
    };
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
        let (label, icon, color) = self.kind.presentation(theme);
        let mut header = ui::Box::new()
            .layout("row")
            .gap(8.0)
            .style(Style::new().align(aqua_ui::AlignItems::CENTER))
            .child(glyph(icon, 16.0, color.to_reference_paint()))
            .child(text(theme, label, 14.0, 20.0, FontWeight(600), color));
        if let Some(title) = &self.title {
            header = header.child(text(
                theme,
                title,
                14.0,
                20.0,
                FontWeight(600),
                theme.colors.text,
            ));
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
                    14.0,
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
        let edge = color.to_reference_paint();
        let background = color.at(Opacity::CalloutTint).to_reference_paint();
        let radius = RadiusTokens::value(RadiusStep::Lg);
        let mut container = ui::Canvas::new()
            .layout("column")
            .gap(6.0)
            .style(
                Style::new()
                    .width(Length::Pct(100.0))
                    .padding_insets(Insets {
                        left: 14.0,
                        right: 12.0,
                        top: 10.0,
                        bottom: 10.0,
                    }),
            )
            .scroll_stable()
            .on_paint(move |bounds| {
                vec![
                    DisplayItem::GpuiBackground {
                        bounds,
                        corners: CornerRadii::uniform(radius),
                        background: GpuiBackground::Solid(background),
                    },
                    DisplayItem::GpuiBorder {
                        bounds,
                        corners: CornerRadii::uniform(radius),
                        widths: Insets {
                            left: 2.0,
                            right: 0.0,
                            top: 0.0,
                            bottom: 0.0,
                        },
                        color: edge,
                        dashed: false,
                    },
                ]
            })
            .child(header);
        if open {
            let mut paragraphs = ui::Box::new().layout("column").gap(8.0);
            for paragraph in self
                .body
                .split("\n\n")
                .map(str::trim)
                .filter(|paragraph| !paragraph.is_empty())
            {
                paragraphs = paragraphs.child(text(
                    theme,
                    paragraph,
                    14.0,
                    22.0,
                    FontWeight::NORMAL,
                    theme.colors.text,
                ));
            }
            container = container.child(paragraphs.build());
        }
        container.build().with_key(id)
    }
}
