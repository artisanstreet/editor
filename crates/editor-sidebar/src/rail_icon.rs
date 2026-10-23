//! The rail's destination icons, which act out their destination once per
//! hover: the pen writes on its page, the bag throws out what it holds, the
//! usage bars bounce like a level meter.
//!
//! Each icon is its Tabler original split into layers (`artisan.rail-*`),
//! drawn over one another in the 24-unit grid, so at rest it is the
//! original icon. Each is its own view: a playing icon re-renders only
//! itself, not the sidebar.
use editor_ui::AssetId;
use editor_ui::asset_seam::asset_glyph;
use editor_ui::motion::smooth_out;
use gpui::{
    Animation, AnimationExt as _, AnyElement, Context, ElementId, IntoElement, ParentElement as _,
    Render, Styled as _, Transformation, Window, div, point, px, size,
};
use std::f32::consts::{PI, TAU};
use std::time::{Duration, Instant};

/// The icon's square, in the rail button's centre.
pub const RAIL_ICON_SIZE: f32 = 18.0;
/// Pixels per unit of the icons' 24-unit grid.
const UNIT: f32 = RAIL_ICON_SIZE / 24.0;

#[derive(Clone, Copy)]
pub enum RailGlyph {
    /// New thread: a pen over a page.
    Pen,
    /// Marketplace: a shopping bag.
    Bag,
    /// Usage: three columns of level cells.
    Bars,
}

impl RailGlyph {
    const fn duration(self) -> Duration {
        match self {
            Self::Pen => Duration::from_millis(1000),
            Self::Bag => Duration::from_millis(950),
            Self::Bars => Duration::from_millis(1200),
        }
    }

    const fn name(self) -> &'static str {
        match self {
            Self::Pen => "pen",
            Self::Bag => "bag",
            Self::Bars => "bars",
        }
    }
}

pub struct RailIcon {
    glyph: RailGlyph,
    /// Counts plays; each play is a fresh animation.
    plays: u64,
    started: Option<Instant>,
}

impl RailIcon {
    pub const fn new(glyph: RailGlyph) -> Self {
        Self {
            glyph,
            plays: 0,
            started: None,
        }
    }

    /// Plays the icon's motion, unless it is still playing: the pointer
    /// leaving and coming back mid-play must not snap it to the start.
    pub fn play(&mut self, cx: &mut Context<Self>) {
        if self
            .started
            .is_some_and(|started| started.elapsed() < self.glyph.duration())
        {
            return;
        }
        self.plays += 1;
        self.started = Some(Instant::now());
        cx.notify();
    }
}

impl Render for RailIcon {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let glyph = self.glyph;
        let frame = div().relative().size(px(RAIL_ICON_SIZE));
        if self.plays == 0 {
            return frame.children(layers(glyph, None)).into_any_element();
        }
        frame
            .with_animation(
                ElementId::Name(format!("rail-icon-{}-{}", glyph.name(), self.plays).into()),
                Animation::new(glyph.duration()),
                move |frame, progress| frame.children(layers(glyph, Some(progress))),
            )
            .into_any_element()
    }
}

/// The icon's layers `progress` through its motion; `None` is at rest.
fn layers(glyph: RailGlyph, progress: Option<f32>) -> Vec<AnyElement> {
    match glyph {
        RailGlyph::Pen => pen(progress),
        RailGlyph::Bag => bag(progress),
        RailGlyph::Bars => bars(progress),
    }
}

fn layer(id: AssetId) -> gpui::Div {
    div()
        .absolute()
        .inset_0()
        .child(asset_glyph(id).size(px(RAIL_ICON_SIZE)))
}

fn moved(id: AssetId, transformation: Transformation) -> AnyElement {
    div()
        .absolute()
        .inset_0()
        .child(
            asset_glyph(id)
                .size(px(RAIL_ICON_SIZE))
                .with_transformation(transformation),
        )
        .into_any_element()
}

/// Rises from 0 to 1 over the first quarter and falls back over the last,
/// so a motion starts and ends at rest.
fn envelope(progress: f32) -> f32 {
    (progress * 4.0).min((1.0 - progress) * 4.0).clamp(0.0, 1.0)
}

/// The pen sweeps right and back across its page, writing three quick
/// strokes on the way with a flick of the wrist.
fn pen(progress: Option<f32>) -> Vec<AnyElement> {
    let p = progress.unwrap_or(1.0);
    let strokes = (3.0 * TAU * p).sin() * (PI * p).sin();
    let dx = 2.2 * (TAU * p).sin();
    let dy = -1.5 * strokes;
    vec![
        layer(AssetId::ARTISAN_RAIL_PEN_PAGE).into_any_element(),
        moved(
            AssetId::ARTISAN_RAIL_PEN_NIB,
            Transformation::translate(point(px(dx * UNIT), px(dy * UNIT)))
                .with_rotation(gpui::radians(-0.12 * strokes)),
        ),
    ]
}

/// What the bag throws out: the item, when it leaves (as a share of the
/// motion), and where it lands relative to the bag's mouth, in grid units.
const BAG_ITEMS: [(AssetId, f32, (f32, f32)); 3] = [
    (AssetId::ARTISAN_RAIL_BAG_ITEM_SPARK, 0.0, (0.0, -11.0)),
    (AssetId::ARTISAN_RAIL_BAG_ITEM_DOT, 0.15, (-9.0, -7.5)),
    (AssetId::ARTISAN_RAIL_BAG_ITEM_GEM, 0.3, (9.0, -7.0)),
];
/// Each item's flight, as a share of the motion.
const BAG_FLIGHT: f32 = 0.6;

/// The bag squashes as it throws three items up and out of its mouth; each
/// grows as it leaves and fades where it lands.
fn bag(progress: Option<f32>) -> Vec<AnyElement> {
    let p = progress.unwrap_or(1.0);
    let squash = (PI * (p / 0.5).min(1.0)).sin();
    let body = Transformation::scale(size(0.05f32.mul_add(squash, 1.0), 0.07f32.mul_add(-squash, 1.0)));
    let mut layers = vec![
        moved(AssetId::ARTISAN_RAIL_BAG_BODY, body),
        moved(AssetId::ARTISAN_RAIL_BAG_HANDLE, body),
    ];
    if progress.is_none() {
        return layers;
    }
    for (item, leaves, (x, y)) in BAG_ITEMS {
        let flight = ((p - leaves) / BAG_FLIGHT).clamp(0.0, 1.0);
        let opacity = (flight / 0.15).min((1.0 - flight) / 0.4).clamp(0.0, 1.0);
        if opacity <= 0.0 {
            continue;
        }
        let travel = smooth_out(flight);
        let grow = 0.6f32.mul_add((flight / 0.3).min(1.0), 0.4);
        layers.push(
            div()
                .absolute()
                .inset_0()
                .opacity(opacity)
                .child(
                    asset_glyph(item).size(px(RAIL_ICON_SIZE)).with_transformation(
                        Transformation::translate(point(
                            px(x * travel * UNIT),
                            px(y * travel * UNIT),
                        ))
                        .with_scaling(size(grow, grow)),
                    ),
                )
                .into_any_element(),
        );
    }
    layers
}

/// The level cells, column by column from the bottom.
const BAR_CELLS: [[AssetId; 5]; 3] = [
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
/// Each column's level at rest, as the Tabler icon draws it.
const BAR_REST: [f32; 3] = [5.0, 3.0, 4.0];
/// Each column's beat: how many swings over the motion, and where it starts.
const BAR_BEAT: [(f32, f32); 3] = [(2.0, 0.0), (2.7, 2.1), (2.3, 4.2)];

/// The columns jump between whole cells like a level meter, each to its own
/// beat, and settle back on the icon's levels.
fn bars(progress: Option<f32>) -> Vec<AnyElement> {
    let p = progress.unwrap_or(1.0);
    let swing = 2.4 * envelope(p);
    let mut layers = Vec::new();
    for ((cells, rest), (beats, phase)) in BAR_CELLS.iter().zip(BAR_REST).zip(BAR_BEAT) {
        let angle = (TAU * beats).mul_add(p, phase);
        let wave = 0.5f32.mul_add((2.7 * angle).sin(), angle.sin()) / 1.5;
        let level = (rest + swing * wave).round().clamp(1.0, 5.0);
        for (row, cell) in (1_u8..).zip(cells) {
            if f32::from(row) <= level {
                layers.push(layer(*cell).into_any_element());
            }
        }
    }
    layers
}
