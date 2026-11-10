use std::{
    f32::consts::{PI, TAU},
    time::{Duration, Instant},
};

use aqua_core::GpuiHsla;
use aqua_ui::{
    element::{Box as UiBox, Element},
    style::{Absolute, Length, Style},
};
use editor_assets::AssetId;

use editor_ui::picker_motion::smooth_out;

pub const ICON_SIZE: f32 = 18.0;
const UNIT: f32 = ICON_SIZE / 24.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Glyph {
    Pen,
    Bag,
    Bars,
}

impl Glyph {
    pub const fn duration(self) -> Duration {
        match self {
            Self::Pen => Duration::from_millis(1000),
            Self::Bag => Duration::from_millis(950),
            Self::Bars => Duration::from_millis(1200),
        }
    }
}

#[derive(Default)]
pub struct RailMotion {
    started: [Option<Instant>; 3],
}

impl RailMotion {
    fn index(glyph: Glyph) -> usize {
        match glyph {
            Glyph::Pen => 0,
            Glyph::Bag => 1,
            Glyph::Bars => 2,
        }
    }
    pub fn play(&mut self, glyph: Glyph, now: Instant) -> bool {
        let slot = &mut self.started[Self::index(glyph)];
        if slot.is_some_and(|started| now.saturating_duration_since(started) < glyph.duration()) {
            return false;
        }
        *slot = Some(now);
        true
    }
    pub fn progress(&self, glyph: Glyph, now: Instant) -> Option<f32> {
        self.started[Self::index(glyph)].map(|started| {
            (now.saturating_duration_since(started).as_secs_f32() / glyph.duration().as_secs_f32())
                .min(1.0)
        })
    }
    pub fn animating(&self, now: Instant) -> bool {
        [Glyph::Pen, Glyph::Bag, Glyph::Bars]
            .into_iter()
            .any(|glyph| self.progress(glyph, now).is_some_and(|p| p < 1.0))
    }
}

#[derive(Clone, Copy)]
struct Layer {
    asset: AssetId,
    dx: f32,
    dy: f32,
    sx: f32,
    sy: f32,
    angle: f32,
    opacity: f32,
}
impl Layer {
    fn new(asset: AssetId) -> Self {
        Self {
            asset,
            dx: 0.0,
            dy: 0.0,
            sx: 1.0,
            sy: 1.0,
            angle: 0.0,
            opacity: 1.0,
        }
    }
}

const CELLS: [[AssetId; 5]; 3] = [
    [
        AssetId::ARTISAN_RAIL_BARS_C1R1,
        AssetId::ARTISAN_RAIL_BARS_C1R2,
        AssetId::ARTISAN_RAIL_BARS_C1R3,
        AssetId::ARTISAN_RAIL_BARS_C1R4,
        AssetId::ARTISAN_RAIL_BARS_C1R5,
    ],
    [
        AssetId::ARTISAN_RAIL_BARS_C2R1,
        AssetId::ARTISAN_RAIL_BARS_C2R2,
        AssetId::ARTISAN_RAIL_BARS_C2R3,
        AssetId::ARTISAN_RAIL_BARS_C2R4,
        AssetId::ARTISAN_RAIL_BARS_C2R5,
    ],
    [
        AssetId::ARTISAN_RAIL_BARS_C3R1,
        AssetId::ARTISAN_RAIL_BARS_C3R2,
        AssetId::ARTISAN_RAIL_BARS_C3R3,
        AssetId::ARTISAN_RAIL_BARS_C3R4,
        AssetId::ARTISAN_RAIL_BARS_C3R5,
    ],
];

fn layers(glyph: Glyph, progress: Option<f32>) -> Vec<Layer> {
    let p = progress.unwrap_or(1.0);
    match glyph {
        Glyph::Pen => {
            let strokes = (3.0 * TAU * p).sin() * (PI * p).sin();
            let mut nib = Layer::new(AssetId::ARTISAN_RAIL_PEN_NIB);
            nib.dx = 2.2 * (TAU * p).sin() * UNIT;
            nib.dy = -1.5 * strokes * UNIT;
            nib.angle = -0.12 * strokes;
            vec![Layer::new(AssetId::ARTISAN_RAIL_PEN_PAGE), nib]
        }
        Glyph::Bag => {
            let squash = (PI * (p / 0.5).min(1.0)).sin();
            let mut body = Layer::new(AssetId::ARTISAN_RAIL_BAG_BODY);
            body.sx = 0.05_f32.mul_add(squash, 1.0);
            body.sy = 0.07_f32.mul_add(-squash, 1.0);
            let mut handle = body;
            handle.asset = AssetId::ARTISAN_RAIL_BAG_HANDLE;
            let mut result = vec![body, handle];
            if progress.is_some() {
                for (asset, leaves, x, y) in [
                    (AssetId::ARTISAN_RAIL_BAG_ITEM_SPARK, 0.0, 0.0, -11.0),
                    (AssetId::ARTISAN_RAIL_BAG_ITEM_DOT, 0.15, -9.0, -7.5),
                    (AssetId::ARTISAN_RAIL_BAG_ITEM_GEM, 0.3, 9.0, -7.0),
                ] {
                    let flight = ((p - leaves) / 0.6).clamp(0.0, 1.0);
                    let opacity = (flight / 0.15).min((1.0 - flight) / 0.4).clamp(0.0, 1.0);
                    if opacity <= 0.0 {
                        continue;
                    }
                    let mut layer = Layer::new(asset);
                    layer.dx = x * smooth_out(flight) * UNIT;
                    layer.dy = y * smooth_out(flight) * UNIT;
                    layer.sx = 0.6_f32.mul_add((flight / 0.3).min(1.0), 0.4);
                    layer.sy = layer.sx;
                    layer.opacity = opacity;
                    result.push(layer);
                }
            }
            result
        }
        Glyph::Bars => {
            let envelope = (p * 4.0).min((1.0 - p) * 4.0).clamp(0.0, 1.0);
            let mut result = Vec::new();
            for ((cells, rest), (beats, phase)) in
                CELLS
                    .iter()
                    .zip([5.0_f32, 3.0, 4.0])
                    .zip([(2.0_f32, 0.0), (2.7, 2.1), (2.3, 4.2)])
            {
                let angle = (TAU * beats).mul_add(p, phase);
                let wave = 0.5_f32.mul_add((2.7 * angle).sin(), angle.sin()) / 1.5;
                let level = (rest + 2.4 * envelope * wave).round().clamp(1.0, 5.0);
                for (index, cell) in cells.iter().enumerate() {
                    if (index + 1) as f32 <= level {
                        result.push(Layer::new(*cell));
                    }
                }
            }
            result
        }
    }
}

pub fn icon(glyph: Glyph, progress: Option<f32>, color: GpuiHsla) -> Element {
    let mut root = UiBox::new().width(ICON_SIZE).height(ICON_SIZE);
    for layer in layers(glyph, progress) {
        let source: std::sync::Arc<str> = editor_assets::get(layer.asset).source.into();
        root = root.child(
            aqua_ui::element::Canvas::new()
                .width(ICON_SIZE)
                .height(ICON_SIZE)
                .style(Style::new().absolute(Absolute {
                    left: Some(0.0),
                    top: Some(0.0),
                    right: None,
                    bottom: None,
                }))
                .on_paint(move |bounds| {
                    vec![aqua_core::DisplayItem::GpuiTransformedSvg {
                        bounds,
                        source: source.clone(),
                        tint: color,
                        opacity: layer.opacity,
                        transform: aqua_core::scene::GpuiSvgTransform {
                            scale: aqua_core::Point::new(layer.sx, layer.sy),
                            translation: aqua_core::Point::new(layer.dx, layer.dy),
                            rotation_radians: layer.angle,
                        },
                    }]
                })
                .build(),
        );
    }
    root.style(
        Style::new()
            .width(Length::Px(ICON_SIZE))
            .height(Length::Px(ICON_SIZE)),
    )
    .build()
}

pub fn avatar(seed: &str) -> Element {
    use editor_ui::theme::{RadiusStep, RadiusTokens};
    use aqua_core::DisplayItem;
    use aqua_ui::element::Canvas;
    let source: std::sync::Arc<str> = super::avatar::svg(seed).into();
    UiBox::new()
        .width(32.0)
        .height(32.0)
        .radius(RadiusTokens::value(RadiusStep::Md))
        .clip(true)
        .child(
            Canvas::new()
                .width(32.0)
                .height(32.0)
                .on_paint(move |bounds| {
                    vec![
                        DisplayItem::PushRoundedClip {
                            bounds,
                            corners: aqua_core::geometry::CornerRadii::uniform(
                                RadiusTokens::value(RadiusStep::Md),
                            ),
                        },
                        DisplayItem::GpuiSvg {
                            bounds,
                            source: source.clone(),
                            tint: None,
                            opacity: 1.0,
                        },
                        DisplayItem::PopClip,
                    ]
                })
                .build(),
        )
        .build()
}

pub fn build(
    sidebar: &super::controller::Sidebar,
    core: &editor_core::Core,
    theme: editor_ui::theme::ArtisanTheme,
    now: Instant,
    emit: std::rc::Rc<dyn Fn(super::view::ViewAction)>,
) -> Element {
    use super::view::ViewAction;
    use editor_core::Screen;
    use editor_ui::theme::{RadiusStep, RadiusTokens};
    use aqua_core::{CursorIcon, Insets, Role};
    use aqua_ui::{AlignItems, JustifyContent, element::Button};

    let screen = sidebar.screen(core);
    let items = [
        (
            Glyph::Pen,
            "sidebar-new-thread",
            "New thread",
            matches!(screen, Screen::NewThread { .. }),
        ),
        (
            Glyph::Bag,
            "sidebar-marketplace",
            "Marketplace",
            matches!(screen, Screen::Marketplace),
        ),
        (
            Glyph::Bars,
            "sidebar-usage",
            "Usage",
            matches!(screen, Screen::Usage),
        ),
    ];
    let mut destinations = UiBox::new()
        .layout("column")
        .gap(4.0)
        .style(Style::new().align(AlignItems::CENTER));
    for (glyph, id, label, selected) in items {
        let action = emit.clone();
        let hover = emit.clone();
        let color = if selected {
            theme.colors.text
        } else {
            theme.colors.text_muted
        };
        let button = Button::new()
            .width(32.0)
            .height(32.0)
            .radius(RadiusTokens::value(RadiusStep::Lg))
            .style(
                Style::new()
                    .align(AlignItems::CENTER)
                    .justify(JustifyContent::CENTER)
                    .flex_shrink(0.0),
            )
            .child(icon(
                glyph,
                sidebar.rail_motion.progress(glyph, now),
                color.to_reference_paint(),
            ))
            .on_activate(move || match glyph {
                Glyph::Pen => action(ViewAction::NewThread(None)),
                Glyph::Bag => action(ViewAction::Navigate(Screen::Marketplace)),
                Glyph::Bars => action(ViewAction::Navigate(Screen::Usage)),
            })
            .build()
            .with_key(id)
            .role(Role::Button)
            .aria_label(label)
            .aria_selected(selected)
            .cursor(CursorIcon::Pointer)
            .on_hover_change(move |hovered| {
                if hovered {
                    hover(ViewAction::RailHover(glyph));
                }
            });
        destinations = destinations.child(button);
    }
    let profile = core.account().profile();
    let host = sidebar.host_data(core);
    let seed = profile
        .map(|profile| profile.account_id.clone())
        .or_else(|| {
            host.map(|host| {
                core.hosts()
                    .find(|(config, _)| config.id == host.id)
                    .map_or_else(
                        || host.id.0.clone(),
                        |(config, _)| config.fingerprint.clone(),
                    )
            })
        })
        .unwrap_or_else(|| "no-host".into());
    let name = profile
        .map(|profile| {
            if profile.display_name.is_empty() {
                profile.email.clone()
            } else {
                profile.display_name.clone()
            }
        })
        .or_else(|| host.map(|host| host.name.clone()))
        .unwrap_or_else(|| "Artisan".into());
    let machine = host.map_or("No host", |host| host.name.as_str());
    let mut trigger = Button::new()
        .width(40.0)
        .height(40.0)
        .padding(4.0)
        .radius(RadiusTokens::value(RadiusStep::Lg))
        .child(avatar(&seed));
    if sidebar.profile_open {
        trigger = trigger.style(
            editor_ui::hover::HoverColors::new(&theme, editor_ui::hover::HoverTone::Neutral)
                .style(editor_ui::hover::HoverFill::Plain),
        );
    }
    if host.is_some_and(|host| host.status_line().is_some()) {
        trigger = trigger.child(
            UiBox::new()
                .width(10.0)
                .height(10.0)
                .radius(5.0)
                .style(
                    Style::new().gpui_background(aqua_core::GpuiBackground::Solid(
                        theme.colors.banner_warning.text.to_reference_paint(),
                    )),
                )
                .style(Style::new().gpui_border(
                    Insets::uniform(2.0),
                    theme.colors.frame.to_reference_paint(),
                    false,
                ))
                .style(Style::new().absolute_at(None, None, Some(1.0), Some(1.0)))
                .build(),
        );
    }
    let show = emit;
    let trigger = trigger
        .on_activate(move || show(ViewAction::ToggleProfile))
        .build()
        .with_key("artisan-desktop-profile-trigger")
        .debug_selector("artisan-desktop-profile-trigger")
        .role(Role::Button)
        .aria_label(format!("{name} on {machine}: account and usage"))
        .aria_expanded(sidebar.profile_open)
        .cursor(CursorIcon::Pointer);
    UiBox::new()
        .width(48.0)
        .height(Length::Pct(100.0))
        .layout("column")
        .style(
            Style::new()
                .flex_shrink(0.0)
                .align(AlignItems::CENTER)
                .justify(JustifyContent::SPACE_BETWEEN)
                .padding_insets(Insets {
                    top: 4.0,
                    bottom: 4.0,
                    left: 0.0,
                    right: 0.0,
                }),
        )
        .child(destinations.build())
        .child(trigger)
        .build()
        .with_key("artisan-desktop-rail")
}
