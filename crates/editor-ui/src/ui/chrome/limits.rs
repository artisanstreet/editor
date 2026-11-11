use crate::ui::hint_card::Phrase;
use artisan_api::{EngineId, Timestamp, UsageCadence, UsageLimits, UsageWindow};

const HOUR_MS: i64 = 3_600_000;
const DAY_MS: i64 = 24 * HOUR_MS;

#[derive(Clone, Debug, PartialEq)]
pub struct Limit {
    pub label: String,
    pub cadence: UsageCadence,
    pub scope: Option<String>,
    pub used_percent: f64,
    pub resets_at: Option<Timestamp>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outlook {
    RunsOut(Timestamp),
    Lasts(Timestamp),
    Exhausted(Timestamp),
    Unknown,
}

#[derive(Clone, Debug, PartialEq)]
pub struct EngineLimits {
    pub engine: EngineId,
    pub limits: Vec<Limit>,
}

#[must_use]
pub fn resolve<'a>(
    reports: impl IntoIterator<Item = (EngineId, &'a UsageLimits)>,
) -> Vec<EngineLimits> {
    reports
        .into_iter()
        .map(|(engine, usage)| EngineLimits {
            engine,
            limits: essentials(engine, usage),
        })
        .filter(|engine| !engine.limits.is_empty())
        .collect()
}

#[must_use]
pub fn essentials(engine: EngineId, usage: &UsageLimits) -> Vec<Limit> {
    let readable: Vec<&UsageWindow> = usage
        .windows
        .iter()
        .filter(|window| (0.0..=100.0).contains(&window.used_percent))
        .collect();
    let headline = readable
        .iter()
        .enumerate()
        .min_by_key(|(index, window)| (rank(window.cadence), window.scope.is_some(), *index))
        .map(|(_, window)| *window);
    let models = readable
        .iter()
        .copied()
        .filter(|_| engine == EngineId::Claude)
        .filter(|window| headline.is_none_or(|headline| !std::ptr::eq(*window, headline)))
        .filter_map(|window| {
            window.scope.clone().map(|label| Limit {
                label,
                ..limit(window)
            })
        });
    headline
        .into_iter()
        .map(|window| Limit {
            label: cadence_label(window.cadence).to_owned(),
            ..limit(window)
        })
        .chain(models)
        .collect()
}

const fn rank(cadence: UsageCadence) -> u8 {
    match cadence {
        UsageCadence::Session => 0,
        UsageCadence::Week => 1,
        UsageCadence::Month => 2,
        UsageCadence::Other => 3,
    }
}

const fn cadence_label(cadence: UsageCadence) -> &'static str {
    match cadence {
        UsageCadence::Session => "Session",
        UsageCadence::Week => "Weekly",
        UsageCadence::Month => "Monthly",
        UsageCadence::Other => "Usage",
    }
}

fn limit(window: &UsageWindow) -> Limit {
    Limit {
        label: String::new(),
        cadence: window.cadence,
        scope: window.scope.clone(),
        used_percent: window.used_percent,
        resets_at: window.resets_at,
    }
}

const fn span(cadence: UsageCadence) -> Option<i64> {
    match cadence {
        UsageCadence::Session => Some(5 * HOUR_MS),
        UsageCadence::Week => Some(7 * DAY_MS),
        UsageCadence::Month => Some(30 * DAY_MS),
        UsageCadence::Other => None,
    }
}

#[must_use]
pub fn outlook(limit: &Limit, now: Timestamp) -> Outlook {
    let Some(reset) = limit.resets_at.filter(|reset| *reset > now) else {
        return Outlook::Unknown;
    };
    if limit.used_percent >= 100.0 {
        return Outlook::Exhausted(reset);
    }
    let Some(span) = span(limit.cadence) else {
        return Outlook::Lasts(reset);
    };
    let start = reset - span;
    let elapsed = now - start;
    if elapsed <= 0 || limit.used_percent <= 0.0 {
        return Outlook::Lasts(reset);
    }
    let runs_out = start + (elapsed as f64 * 100.0 / limit.used_percent).round() as i64;
    if runs_out < reset {
        Outlook::RunsOut(runs_out.max(now))
    } else {
        Outlook::Lasts(reset)
    }
}

#[must_use]
pub fn title(engine: EngineId, limit: &Limit) -> String {
    format!("{} usage for {}", limit.label, engine.display_name())
}

#[must_use]
pub fn explanation(
    limit: &Limit,
    outlook: Outlook,
    when: impl Fn(Timestamp) -> String,
) -> Vec<Phrase> {
    let allowance = match (limit.cadence, &limit.scope) {
        (UsageCadence::Other, None) => "usage".to_owned(),
        (UsageCadence::Other, Some(scope)) => format!("{scope} usage"),
        (cadence, None) => format!("{} usage", cadence_word(cadence)),
        (cadence, Some(scope)) => format!("{} {scope} usage", cadence_word(cadence)),
    };
    let percent = limit.used_percent.round().clamp(0.0, 100.0);
    let outcome = match outlook {
        Outlook::RunsOut(at) => format!(" and are on track to run out of usage {}.", when(at)),
        Outlook::Lasts(at) => format!(" and are on track to last until it resets {}.", when(at)),
        Outlook::Exhausted(at) => format!(". It resets {}.", when(at)),
        Outlook::Unknown => ".".to_owned(),
    };
    vec![
        Phrase::Muted("You have used up ".to_owned()),
        Phrase::Strong(format!("{percent:.0}%")),
        Phrase::Muted(format!(" of your {allowance}{outcome}")),
    ]
}

const fn cadence_word(cadence: UsageCadence) -> &'static str {
    match cadence {
        UsageCadence::Session => "session",
        UsageCadence::Week => "weekly",
        UsageCadence::Month => "monthly",
        UsageCadence::Other => "",
    }
}
