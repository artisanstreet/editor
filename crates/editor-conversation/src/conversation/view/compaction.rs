use crate::conversation::compaction::{CompactionFrame, CompactionPhase, eased};
use editor_ui::{
    kit::{IconSize, Space, Spaced, TextRole},
    motion::MotionCurve,
    shimmer_text::{ShimmerTiming, sweep},
    theme::ArtisanTheme,
};
use aqua_core::{DisplayItem, Rect, scene::TextShimmer};
use aqua_ui::{
    AlignItems, JustifyContent,
    element::{Box as UiBox, Canvas, Element, Text},
    style::{Length, Style},
};
use artisan_api::{Item, ItemKind, ModelSwitch, NamedModel, Status};

const SHIMMER_KEY: &str = "compaction-shimmer";
const RULE_LINE: f32 = 1.0;
const PULSE_LENGTH: f32 = 56.0;
const ROLE: TextRole = TextRole::Prose;

#[expect(
    clippy::disallowed_types,
    reason = "the compaction rule is an animated painted guide: it draws in and carries a travelling pulse"
)]
fn rule(frame: CompactionFrame, left: bool, theme: ArtisanTheme) -> Element {
    Canvas::new()
        .height(RULE_LINE)
        .style(Style::new().flex_grow(1.0).min_width(Length::Pct(0.0)))
        .on_paint(move |bounds| {
            let line = |x, width, color| DisplayItem::GpuiBackground {
                bounds: Rect {
                    x,
                    y: bounds.y,
                    w: width,
                    h: RULE_LINE,
                },
                corners: aqua_core::CornerRadii::uniform(0.0),
                background: aqua_core::GpuiBackground::Solid(color),
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
                let lane = (bounds.w - PULSE_LENGTH).max(0.0);
                let x = if left {
                    bounds.x + lane * (1.0 - travel)
                } else {
                    bounds.x + lane * travel
                };
                result.push(line(
                    x,
                    PULSE_LENGTH,
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

fn shimmering(style: Style, shimmer: Option<TextShimmer>) -> Style {
    match shimmer {
        Some(shimmer) => style.text_shimmer(shimmer),
        None => style,
    }
}

fn switch_line(
    switch: &ModelSwitch,
    shimmer: Option<TextShimmer>,
    theme: ArtisanTheme,
) -> Element {
    let model = |model: &NamedModel| {
        UiBox::new()
            .spaced(Space::S4)
            .style(Style::new().align(AlignItems::CENTER))
            .child(IconSize::Md.glyph(
                editor_ui::engine::engine_icon(&theme, model.engine),
                theme.colors.text.to_reference_paint(),
            ))
            .child(
                Text::new(model.name.clone())
                    .gpui_source_color(theme.colors.text.to_reference_text_color())
                    .style(Style::new().text_wrap(false))
                    .build(),
            )
            .build()
    };
    UiBox::new()
        .spaced(Space::S6)
        .style(shimmering(
            Style::new()
                .align(AlignItems::CENTER)
                .gpui_source_text_color(theme.colors.text_muted.to_reference_text_color()),
            shimmer,
        ))
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
    shimmer_key: Option<u64>,
) -> Element {
    let widest = if phase == CompactionPhase::Stopped {
        CompactionPhase::Stopped
    } else {
        CompactionPhase::Running
    };
    let content = |value: CompactionPhase, shimmer: Option<TextShimmer>| {
        switch.map_or_else(
            || {
                UiBox::new()
                    .style(shimmering(Style::new(), shimmer))
                    .child(text(value.label(), theme))
                    .build()
            },
            |switch| switch_line(switch, shimmer, theme),
        )
    };
    let placeholder = UiBox::new()
        .style(Style::new().opacity(0.0))
        .child(content(widest, None))
        .build();
    let shown = displayed_phase(phase, switch.is_some(), frame);
    let shimmer = shimmer_key
        .filter(|_| shown == CompactionPhase::Running)
        .map(|key| sweep(&theme, ShimmerTiming::Progress, key));
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
        .child(content(shown, shimmer).with_key(SHIMMER_KEY))
        .build();
    UiBox::new()
        .style(Style::new().flex_shrink(0.0))
        .child(placeholder)
        .child(visible)
        .build()
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
    Some(
        UiBox::new()
            .layout("row")
            .width(Length::Pct(100.0))
            .height(ROLE.spec().line * frame.open)
            .clip(true)
            .spaced(Space::S12)
            .style(ROLE.inherit(&theme))
            .style(
                Style::new()
                    .min_width(Length::Pct(0.0))
                    .align(AlignItems::CENTER)
                    .gpui_source_text_color(theme.colors.text_muted.to_reference_text_color()),
            )
            .child(rule(frame, true, theme))
            .child(label(phase, switch.as_ref(), frame, theme))
            .child(rule(frame, false, theme))
            .build()
            .debug_selector("compaction"),
    )
}
