//! The profile menu's Usage section, ported from the previous Editor's
//! `desktop_profile_usage` (`sidebar-engine-usage.svelte`): per engine its
//! mark and name with a "last checked" reading that turns into Refresh on
//! hover, one group per cadence with 72×8 accent meters, and the sentence
//! saying when the group's limit resets.

use crate::Sidebar;
use artisan_api::{EngineId, EngineStatus, Timestamp, UsageLimits, UsageWindow};
use editor_ui::AssetId;
use editor_ui::glass::{
    GlassStrength, glass_blur_radius, glass_card_shadows, glass_foreground_base,
    glass_highlight_layer, glass_material_layer,
};
use editor_ui::icon::{IconSize, IconStyle, IconTint, icon};
use editor_ui::picker_motion::HoverRect;
use editor_ui::theme::{ArtisanTheme, DesktopTheme, RadiusStep, RadiusTokens};
use gpui::{
    Bounds, Context, Div, FontWeight, HighlightStyle, Hsla, InteractiveElement as _, IntoElement,
    ParentElement as _, Pixels, SharedString, Stateful, StatefulInteractiveElement as _,
    Styled as _, StyledText, canvas, div, px,
};
use std::cell::RefCell;
use std::collections::BTreeSet;
use std::rc::Rc;

/// Ticks in a meter, as the previous Editor quantized it.
const METER_SEGMENTS: usize = 14;
/// One tick: fourteen 72 px pitches less their 2 px gap.
#[allow(clippy::cast_precision_loss)]
const METER_TICK: f32 = 72.0 / METER_SEGMENTS as f32 - 2.0;
/// The engine title sits a little further from its first group.
const TITLE_GAP: f32 = 4.0;
/// The tooltip's width cap and its distance from the meter row.
const TOOLTIP_WIDTH: f32 = 224.0;
const TOOLTIP_OFFSET: f32 = 8.0;

/// The provider-disclosed cadence a window belongs to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Cadence {
    Session,
    Weekly,
    Monthly,
    Unknown,
}

impl Cadence {
    const fn title(self) -> &'static str {
        match self {
            Self::Session => "Session",
            Self::Weekly => "Weekly",
            Self::Monthly => "Monthly",
            Self::Unknown => "Usage",
        }
    }

    fn of(word: &str) -> Option<Self> {
        match word.trim().to_lowercase().as_str() {
            "session" | "5 hours" => Some(Self::Session),
            "week" | "weekly" | "7 days" => Some(Self::Weekly),
            "month" | "monthly" => Some(Self::Monthly),
            _ => None,
        }
    }
}

/// A window as the menu shows it: its cadence and the scope it covers
/// (`None` reads "All models").
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Meter {
    pub(crate) index: usize,
    pub(crate) cadence: Cadence,
    pub(crate) scope: Option<String>,
    pub(crate) used_percent: f64,
    pub(crate) resets_at: Option<Timestamp>,
}

/// Reads Forge's window label: `Session`, `Week`, `Week (Opus)`,
/// `GPT-5 (Session)`, or a scope with no cadence (`Cursor models`).
pub(crate) fn meter(index: usize, window: &UsageWindow) -> Meter {
    let (cadence, scope) = match window.label.rsplit_once(" (") {
        Some((head, tail)) => {
            let tail = tail.trim_end_matches(')');
            match (Cadence::of(tail), Cadence::of(head)) {
                (Some(cadence), _) => (cadence, Some(head.to_owned())),
                (None, Some(cadence)) => (cadence, Some(tail.to_owned())),
                (None, None) => (Cadence::Unknown, Some(window.label.clone())),
            }
        }
        None => match Cadence::of(&window.label) {
            Some(cadence) => (cadence, None),
            None => (Cadence::Unknown, Some(window.label.clone())),
        },
    };
    Meter {
        index,
        cadence,
        scope,
        used_percent: window.used_percent,
        resets_at: window.resets_at,
    }
}

/// Groups in cadence order; account-wide windows before scoped ones.
pub(crate) fn groups(usage: &UsageLimits) -> Vec<(Cadence, Vec<Meter>)> {
    let mut meters: Vec<Meter> = usage
        .windows
        .iter()
        .enumerate()
        .filter(|(_, window)| (0.0..=100.0).contains(&window.used_percent))
        .map(|(index, window)| meter(index, window))
        .collect();
    meters.sort_by_key(|meter| (meter.cadence, meter.scope.is_some(), meter.index));
    let mut groups: Vec<(Cadence, Vec<Meter>)> = Vec::new();
    for meter in meters {
        match groups.last_mut() {
            Some((cadence, list)) if *cadence == meter.cadence => list.push(meter),
            _ => groups.push((meter.cadence, vec![meter])),
        }
    }
    groups
}

/// "3 hours", once every window of a group has a future reset; the latest
/// one counts.
pub(crate) fn reset_duration(meters: &[Meter], now: Timestamp) -> Option<String> {
    const MINUTE: i64 = 60_000;
    let mut latest: Option<Timestamp> = None;
    for meter in meters {
        let at = meter.resets_at.filter(|at| *at > now)?;
        latest = Some(latest.map_or(at, |latest| latest.max(at)));
    }
    let minutes = ((latest? - now) + MINUTE - 1) / MINUTE;
    let minutes = minutes.max(1);
    let (amount, unit) = if minutes < 60 {
        (minutes, "minute")
    } else if minutes < 24 * 60 {
        ((minutes + 59) / 60, "hour")
    } else {
        ((minutes + 24 * 60 - 1) / (24 * 60), "day")
    };
    Some(if amount == 1 {
        format!("1 {unit}")
    } else {
        format!("{amount} {unit}s")
    })
}

/// The reading beside an engine's name.
pub(crate) fn checked_label(read_at: Timestamp, now: Timestamp) -> String {
    let minutes = ((now - read_at) / 60_000).max(0);
    if minutes < 1 {
        return "last checked now".to_owned();
    }
    if minutes < 60 {
        return format!("last checked {minutes} min ago");
    }
    let hours = minutes / 60;
    if hours < 24 {
        return format!("last checked {hours} hr ago");
    }
    format!("last checked {} d ago", hours / 24)
}

/// Lit ticks: the used share rounded up to a whole tick.
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss
)]
pub(crate) fn lit_ticks(used_percent: f64) -> usize {
    (used_percent.clamp(0.0, 100.0) / 100.0 * METER_SEGMENTS as f64).ceil() as usize
}

/// What the tooltip says is left.
#[allow(clippy::cast_possible_truncation)]
pub(crate) fn remaining_percent(used_percent: f64) -> i64 {
    (100.0 - used_percent.round()).clamp(0.0, 100.0) as i64
}

pub(crate) fn engine_asset(engine: EngineId) -> AssetId {
    match engine {
        EngineId::Claude => AssetId::SVGL_CLAUDE_AI,
        EngineId::Codex => AssetId::SVGL_OPENAI,
        EngineId::OpenCode => AssetId::BRANDS_OPENCODE,
        EngineId::Grok => AssetId::SVGL_GROK,
        EngineId::Cursor => AssetId::SVGL_CURSOR,
    }
}

/// The meter accent (`lib/engine/presentation.ts`).
fn engine_accent(engine: EngineId) -> Hsla {
    let hex = match engine {
        EngineId::Claude => 0x00d9_7757,
        EngineId::Codex => 0x0010_a37f,
        EngineId::Cursor | EngineId::Grok | EngineId::OpenCode => 0x006b_7280,
    };
    gpui::rgb_to_hsla(gpui::rgb(hex))
}

/// The product name the menu titles a block with.
pub(crate) const fn engine_name(engine: EngineId) -> &'static str {
    engine.display_name()
}

/// One meter: its engine and its window's place in the report.
type MeterKey = (EngineId, usize);
/// The hovered meter and its row's place in the menu.
type TipAnchor = Rc<RefCell<Option<(MeterKey, HoverRect)>>>;

/// The Usage section's pointer state.
#[derive(Default)]
pub(crate) struct UsageUi {
    /// Engines whose refresh is in flight.
    pub(crate) refreshing: BTreeSet<EngineId>,
    /// The engine whose reading the pointer is on.
    pub(crate) refresh_hover: Option<EngineId>,
    /// The hovered meter and its row's place in the menu.
    pub(crate) meter_hover: Option<MeterKey>,
    pub(crate) tip_anchor: TipAnchor,
    pub(crate) menu_bounds: Rc<RefCell<Option<Bounds<Pixels>>>>,
}

impl UsageUi {
    pub(crate) fn clear(&mut self) {
        self.refresh_hover = None;
        self.meter_hover = None;
        self.tip_anchor.borrow_mut().take();
    }

    /// Records the menu panel's bounds, for the tooltip's place.
    pub(crate) fn menu_probe(&self) -> impl IntoElement {
        let bounds = Rc::clone(&self.menu_bounds);
        canvas(
            |_, _, _| {},
            move |measured, (), window, cx| {
                if bounds.borrow_mut().replace(measured) != Some(measured) {
                    window.defer(cx, |window, _| window.refresh());
                }
            },
        )
        .absolute()
        .top_0()
        .left_0()
        .size_full()
    }
}

/// Engines with something to show, in the host's order.
pub(crate) fn shown<'a>(
    engines: impl Iterator<Item = &'a EngineStatus>,
) -> Vec<(EngineId, UsageLimits)> {
    engines
        .filter_map(|status| {
            let usage = status.usage.clone()?;
            (!groups(&usage).is_empty()).then_some((status.engine, usage))
        })
        .collect()
}

/// The whole section: engine blocks separated by hairlines.
pub(crate) fn section(
    sidebar: &Sidebar,
    engines: &[(EngineId, UsageLimits)],
    theme: &ArtisanTheme,
    desktop: DesktopTheme,
    separator: Hsla,
    cx: &mut Context<Sidebar>,
) -> Stateful<Div> {
    let now = artisan_api::now();
    let mut section = div()
        .id("artisan-profile-usage")
        .debug_selector(|| "artisan-profile-usage".to_owned())
        .flex()
        .flex_col()
        .px(px(4.0))
        .py(px(4.0));
    for (index, (engine, usage)) in engines.iter().enumerate() {
        if index > 0 {
            section = section.child(div().h(px(1.0)).bg(separator).my(px(4.0)));
        }
        section = section.child(engine_block(
            sidebar, *engine, usage, now, theme, desktop, cx,
        ));
    }
    section
}

#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
fn engine_block(
    sidebar: &Sidebar,
    engine: EngineId,
    usage: &UsageLimits,
    now: Timestamp,
    theme: &ArtisanTheme,
    desktop: DesktopTheme,
    cx: &mut Context<Sidebar>,
) -> Div {
    let accent = engine_accent(engine);
    let dim = theme.colors.foreground.with_alpha(0.11).to_paint();
    let refreshing = sidebar.usage.refreshing.contains(&engine);
    let block_selector = format!("artisan-profile-usage-engine-{engine}");
    let title = div()
        .flex()
        .items_center()
        .justify_between()
        .gap(px(8.0))
        .mb(px(TITLE_GAP))
        .child(
            div()
                .flex()
                .min_w(px(0.0))
                .items_center()
                .gap(px(8.0))
                .text_color(desktop.foreground)
                .child(
                    icon(IconStyle::resolve(
                        *theme,
                        engine_asset(engine),
                        IconSize::Default,
                        IconTint::Inherit,
                    ))
                    .size(px(16.0))
                    .flex_shrink_0(),
                )
                .child(
                    div()
                        .truncate()
                        .text_size(px(12.0))
                        .line_height(px(16.0))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(desktop.foreground)
                        .child(engine_name(engine)),
                ),
        )
        .child(refresh_control(
            sidebar,
            engine,
            &checked_label(usage.read_at, now),
            refreshing,
            theme,
            desktop,
            cx,
        ));
    let mut block = div()
        .debug_selector(move || block_selector.clone())
        .flex()
        .flex_col()
        .gap(px(6.0))
        .px(px(8.0))
        .py(px(4.0))
        .child(title);
    for (group_index, (cadence, meters)) in groups(usage).into_iter().enumerate() {
        let mut group = div().flex().flex_col().gap(px(6.0));
        if group_index > 0 {
            group = group.mt(px(8.0));
        }
        group = group.child(
            div()
                .text_size(px(12.0))
                .line_height(px(16.0))
                .font_weight(FontWeight::MEDIUM)
                .text_color(desktop.foreground)
                .child(cadence.title()),
        );
        for meter in &meters {
            group = group.child(meter_row(sidebar, engine, meter, accent, dim, desktop, cx));
        }
        if let Some(duration) = reset_duration(&meters, now) {
            let sentence = format!(
                "Your {} limit resets in {duration}.",
                cadence.title().to_lowercase()
            );
            let body = match sentence.find(duration.as_str()) {
                Some(start) => StyledText::new(SharedString::from(sentence.clone()))
                    .with_highlights([(
                        start..start + duration.len(),
                        HighlightStyle {
                            color: Some(desktop.foreground),
                            ..Default::default()
                        },
                    )]),
                None => StyledText::new(SharedString::from(sentence)),
            };
            group = group.child(
                div()
                    .mt(px(8.0))
                    .text_size(px(12.0))
                    .line_height(px(16.0))
                    .text_color(desktop.secondary)
                    .child(body),
            );
        }
        block = block.child(group);
    }
    block
}

/// The reading beside the name: "last checked …" at rest, Refresh under
/// the pointer, "Checking…" while the refresh runs. All three share one
/// cell sized to the widest, so the swap never moves the row.
fn refresh_control(
    sidebar: &Sidebar,
    engine: EngineId,
    checked: &str,
    refreshing: bool,
    _theme: &ArtisanTheme,
    desktop: DesktopTheme,
    cx: &mut Context<Sidebar>,
) -> Stateful<Div> {
    let selector = format!("artisan-profile-usage-refresh-{engine}");
    let hovered = sidebar.usage.refresh_hover == Some(engine);
    let shown = if refreshing {
        2
    } else if hovered {
        1
    } else {
        0
    };
    let layer = |index: usize, text: String, color: Hsla| {
        div()
            .col_start(1)
            .row_start(1)
            .flex()
            .items_center()
            .justify_end()
            .whitespace_nowrap()
            .text_color(color)
            .opacity(if index == shown { 1.0 } else { 0.0 })
            .child(text)
    };
    let mut control = div()
        .id(SharedString::from(selector.clone()))
        .debug_selector(move || selector.clone())
        .grid()
        .flex_shrink_0()
        .text_size(px(12.0))
        .line_height(px(16.0))
        .child(layer(0, checked.to_owned(), desktop.secondary))
        .child(layer(1, "Refresh".to_owned(), desktop.foreground))
        .child(layer(2, "Checking\u{2026}".to_owned(), desktop.secondary))
        .on_hover(cx.listener(move |sidebar, hovered: &bool, _, cx| {
            if *hovered {
                sidebar.usage.refresh_hover = Some(engine);
            } else if sidebar.usage.refresh_hover == Some(engine) {
                sidebar.usage.refresh_hover = None;
            }
            cx.notify();
        }));
    if !refreshing {
        control = control
            .cursor_pointer()
            .on_click(cx.listener(move |sidebar, _, _, cx| {
                cx.stop_propagation();
                sidebar.refresh_usage(engine, cx);
            }));
    }
    control
}

/// One meter row: the scope, then fourteen ticks lit by the used share.
/// Hovering shows what is left in the glass tooltip.
#[allow(clippy::too_many_arguments)]
fn meter_row(
    sidebar: &Sidebar,
    engine: EngineId,
    meter: &Meter,
    accent: Hsla,
    dim: Hsla,
    desktop: DesktopTheme,
    cx: &mut Context<Sidebar>,
) -> Stateful<Div> {
    let lit = lit_ticks(meter.used_percent);
    let key = (engine, meter.index);
    let selector = format!("artisan-profile-usage-meter-{engine}-{}", meter.index);
    let mut bar = div().w(px(72.0)).h(px(8.0)).flex_shrink_0().flex();
    for index in 0..METER_SEGMENTS {
        bar = bar.child(
            div()
                .w(px(METER_TICK))
                .mr(px(2.0))
                .h_full()
                .bg(if index < lit { accent } else { dim }),
        );
    }
    let probe = {
        let (anchor, surface) = (
            Rc::clone(&sidebar.usage.tip_anchor),
            Rc::clone(&sidebar.usage.menu_bounds),
        );
        let hovered = sidebar.usage.meter_hover == Some(key);
        canvas(
            |_, _, _| {},
            move |bounds, (), window, cx| {
                if !hovered {
                    return;
                }
                let Some(surface) = *surface.borrow() else {
                    return;
                };
                let rect = HoverRect::within(bounds, surface);
                let mut anchor = anchor.borrow_mut();
                if anchor.as_ref() == Some(&(key, rect)) {
                    return;
                }
                *anchor = Some((key, rect));
                window.defer(cx, |window, _| window.refresh());
            },
        )
        .absolute()
        .top_0()
        .left_0()
        .size_full()
    };
    div()
        .id(SharedString::from(selector.clone()))
        .debug_selector(move || selector.clone())
        .relative()
        .flex()
        .items_center()
        .gap(px(16.0))
        .child(probe)
        .child(
            div()
                .min_w(px(0.0))
                .flex_1()
                .truncate()
                .pl(px(8.0))
                .text_size(px(12.0))
                .line_height(px(16.0))
                .text_color(desktop.secondary)
                .child(
                    meter
                        .scope
                        .clone()
                        .unwrap_or_else(|| "All models".to_owned()),
                ),
        )
        .child(bar)
        .on_hover(cx.listener(move |sidebar, hovered: &bool, _, cx| {
            if *hovered {
                sidebar.usage.meter_hover = Some(key);
            } else if sidebar.usage.meter_hover == Some(key) {
                sidebar.usage.meter_hover = None;
                sidebar.usage.tip_anchor.borrow_mut().take();
            }
            cx.notify();
        }))
}

/// The glass tooltip beside the hovered meter: "You have N% left."
pub(crate) fn tooltip(
    sidebar: &Sidebar,
    engines: &[(EngineId, UsageLimits)],
    theme: &ArtisanTheme,
    desktop: DesktopTheme,
) -> Option<Stateful<Div>> {
    let key = sidebar.usage.meter_hover?;
    let (anchor_key, anchor) = (*sidebar.usage.tip_anchor.borrow())?;
    if anchor_key != key {
        return None;
    }
    let window = engines
        .iter()
        .find(|(engine, _)| *engine == key.0)?
        .1
        .windows
        .get(key.1)?;
    let radius = RadiusTokens::value(RadiusStep::X2l);
    Some(
        div()
            .id("artisan-profile-usage-tooltip")
            .debug_selector(|| "artisan-profile-usage-tooltip".to_owned())
            .absolute()
            .left(px(anchor.left + anchor.width + TOOLTIP_OFFSET))
            .top(px(anchor.top))
            .max_w(px(TOOLTIP_WIDTH))
            .rounded(radius)
            .backdrop_blur(glass_blur_radius(GlassStrength::Quiet))
            .bg(glass_foreground_base(theme))
            .border_1()
            .border_color(desktop.line)
            .shadow(glass_card_shadows())
            .child(glass_material_layer(GlassStrength::Quiet, radius))
            .child(glass_highlight_layer(GlassStrength::Quiet, radius))
            .child(
                div()
                    .px(px(12.0))
                    .py(px(8.0))
                    .flex()
                    .flex_row()
                    .items_center()
                    .whitespace_nowrap()
                    .text_size(px(12.0))
                    .line_height(px(16.0))
                    .text_color(desktop.foreground)
                    .child(format!(
                        "You have {}% left.",
                        remaining_percent(window.used_percent)
                    )),
            ),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn window(label: &str, used: f64, resets_at: Option<Timestamp>) -> UsageWindow {
        UsageWindow {
            label: label.to_owned(),
            used_percent: used,
            resets_at,
        }
    }

    #[test]
    fn claude_and_codex_windows_group_like_the_previous_menu() {
        let hour = 3_600_000;
        let usage = UsageLimits {
            windows: vec![
                window("Week (Opus)", 40.0, Some(5 * 24 * hour)),
                window("Session", 3.0, Some(2 * hour + 1)),
                window("Week", 12.0, Some(5 * 24 * hour)),
                window("Cursor models", 50.0, None),
            ],
            read_at: 0,
        };
        let groups = groups(&usage);
        let titles: Vec<_> = groups.iter().map(|(cadence, _)| cadence.title()).collect();
        assert_eq!(titles, ["Session", "Weekly", "Usage"]);
        let weekly: Vec<_> = groups[1]
            .1
            .iter()
            .map(|meter| meter.scope.clone().unwrap_or_else(|| "All models".into()))
            .collect();
        assert_eq!(weekly, ["All models", "Opus"]);
        assert_eq!(reset_duration(&groups[0].1, 0).as_deref(), Some("3 hours"));
        assert_eq!(reset_duration(&groups[1].1, 0).as_deref(), Some("5 days"));
        assert_eq!(reset_duration(&groups[2].1, 0), None);
        assert_eq!(lit_ticks(3.0), 1);
        assert_eq!(remaining_percent(12.4), 88);
        assert_eq!(checked_label(0, 61 * 60_000), "last checked 1 hr ago");
    }
}
