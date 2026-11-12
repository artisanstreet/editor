mod view;
pub(crate) use view::{ModelServices, ModelState};

use super::SizeHint;
use crate::ui::kit::TextRole;
use crate::ui::media::{
    color,
    model::{Format, Up, srgb_to_linear},
    source::MediaSource,
};
use glam::Vec3;
use serde::Deserialize;

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Shading {
    #[default]
    Shaded,
    Wireframe,
}

impl Shading {
    pub(crate) const ALL: [Self; 2] = [Self::Shaded, Self::Wireframe];

    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::Shaded => "Shaded",
            Self::Wireframe => "Wireframe",
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(try_from = "Spec")]
pub(crate) struct ModelBlock {
    src: MediaSource,
    name: Option<String>,
    format: Option<Format>,
    up: Option<Up>,
    color: Option<Vec3>,
    shading: Shading,
    yaw: Option<f32>,
    pitch: Option<f32>,
    hint: SizeHint,
}

#[derive(Debug, Deserialize)]
struct Spec {
    src: MediaSource,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    format: Option<Format>,
    #[serde(default)]
    up: Option<Up>,
    #[serde(default)]
    color: Option<String>,
    #[serde(default)]
    shading: Shading,
    #[serde(default)]
    yaw: Option<f32>,
    #[serde(default)]
    pitch: Option<f32>,
    #[serde(flatten)]
    hint: SizeHint,
}

impl TryFrom<Spec> for ModelBlock {
    type Error = String;

    fn try_from(spec: Spec) -> Result<Self, String> {
        let src = spec.src;
        if src.is_empty() {
            return Err("a model needs a `src`".to_owned());
        }
        let color = match spec.color.as_deref().map(str::trim) {
            None | Some("") => None,
            Some(text) => {
                let parsed = color::parse(text)
                    .ok_or_else(|| format!("`{text}` is not a CSS color"))?
                    .clipped();
                #[expect(clippy::cast_possible_truncation, reason = "channels are 0..=1")]
                let channel = |value: f64| srgb_to_linear(value.clamp(0.0, 1.0) as f32);
                Some(Vec3::new(
                    channel(parsed.r),
                    channel(parsed.g),
                    channel(parsed.b),
                ))
            }
        };
        let finite = |angle: Option<f32>| angle.filter(|angle| angle.is_finite());
        Ok(Self {
            src,
            name: spec
                .name
                .map(|name| name.trim().to_owned())
                .filter(|name| !name.is_empty()),
            format: spec.format,
            up: spec.up,
            color,
            shading: spec.shading,
            yaw: finite(spec.yaw),
            pitch: finite(spec.pitch),
            hint: spec.hint,
        })
    }
}

impl ModelBlock {
    pub(crate) fn resident_bytes(&self) -> usize {
        std::mem::size_of::<Self>()
            + self.src.resident_bytes()
            + crate::ui::rich::kit::retained_optional(self.name.as_ref())
    }
    fn display_name(&self) -> String {
        self.name.clone().unwrap_or_else(|| {
            self.src
                .rsplit(['/', '\\'])
                .next()
                .filter(|name| !name.is_empty() && !self.src.starts_with("data:"))
                .unwrap_or("Model")
                .to_owned()
        })
    }
}

pub(crate) fn services(
    id: &str,
    theme: crate::ui::theme::ArtisanTheme,
    assets: crate::ui::media::model::load::ModelAssets,
    capability: std::rc::Rc<dyn Fn() -> Option<crate::ui::media::model::load::RendererCapability>>,
    glyph: crate::ui::rich::narrative::callout::GlyphFactory,
    effect: std::rc::Rc<dyn Fn(crate::ui::media::preview::Effect)>,
    hovered: std::rc::Rc<std::cell::Cell<bool>>,
    invalidate: std::rc::Rc<dyn Fn()>,
) -> ModelServices {
    use crate::ui::kit::{Button, ButtonTone, IconSize, Media, Space, Spaced, Surface};
    use aqua_core::GpuiBackground;
    use aqua_ui::{
        element::{Element, ui},
        style::{Length, Style},
    };
    let frame_hover = hovered.clone();
    let frame_invalidate = invalidate.clone();
    let frame = std::rc::Rc::new(
        move |content: Element, width: f32, aspect: f32, _name: &str, dragging: Option<bool>| {
            let mut layers = vec![content];
            if dragging == Some(false) && frame_hover.get() {
                let shortcut = if cfg!(target_os = "macos") {
                    "⌘-scroll"
                } else {
                    "Ctrl-scroll"
                };
                let hint = crate::ui::rich::narrative::callout::text_with_wrap(
                    &theme,
                    &format!("Drag to turn · Shift-drag to pan · {shortcut} to zoom"),
                    TextRole::Caption,
                    theme.colors.text_muted,
                    false,
                )
                .exclude_from_hit_testing();
                let inset = Space::S8.px();
                layers.push(
                    ui::Box::new()
                        .style(Style::new().absolute_at(Some(inset), None, None, Some(inset)))
                        .child(hint)
                        .build()
                        .exclude_from_hit_testing(),
                );
            }
            let hover = frame_hover.clone();
            let notify = frame_invalidate.clone();
            let frame = Surface::Media(Media::Frame).build(&theme, layers).style(
                Style::new()
                    .width(Length::Px(width))
                    .height(Length::Px(width / aspect)),
            );
            frame.on_hover_change(move |over| {
                if hover.replace(over) != over {
                    notify();
                }
            })
        },
    );
    let placeholder_glyph = glyph.clone();
    let placeholder = std::rc::Rc::new(move |message: &str, failed: bool| {
        let mut content = ui::Box::new()
            .layout("column")
            .spaced(Space::S4)
            .padded(Space::S12)
            .width(Length::Pct(100.0))
            .height(Length::Pct(100.0))
            .style(
                Style::new()
                    .align(aqua_ui::AlignItems::CENTER)
                    .justify(aqua_ui::JustifyContent::CENTER)
                    .gpui_background(GpuiBackground::Solid(
                        theme.colors.surface.to_reference_paint(),
                    )),
            );
        if failed {
            content = content.child(placeholder_glyph(
                editor_assets::AssetId::TABLER_FILE_OFF,
                IconSize::Lg.px(),
                theme.colors.text_muted.to_reference_paint(),
            ));
        }
        let style = TextRole::Label.style(&theme);
        content
            .child(
                ui::Text::new(message.to_owned())
                    .text_style(style)
                    .gpui_source_color(theme.colors.text_muted.to_reference_text_color())
                    .wrap(false)
                    .overflow(aqua_text::overflow::OverflowMode::EndEllipsis)
                    .build(),
            )
            .build()
    });
    let toolbar_id = id.to_owned();
    let toolbar = std::rc::Rc::new(
        move |shading: Shading,
              moved: bool,
              facts: &str,
              change: std::rc::Rc<dyn Fn(Shading)>,
              reset: std::rc::Rc<dyn Fn()>| {
            let selection = crate::ui::segmented_control::SegmentedControl::new(
                format!("{toolbar_id}-shading"),
                theme,
                "Model shading",
                shading,
                crate::ui::segmented_control::Segment::new(
                    Shading::ALL[0],
                    Shading::ALL[0].label(),
                ),
            )
            .options(Shading::ALL[1..].iter().map(|shading| {
                crate::ui::segmented_control::Segment::new(*shading, shading.label())
            }))
            .on_change(move |value| change(value))
            .build();
            let mut controls = ui::Box::new()
                .layout("row")
                .spaced(Space::S2)
                .style(Style::new().align(aqua_ui::AlignItems::CENTER))
                .child(selection);
            if moved {
                controls = controls.child(
                    Button::text(
                        format!("{toolbar_id}-reset"),
                        theme,
                        ButtonTone::Quiet,
                        "Reset view",
                    )
                    .on_activate(move |_| reset())
                    .build(),
                );
            }
            ui::Box::new()
                .layout("row")
                .width(Length::Pct(100.0))
                .spaced(Space::S8)
                .style(
                    Style::new()
                        .align(aqua_ui::AlignItems::CENTER)
                        .justify(aqua_ui::JustifyContent::SPACE_BETWEEN),
                )
                .child(controls.build())
                .child(crate::ui::rich::narrative::callout::text_with_wrap(
                    &theme,
                    facts,
                    TextRole::Caption,
                    theme.colors.text_muted,
                    false,
                ))
                .build()
        },
    );
    let copy = std::rc::Rc::new(move |position, path: &str| {
        effect(crate::ui::media::preview::Effect::ShowCopyMenu(
            crate::ui::rich::actions::ShowCopyMenu {
                position,
                entries: vec![crate::ui::rich::actions::CopyEntry::primary(
                    "Copy Model Path",
                    path.to_owned(),
                )],
            },
        ))
    });
    ModelServices {
        assets,
        capability,
        frame,
        placeholder,
        toolbar,
        copy,
        invalidate,
    }
}
