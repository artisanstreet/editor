use crate::conversation::compaction::{CompactionFrame, CompactionPhase, eased};
use crate::ui::{
    asset::source_asset_glyph,
    motion::MotionCurve,
    shimmer_text::{ShimmerMotion, ShimmerTiming},
    theme::{ArtisanTheme, ProseTypography},
};
use aqua_core::{CornerRadii, DisplayItem, GpuiBackground, NodeId, Rect};
use aqua_text::{FontFamily, TextStyle};
use aqua_ui::{
    AlignItems, JustifyContent,
    element::{Box as UiBox, Canvas, Element, Key, Text},
    style::{Length, Style},
    tree::hash_key,
};
use artisan_api::{Item, ItemKind, ModelSwitch, NamedModel, Status};

const SHIMMER_KEY: &str = "compaction-shimmer";

fn rule(frame: CompactionFrame, left: bool, theme: ArtisanTheme) -> Element {
    Canvas::new()
        .height(1.0)
        .style(Style::new().flex_grow(1.0).min_width(Length::Px(0.0)))
        .on_paint(move |bounds| {
            let line = |x, width, color| DisplayItem::GpuiBackground {
                bounds: Rect {
                    x,
                    y: bounds.y,
                    w: width,
                    h: 1.0,
                },
                corners: CornerRadii::uniform(0.0),
                background: GpuiBackground::Solid(color),
            };
            let length = bounds.w * frame.draw;
            let start = if left {
                bounds.x + bounds.w - length
            } else {
                bounds.x
            };
            let mut result = vec![line(start, length, theme.colors.line.to_reference_paint())];
            if let Some(pulse) = frame.pulse {
                let travel = eased(MotionCurve::SmoothOut, pulse);
                let alpha = 0.55 * (std::f32::consts::PI * pulse).sin();
                let lane = (bounds.w - 56.0).max(0.0);
                let x = if left {
                    bounds.x + lane * (1.0 - travel)
                } else {
                    bounds.x + lane * travel
                };
                result.push(line(
                    x,
                    56.0,
                    theme.colors.text.fade(alpha).to_reference_paint(),
                ));
            }
            result
        })
        .build()
}

fn text(value: impl Into<String>, theme: ArtisanTheme) -> Element {
    Text::new(value.into())
        .gpui_source_color(theme.colors.text_muted.to_reference_text_color())
        .style(Style::new().text_wrap(false))
        .build()
}

fn switch_line(switch: &ModelSwitch, theme: ArtisanTheme) -> Element {
    let model = |model: &NamedModel| {
        UiBox::new()
            .gap(4.0)
            .style(Style::new().align(AlignItems::CENTER))
            .child(source_asset_glyph(
                crate::sidebar::usage_view::engine_icon(&theme, model.engine),
                14.0,
                theme.colors.text_muted.to_reference_paint(),
            ))
            .child(text(model.name.clone(), theme))
            .build()
    };
    UiBox::new()
        .gap(6.0)
        .style(
            Style::new()
                .align(AlignItems::CENTER)
                .gpui_source_text_color(theme.colors.text_muted.to_reference_text_color()),
        )
        .child(text("Changed from", theme))
        .child(model(&switch.from))
        .child(text("to", theme))
        .child(model(&switch.to))
        .build()
}

fn displayed_phase(
    phase: CompactionPhase,
    switch: bool,
    frame: CompactionFrame,
) -> CompactionPhase {
    match frame.swap.filter(|_| !switch) {
        Some((from, swap)) if swap < 0.5 => from,
        _ => phase,
    }
}

pub fn label(
    phase: CompactionPhase,
    switch: Option<&ModelSwitch>,
    frame: CompactionFrame,
    theme: ArtisanTheme,
) -> Element {
    let widest = if phase == CompactionPhase::Stopped {
        CompactionPhase::Stopped
    } else {
        CompactionPhase::Running
    };
    let content = |value: CompactionPhase| {
        switch.map_or_else(
            || text(value.label(), theme),
            |switch| switch_line(switch, theme),
        )
    };
    let placeholder = UiBox::new()
        .style(Style::new().opacity(0.0))
        .child(content(widest))
        .build();
    let shown = displayed_phase(phase, switch.is_some(), frame);
    let (opacity, top, blur) = match frame.swap.filter(|_| switch.is_none()) {
        Some((_, swap)) if swap < 0.5 => {
            let out = eased(MotionCurve::EaseInOut, swap * 2.0);
            (1.0 - out, -4.0 * out, 2.0 * out)
        }
        Some((_, swap)) => {
            let enter = eased(MotionCurve::EaseInOut, 2.0f32.mul_add(swap, -1.0));
            (enter, 4.0 * (1.0 - enter), 2.0 * (1.0 - enter))
        }
        None if frame.label < 1.0 => (
            frame.label,
            4.0 * (1.0 - frame.label),
            3.0 * (1.0 - frame.label),
        ),
        None => (1.0, 0.0, 0.0),
    };
    let mut visible_style = Style::new()
        .absolute_at(Some(0.0), Some(top), Some(0.0), None)
        .justify(JustifyContent::CENTER)
        .opacity(opacity);
    if blur > 0.0 {
        visible_style = visible_style.content_blur(blur);
    }
    let visible = UiBox::new()
        .style(visible_style)
        .child(content(shown).with_key(SHIMMER_KEY))
        .build();
    UiBox::new()
        .style(Style::new().flex_shrink(0.0))
        .child(placeholder)
        .child(visible)
        .build()
}

pub fn sync_label(
    app: &mut aqua::UiApp,
    row: NodeId,
    phase: CompactionPhase,
    has_switch: bool,
    frame: CompactionFrame,
    theme: &ArtisanTheme,
    reduced_motion: bool,
) -> bool {
    let key = hash_key(Key::from(SHIMMER_KEY));
    let mut stack = vec![row];
    while let Some(node) = stack.pop() {
        if let Some(record) = app.tree().get(node) {
            stack.extend(record.children.iter().copied());
            if record.key == Some(key) {
                let mut motion = ShimmerMotion::new(theme, ShimmerTiming::Progress);
                motion.set_active(
                    displayed_phase(phase, has_switch, frame) == CompactionPhase::Running,
                );
                return motion.sync(app, node, reduced_motion);
            }
        }
    }
    false
}

pub fn row(
    item: &Item,
    frame: CompactionFrame,
    theme: ArtisanTheme,
    label: impl FnOnce(CompactionPhase, Option<&ModelSwitch>, CompactionFrame, ArtisanTheme) -> Element,
) -> Option<Element> {
    let ItemKind::Compaction { switch, .. } = &item.kind else {
        return None;
    };
    let phase = match item.status {
        Status::Running => CompactionPhase::Running,
        Status::Done => CompactionPhase::Done,
        Status::AwaitingUser | Status::Failed | Status::Declined | Status::Cancelled => {
            CompactionPhase::Stopped
        }
    };
    let typography = TextStyle {
        families: vec![FontFamily::Named(theme.typography.sans.family.to_owned())],
        size_px: ProseTypography::BODY_SIZE_PX,
        line_height_px: 24.0,
        weight: ProseTypography::BODY_WEIGHT,
        letter_spacing_em: Some(theme.typography.tracking_em),
        ..TextStyle::default()
    };
    Some(
        UiBox::new()
            .layout("row")
            .width(Length::Pct(100.0))
            .height(24.0 * frame.open)
            .clip(true)
            .gap(12.0)
            .style(
                Style::new()
                    .min_width(Length::Px(0.0))
                    .align(AlignItems::CENTER)
                    .text_style(typography)
                    .gpui_source_text_color(theme.colors.text_muted.to_reference_text_color()),
            )
            .child(rule(frame, true, theme))
            .child(label(phase, switch.as_ref(), frame, theme))
            .child(rule(frame, false, theme))
            .build()
            .debug_selector("compaction"),
    )
}
