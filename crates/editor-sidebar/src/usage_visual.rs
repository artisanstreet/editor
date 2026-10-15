//! The profile menu's Usage section, ported from the previous Editor's
//! `desktop_profile_usage` (`sidebar-engine-usage.svelte`): per engine its
//! mark and name with a "last checked" reading that turns into Refresh on
//! hover, one group per cadence with 72×8 accent meters, and the sentence
//! saying when the group's limit resets.

use crate::usage::Account;
use artisan_api::{EngineId, Timestamp, UsageLimits, UsageWindow};
use editor_ui::AssetId;
use editor_ui::fade_arc::FadeArc;
use editor_ui::glass::{
    GlassStrength, glass_blur_radius, glass_card_shadows, glass_foreground_base,
    glass_highlight_layer, glass_material_layer,
};
use editor_ui::icon::{IconSize, IconStyle, IconTint, icon};
use editor_ui::measure::Measured;
use editor_ui::motion::{MotionCurve, MotionDuration};
use editor_ui::picker_motion::HoverRect;
use editor_ui::theme::{
    ArtisanTheme, ColorTokens, RadiusStep, RadiusTokens, TextStep, TypographyTokens,
};
use gpui::prelude::FluentBuilder as _;
use gpui::{
    Context, Div, FontWeight, HighlightStyle, Hsla, InteractiveElement as _, ParentElement as _,
    SharedString, Stateful, StatefulInteractiveElement as _, Styled as _, StyledText, canvas, div,
    px,
};
use std::cell::RefCell;
use std::collections::HashMap;
use std::time::Instant;

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
    Unknown,
}

impl Cadence {
    const fn title(self) -> &'static str {
        match self {
            Self::Session => "Session",
            Self::Weekly => "Weekly",
            Self::Unknown => "Usage",
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
    let cadence = match window.cadence {
        artisan_api::UsageCadence::Session => Cadence::Session,
        artisan_api::UsageCadence::Week => Cadence::Weekly,
        artisan_api::UsageCadence::Other => Cadence::Unknown,
    };
    let scope = window.scope.clone();
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
    let minutes = latest?.saturating_sub(now).saturating_add(MINUTE - 1) / MINUTE;
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
    let minutes = (now.saturating_sub(read_at) / 60_000).max(0);
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

/// Lit ticks: the used share to the nearest tick, with any use lighting
/// one and only a spent limit lighting all. Rounding up (master's way) lit
/// every tick from 93 % on, so "5 % left" read as nothing left.
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss
)]
pub(crate) fn lit_ticks(used_percent: f64) -> usize {
    if !used_percent.is_finite() || used_percent <= 0.0 {
        return 0;
    }
    if used_percent >= 100.0 {
        return METER_SEGMENTS;
    }
    let nearest = (used_percent / 100.0 * METER_SEGMENTS as f64).round() as usize;
    nearest.clamp(1, METER_SEGMENTS - 1)
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

/// The Usage section's pointer state.
#[derive(Default)]
pub(crate) struct UsageUi {
    /// The engine whose reading the pointer is on.
    pub(crate) refresh_hover: Option<EngineId>,
    /// Each engine's reading/Refresh/spinner swap, mid-flight or at rest.
    swaps: RefCell<HashMap<EngineId, RefreshSwap>>,
    /// The hovered meter, and its row's and the menu's measured places.
    pub(crate) meter_hover: Option<MeterKey>,
    pub(crate) tip_anchor: Measured,
    pub(crate) menu_bounds: Measured,
}

impl UsageUi {
    pub(crate) fn clear(&mut self) {
        self.refresh_hover = None;
        self.meter_hover = None;
        self.tip_anchor.clear();
    }
}

/// The whole section: engine blocks separated by hairlines.
pub(crate) fn section(
    sidebar: &Account,
    engines: &[(EngineId, UsageLimits)],
    theme: &ArtisanTheme,
    desktop: &ColorTokens,
    separator: Hsla,
    cx: &mut Context<Account>,
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
    sidebar: &Account,
    engine: EngineId,
    usage: &UsageLimits,
    now: Timestamp,
    theme: &ArtisanTheme,
    desktop: &ColorTokens,
    cx: &mut Context<Account>,
) -> Div {
    let accent = engine_accent(engine);
    let dim = theme.colors.foreground.with_alpha(0.11).to_paint();
    let refreshing = sidebar.refreshing.contains_key(&engine);
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
                .text_color(desktop.workspace_foreground.to_paint())
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
                        .text_size(TypographyTokens::size(TextStep::Xs))
                        .line_height(px(16.0))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(desktop.workspace_foreground.to_paint())
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
                .text_size(TypographyTokens::size(TextStep::Xs))
                .line_height(px(16.0))
                .font_weight(FontWeight::MEDIUM)
                .text_color(desktop.workspace_foreground.to_paint())
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
                            color: Some(desktop.workspace_foreground.to_paint()),
                            ..Default::default()
                        },
                    )]),
                None => StyledText::new(SharedString::from(sentence)),
            };
            group = group.child(
                div()
                    .mt(px(8.0))
                    .text_size(TypographyTokens::size(TextStep::Xs))
                    .line_height(px(16.0))
                    .text_color(desktop.workspace_secondary.to_paint())
                    .child(body),
            );
        }
        block = block.child(group);
    }
    block
}

/// The swap of one refresh control (master's `RefreshSwap`): the reading
/// at rest, Refresh under the pointer, the spinner while checking. Each of
/// the three fades, unblurs and slides 4 px into place on the 150 ms
/// ease-in-out text-swap clock, and a change mid-flight starts from what is
/// on screen, so a reversal never jumps.
#[derive(Clone, Copy, Debug)]
struct RefreshSwap {
    from: [f32; 3],
    to: [f32; 3],
    off_from: [f32; 3],
    off_to: [f32; 3],
    started: Instant,
}

impl RefreshSwap {
    fn resting() -> Self {
        let rest = [1.0, 0.0, 0.0];
        Self {
            from: rest,
            to: rest,
            off_from: swap_offsets(rest),
            off_to: swap_offsets(rest),
            started: Instant::now(),
        }
    }

    /// Opacity and paint offset of each layer now, and whether it moves.
    fn at(&self, now: Instant) -> ([f32; 3], [f32; 3], bool) {
        let total = MotionDuration::Quick.as_duration().as_secs_f32();
        let progress = (now.saturating_duration_since(self.started).as_secs_f32() / total).min(1.0);
        #[allow(clippy::cast_possible_truncation)]
        let eased = MotionCurve::EaseInOut.sample(f64::from(progress)) as f32;
        let mix = |from: [f32; 3], to: [f32; 3]| {
            std::array::from_fn(|index| from[index] + (to[index] - from[index]) * eased)
        };
        (
            mix(self.from, self.to),
            mix(self.off_from, self.off_to),
            progress < 1.0,
        )
    }

    #[expect(
        clippy::float_cmp,
        reason = "targets are the exact 0/1 endpoints; an unchanged one must not restart the clock"
    )]
    fn retarget(&mut self, to: [f32; 3], reduce_motion: bool, now: Instant) {
        if reduce_motion {
            *self = Self {
                from: to,
                to,
                off_from: swap_offsets(to),
                off_to: swap_offsets(to),
                started: now,
            };
            return;
        }
        if self.to == to {
            return;
        }
        let (shown, offsets, _) = self.at(now);
        *self = Self {
            from: shown,
            to,
            off_from: offsets,
            off_to: swap_offsets(to),
            started: now,
        };
    }
}

/// Where each layer rests: shown at zero, the reading leaving upward, the
/// action and spinner waiting below.
fn swap_offsets(to: [f32; 3]) -> [f32; 3] {
    [
        if to[0] >= 1.0 { 0.0 } else { -4.0 },
        if to[1] >= 1.0 { 0.0 } else { 4.0 },
        if to[2] >= 1.0 { 0.0 } else { 4.0 },
    ]
}

/// The reading beside the name: "last checked …" at rest, Refresh under
/// the pointer, a spinner while the refresh runs (master's
/// `desktop_profile_usage_refresh`). All three share one cell sized to the
/// widest, so the swap never moves the row.
fn refresh_control(
    sidebar: &Account,
    engine: EngineId,
    checked: &str,
    refreshing: bool,
    theme: &ArtisanTheme,
    desktop: &ColorTokens,
    cx: &mut Context<Account>,
) -> Stateful<Div> {
    let selector = format!("artisan-profile-usage-refresh-{engine}");
    let hovered = sidebar.ui.refresh_hover == Some(engine);
    let target = if refreshing {
        [0.0, 0.0, 1.0]
    } else if hovered {
        [0.0, 1.0, 0.0]
    } else {
        [1.0, 0.0, 0.0]
    };
    let now = Instant::now();
    let (shown, offsets, moving) = {
        let mut swaps = sidebar.ui.swaps.borrow_mut();
        let swap = swaps.entry(engine).or_insert_with(RefreshSwap::resting);
        swap.retarget(target, cx.reduce_motion(), now);
        swap.at(now)
    };
    let layer = |index: usize| {
        div()
            .col_start(1)
            .row_start(1)
            .relative()
            .top(px(offsets[index]))
            .flex()
            .items_center()
            .justify_end()
            .whitespace_nowrap()
            .opacity(shown[index])
            .blur(px((1.0 - shown[index]) * 2.0))
    };
    let mut control = div()
        .id(SharedString::from(selector.clone()))
        .debug_selector(move || selector.clone())
        .grid()
        .flex_shrink_0()
        .text_size(TypographyTokens::size(TextStep::Xs))
        .line_height(px(16.0))
        .child(
            layer(0)
                .text_color(desktop.workspace_secondary.to_paint())
                .child(checked.to_owned()),
        )
        .child(
            layer(1)
                .text_color(desktop.workspace_foreground.to_paint())
                .child("Refresh"),
        )
        .child(
            layer(2).child(
                FadeArc::new(
                    SharedString::from(format!("usage-refresh-{engine}")),
                    *theme,
                )
                .size(px(14.0)),
            ),
        )
        .on_hover(cx.listener(move |sidebar, hovered: &bool, _, cx| {
            if *hovered {
                sidebar.ui.refresh_hover = Some(engine);
            } else if sidebar.ui.refresh_hover == Some(engine) {
                sidebar.ui.refresh_hover = None;
            }
            cx.notify();
        }));
    if moving {
        // Frames keep coming while the swap moves.
        let sidebar = cx.entity().downgrade();
        control = control.child(
            canvas(
                |_, _, _| {},
                move |_, (), window, cx| {
                    window.request_animation_frame();
                    let _ = sidebar.update(cx, |_, cx| cx.notify());
                },
            )
            .absolute()
            .size_0(),
        );
    }
    if !refreshing {
        control = control
            .role(gpui::Role::Button)
            .aria_label(format!("Refresh {} usage", engine.display_name()))
            .focusable()
            .tab_index(0)
            .cursor_pointer()
            .on_click(cx.listener(move |sidebar, _, _, cx| {
                cx.stop_propagation();
                sidebar.refresh(engine, cx);
            }));
    }
    control
}

/// One meter row: the scope, then fourteen ticks lit by the used share.
/// Hovering shows what is left in the glass tooltip.
#[allow(clippy::too_many_arguments)]
fn meter_row(
    sidebar: &Account,
    engine: EngineId,
    meter: &Meter,
    accent: Hsla,
    dim: Hsla,
    desktop: &ColorTokens,
    cx: &mut Context<Account>,
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
    let hovered = sidebar.ui.meter_hover == Some(key);
    div()
        .id(SharedString::from(selector.clone()))
        .debug_selector(move || selector.clone())
        .relative()
        .flex()
        .items_center()
        .gap(px(16.0))
        .when(hovered, |row| row.child(sidebar.ui.tip_anchor.track()))
        .child(
            div()
                .min_w(px(0.0))
                .flex_1()
                .truncate()
                .pl(px(8.0))
                .text_size(TypographyTokens::size(TextStep::Xs))
                .line_height(px(16.0))
                .text_color(desktop.workspace_secondary.to_paint())
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
                sidebar.ui.meter_hover = Some(key);
            } else if sidebar.ui.meter_hover == Some(key) {
                sidebar.ui.meter_hover = None;
                sidebar.ui.tip_anchor.clear();
            }
            cx.notify();
        }))
}

/// The glass tooltip beside the hovered meter: "You have N% left."
pub(crate) fn tooltip(
    sidebar: &Account,
    engines: &[(EngineId, UsageLimits)],
    theme: &ArtisanTheme,
    desktop: &ColorTokens,
) -> Option<Stateful<Div>> {
    let key = sidebar.ui.meter_hover?;
    let anchor = HoverRect::within(sidebar.ui.tip_anchor.get()?, sidebar.ui.menu_bounds.get()?);
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
            .border_color(desktop.line.to_paint())
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
                    .text_size(TypographyTokens::size(TextStep::Xs))
                    .line_height(px(16.0))
                    .text_color(desktop.workspace_foreground.to_paint())
                    .child(format!(
                        "You have {}% left.",
                        remaining_percent(window.used_percent)
                    )),
            ),
    )
}
