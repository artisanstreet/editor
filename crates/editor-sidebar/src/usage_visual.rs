//! The profile menu's Usage section: one card, a row per engine with its
//! mark and the meter of its headline limit (the session, else the shortest
//! cadence it reports). Hovering a row says how much is used and when it
//! resets. The cadence grouping is shared with the Usage page.

use crate::usage::Account;
use artisan_api::{EngineId, Timestamp, UsageLimits, UsageWindow};
use editor_ui::glass::card_shadows;
use editor_ui::hover_card::glass_tooltip;
use editor_ui::icon::{IconSize, IconStyle, IconTint, icon};
use editor_ui::measure::Measured;
use editor_ui::picker_motion::HoverRect;
use editor_ui::theme::{ArtisanTheme, ColorTokens, Opacity};
use gpui::prelude::FluentBuilder as _;
use gpui::{
    Context, Div, HighlightStyle, Hsla, InteractiveElement as _, ParentElement as _, SharedString,
    Stateful, StatefulInteractiveElement as _, Styled as _, StyledText, div, px,
};

/// Ticks in a meter, as the previous Editor quantized it.
pub const METER_SEGMENTS: usize = 14;
/// Ticks in a menu row's meter, which spans the card.
const ROW_SEGMENTS: usize = 48;
/// The tooltip's width cap and its distance from the meter row.
const TOOLTIP_WIDTH: f32 = 224.0;
const TOOLTIP_OFFSET: f32 = 8.0;

/// The provider-disclosed cadence a window belongs to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Cadence {
    Session,
    Weekly,
    Monthly,
    Unknown,
}

impl Cadence {
    pub(crate) const fn title(self) -> &'static str {
        match self {
            Self::Session => "Session",
            Self::Weekly => "Weekly",
            Self::Monthly => "Monthly",
            Self::Unknown => "Usage",
        }
    }
}

/// A window as the menu shows it: its cadence and the scope it covers
/// (`None` reads "All models").
#[derive(Clone, Debug, PartialEq)]
pub struct Meter {
    pub(crate) index: usize,
    pub(crate) cadence: Cadence,
    pub(crate) scope: Option<String>,
    pub(crate) used_percent: f64,
    pub(crate) resets_at: Option<Timestamp>,
}

/// Reads Forge's window label: `Session`, `Week`, `Week (Opus)`,
/// `GPT-5 (Session)`, or a scope with no cadence (`Cursor models`).
pub fn meter(index: usize, window: &UsageWindow) -> Meter {
    let cadence = match window.cadence {
        artisan_api::UsageCadence::Session => Cadence::Session,
        artisan_api::UsageCadence::Week => Cadence::Weekly,
        artisan_api::UsageCadence::Month => Cadence::Monthly,
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
pub fn groups(usage: &UsageLimits) -> Vec<(Cadence, Vec<Meter>)> {
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
pub fn reset_duration(meters: &[Meter], now: Timestamp) -> Option<String> {
    let mut latest: Option<Timestamp> = None;
    for meter in meters {
        let at = meter.resets_at.filter(|at| *at > now)?;
        latest = Some(latest.map_or(at, |latest| latest.max(at)));
    }
    reset_remaining(latest?, now)
}

fn reset_remaining(at: Timestamp, now: Timestamp) -> Option<String> {
    const MINUTE: i64 = 60_000;
    if at <= now {
        return None;
    }
    let minutes = at.saturating_sub(now).saturating_add(MINUTE - 1) / MINUTE;
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
pub fn checked_label(read_at: Timestamp, now: Timestamp) -> String {
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
pub fn lit_ticks(used_percent: f64) -> usize {
    lit_of(used_percent, METER_SEGMENTS)
}

#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss,
    reason = "percentages and tick counts stay within 0..=100"
)]
fn lit_of(used_percent: f64, segments: usize) -> usize {
    if !used_percent.is_finite() || used_percent <= 0.0 {
        return 0;
    }
    if used_percent >= 100.0 {
        return segments;
    }
    let nearest = (used_percent / 100.0 * segments as f64).round() as usize;
    nearest.clamp(1, segments - 1)
}

/// What the tooltip says is left.
#[expect(
    clippy::cast_possible_truncation,
    reason = "the value is clamped to 0..=100 before the cast"
)]
pub fn remaining_percent(used_percent: f64) -> i64 {
    (100.0 - used_percent.round()).clamp(0.0, 100.0) as i64
}

/// The meter accent.
pub fn engine_accent(engine: EngineId, theme: &ArtisanTheme) -> Hsla {
    editor_ui::engine_accent(theme, engine).to_paint()
}

/// The product name the menu titles a block with.
pub const fn engine_name(engine: EngineId) -> &'static str {
    engine.display_name()
}

/// One meter: its engine and its window's place in the report.
type MeterKey = (EngineId, usize);

/// The Usage section's pointer state.
#[derive(Default)]
pub struct UsageUi {
    /// The hovered meter, and its row's and the menu's measured places.
    pub(crate) meter_hover: Option<MeterKey>,
    pub(crate) tip_anchor: Measured,
    pub(crate) menu_bounds: Measured,
}

impl UsageUi {
    pub(crate) fn clear(&mut self) {
        self.meter_hover = None;
        self.tip_anchor.clear();
    }
}

/// The whole section, one card: a row per engine with its mark and the
/// meter of the limit that binds first.
pub fn section(
    sidebar: &Account,
    engines: &[(EngineId, UsageLimits)],
    theme: &ArtisanTheme,
    cx: &Context<Account>,
) -> Stateful<Div> {
    let mut section = div()
        .id("artisan-profile-usage")
        .debug_selector(|| "artisan-profile-usage".to_owned())
        .flex()
        .flex_col()
        .gap(px(8.0))
        .px(px(12.0))
        .py(px(8.0));
    for (engine, usage) in engines {
        if let Some((index, window)) = headline(usage) {
            section = section.child(engine_row(sidebar, *engine, index, window, theme, cx));
        }
    }
    section
}

/// The window the menu shows: the account-wide session, else the shortest
/// cadence the engine reports (Codex's week without a session).
pub fn headline(usage: &UsageLimits) -> Option<(usize, &UsageWindow)> {
    usage
        .windows
        .iter()
        .enumerate()
        .filter(|(_, window)| (0.0..=100.0).contains(&window.used_percent))
        .min_by_key(|(index, window)| {
            (
                meter(*index, window).cadence,
                window.scope.is_some(),
                *index,
            )
        })
}

/// The engine's mark, then its meter across the card. Hovering says how
/// much is used and when it resets.
fn engine_row(
    sidebar: &Account,
    engine: EngineId,
    index: usize,
    window: &UsageWindow,
    theme: &ArtisanTheme,
    cx: &Context<Account>,
) -> Stateful<Div> {
    let desktop = theme.colors;
    let accent = engine_accent(engine, theme);
    let dim = desktop.text.at(Opacity::MeterTrack).to_paint();
    let lit = lit_of(window.used_percent, ROW_SEGMENTS);
    let key = (engine, index);
    let selector = format!("artisan-profile-usage-meter-{engine}");
    let hovered = sidebar.ui.meter_hover == Some(key);
    div()
        .id(SharedString::from(selector.clone()))
        .debug_selector(move || selector)
        .relative()
        .h(px(24.0))
        .flex()
        .items_center()
        .gap(px(12.0))
        .aria_label(format!("{} usage", engine.display_name()))
        .when(hovered, |row| row.child(sidebar.ui.tip_anchor.track()))
        .child(
            div()
                .size(px(16.0))
                .flex()
                .items_center()
                .justify_center()
                .flex_shrink_0()
                .text_color(desktop.text.to_paint())
                .child(
                    icon(IconStyle::resolve(
                        *theme,
                        editor_ui::engine_icon(theme, engine),
                        IconSize::Default,
                        IconTint::Inherit,
                    ))
                    .w(px(if engine == EngineId::OpenCode {
                        12.8
                    } else {
                        16.0
                    }))
                    .h(px(16.0))
                    .flex_shrink_0(),
                ),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .h(px(16.0))
                .flex()
                .gap(px(1.5))
                .children((0..ROW_SEGMENTS).map(|tick| {
                    div()
                        .flex_1()
                        .h_full()
                        .bg(if tick < lit { accent } else { dim })
                })),
        )
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

/// "12% used of your session limit. Resets in 3 hours."
pub fn tooltip_text(window: &UsageWindow, now: Timestamp) -> String {
    let limit = match window.cadence {
        artisan_api::UsageCadence::Session => "session",
        artisan_api::UsageCadence::Week => "weekly",
        artisan_api::UsageCadence::Month => "monthly",
        artisan_api::UsageCadence::Other => "usage",
    };
    let used = 100 - remaining_percent(window.used_percent);
    let mut text = format!("{used}% used of your {limit} limit.");
    if let Some(at) = window.resets_at {
        if let Some(duration) = reset_remaining(at, now) {
            text.push_str(" Resets in ");
            text.push_str(&duration);
            text.push('.');
        } else {
            text.push_str(" Reset time reached.");
        }
    }
    text
}

/// The glass tooltip beside the hovered row.
pub fn tooltip(
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
    let text = tooltip_text(window, artisan_api::now());
    let numbers: Vec<_> = text
        .match_indices(|ch: char| ch.is_ascii_digit() || ch == '%')
        .map(|(start, digit)| {
            (
                start..start + digit.len(),
                HighlightStyle {
                    color: Some(desktop.text.to_paint()),
                    ..HighlightStyle::default()
                },
            )
        })
        .collect();
    Some(
        glass_tooltip(
            "artisan-profile-usage-tooltip",
            theme,
            px(TOOLTIP_WIDTH),
            false,
        )
        .debug_selector(|| "artisan-profile-usage-tooltip".to_owned())
        .absolute()
        .left(px(anchor.left + anchor.width + TOOLTIP_OFFSET))
        .top(px(anchor.top))
        .shadow(card_shadows(theme))
        .child(StyledText::new(text).with_highlights(numbers)),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use artisan_api::UsageCadence;

    fn window(cadence: UsageCadence, scope: Option<&str>, used_percent: f64) -> UsageWindow {
        UsageWindow {
            label: String::new(),
            cadence,
            scope: scope.map(str::to_owned),
            used_percent,
            resets_at: None,
        }
    }

    fn limits(windows: Vec<UsageWindow>) -> UsageLimits {
        UsageLimits {
            windows,
            spend: None,
            read_at: 0,
        }
    }

    /// Claude's row meters its session, Codex (a week only) its week, and
    /// the Go plan its rolling five hours over its week and month.
    #[test]
    fn the_menu_meters_the_session_else_the_shortest_cadence() {
        let claude = limits(vec![
            window(UsageCadence::Week, Some("Fable"), 0.0),
            window(UsageCadence::Week, None, 4.0),
            window(UsageCadence::Session, None, 3.0),
        ]);
        let codex = limits(vec![window(UsageCadence::Week, None, 70.0)]);
        let opencode = limits(vec![
            window(UsageCadence::Month, Some("OpenCode Go"), 25.0),
            window(UsageCadence::Session, Some("OpenCode Go"), 0.0),
        ]);
        assert_eq!(headline(&claude).map(|(index, _)| index), Some(2));
        assert_eq!(headline(&codex).map(|(index, _)| index), Some(0));
        assert_eq!(headline(&opencode).map(|(index, _)| index), Some(1));
        assert_eq!(
            tooltip_text(&codex.windows[0], 0),
            "70% used of your weekly limit."
        );
    }

    #[test]
    fn tooltip_reset_times_are_relative_and_round_up() {
        const NOW: Timestamp = 1_700_000_000_000;
        let mut window = window(UsageCadence::Session, None, 12.0);
        for (remaining, expected) in [
            (1, "Resets in 1 minute."),
            (60_000, "Resets in 1 minute."),
            (60_001, "Resets in 2 minutes."),
            (3_600_000, "Resets in 1 hour."),
            (2 * 3_600_000 + 1, "Resets in 3 hours."),
            (86_400_000, "Resets in 1 day."),
            (3 * 86_400_000 + 1, "Resets in 4 days."),
            (0, "Reset time reached."),
            (-1, "Reset time reached."),
        ] {
            window.resets_at = Some(NOW + remaining);
            assert_eq!(
                tooltip_text(&window, NOW),
                format!("12% used of your session limit. {expected}")
            );
        }
        window.resets_at = None;
        assert_eq!(
            tooltip_text(&window, NOW),
            "12% used of your session limit."
        );
    }
}
