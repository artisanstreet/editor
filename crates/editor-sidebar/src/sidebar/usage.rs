use artisan_api::{Timestamp, UsageCadence, UsageLimits, UsageWindow};

pub const METER_SEGMENTS: usize = 14;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Cadence {
    Session,
    Weekly,
    Monthly,
    Unknown,
}
impl Cadence {
    pub const fn title(self) -> &'static str {
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
    pub index: usize,
    pub cadence: Cadence,
    pub scope: Option<String>,
    pub used_percent: f64,
    pub resets_at: Option<Timestamp>,
}
pub fn meter(index: usize, window: &UsageWindow) -> Meter {
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
pub fn groups(usage: &UsageLimits) -> Vec<(Cadence, Vec<Meter>)> {
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
pub fn reset_duration(meters: &[Meter], now: Timestamp) -> Option<String> {
    let mut latest: Option<Timestamp> = None;
    for meter in meters {
        let at = meter.resets_at.filter(|at| *at > now)?;
        latest = Some(latest.map_or(at, |latest| latest.max(at)));
    }
    reset_remaining(latest?, now)
}
pub fn reset_remaining(at: Timestamp, now: Timestamp) -> Option<String> {
    const MINUTE: i64 = 60_000;
    if at <= now {
        return None;
    }
    let minutes = (at.saturating_sub(now).saturating_add(MINUTE - 1) / MINUTE).max(1);
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
pub fn lit_of(used_percent: f64, segments: usize) -> usize {
    if !used_percent.is_finite() || used_percent <= 0.0 {
        return 0;
    }
    if used_percent >= 100.0 {
        return segments;
    }
    let nearest = (used_percent / 100.0 * segments as f64).round() as usize;
    nearest.clamp(1, segments - 1)
}
pub fn remaining_percent(used_percent: f64) -> i64 {
    (100.0 - used_percent.round()).clamp(0.0, 100.0) as i64
}
