pub mod controller;
pub mod view;

use artisan_api::{Timestamp, UsageCadence, UsageLimits, UsageWindow};

pub(crate) const METER_SEGMENTS: u8 = 14;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Cadence {
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
}
#[derive(Clone, Debug, PartialEq)]
pub struct Meter {
    index: usize,
    cadence: Cadence,
    scope: Option<String>,
    used_percent: f64,
    resets_at: Option<Timestamp>,
}
fn meter(index: usize, window: &UsageWindow) -> Meter {
    Meter {
        index,
        cadence: match window.cadence {
            UsageCadence::Session => Cadence::Session,
            UsageCadence::Week => Cadence::Weekly,
            UsageCadence::Month => Cadence::Monthly,
            UsageCadence::Other => Cadence::Unknown,
        },
        scope: window.scope.clone(),
        used_percent: window.used_percent,
        resets_at: window.resets_at,
    }
}
pub(crate) fn groups(usage: &UsageLimits) -> Vec<(Cadence, Vec<Meter>)> {
    let mut meters: Vec<_> = usage
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
pub(crate) fn reset_duration(meters: &[Meter], now: Timestamp) -> Option<String> {
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
    let minutes = (at.saturating_sub(now).saturating_add(MINUTE - 1) / MINUTE).max(1);
    let (amount, unit) = if minutes < 60 {
        (minutes, ("minute", "minutes"))
    } else if minutes < 24 * 60 {
        ((minutes + 59) / 60, ("hour", "hours"))
    } else {
        ((minutes + 24 * 60 - 1) / (24 * 60), ("day", "days"))
    };
    Some(editor_ui::locale::count(amount, unit.0, unit.1))
}
pub(crate) fn checked_label(read_at: Timestamp, now: Timestamp) -> String {
    let minutes = (now.saturating_sub(read_at) / 60_000).max(0);
    if minutes < 1 {
        return "last checked now".to_owned();
    }
    if minutes < 60 {
        return format!(
            "last checked {} min ago",
            editor_ui::locale::integer(minutes)
        );
    }
    let hours = minutes / 60;
    if hours < 24 {
        return format!("last checked {} hr ago", editor_ui::locale::integer(hours));
    }
    format!(
        "last checked {} d ago",
        editor_ui::locale::integer(hours / 24)
    )
}
pub(crate) fn lit_of(used_percent: f64, segments: u8) -> u8 {
    if !used_percent.is_finite() || used_percent <= 0.0 {
        return 0;
    }
    if used_percent >= 100.0 {
        return segments;
    }
    let scaled = used_percent / 100.0 * f64::from(segments);
    (2..segments)
        .take_while(|&count| f64::from(count) - 0.5 <= scaled)
        .last()
        .unwrap_or(1)
}
pub(crate) fn remaining_percent(used_percent: f64) -> u8 {
    (0..=100_u8)
        .rev()
        .find(|&left| f64::from(left) <= 100.0 - used_percent.round())
        .unwrap_or(0)
}
