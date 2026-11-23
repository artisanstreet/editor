use crate::ui::hint_card::Phrase;
use artisan_api::{EngineId, SpendLine, Timestamp, UsageCadence, UsageLimits, UsageWindow};

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
    pub gauge: Gauge,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Gauge {
    Meters(Vec<Limit>),
    Spend {
        headline: SpendLine,
        periods: Vec<SpendLine>,
    },
}

#[must_use]
pub fn resolve<'a>(
    reports: impl IntoIterator<Item = (EngineId, &'a UsageLimits)>,
) -> Vec<EngineLimits> {
    reports
        .into_iter()
        .filter_map(|(engine, usage)| {
            let meters = essentials(engine, usage);
            let gauge = if meters.is_empty() {
                let periods = usage.spend.as_ref()?.periods.clone();
                Gauge::Spend {
                    headline: periods.last()?.clone(),
                    periods,
                }
            } else {
                Gauge::Meters(meters)
            };
            Some(EngineLimits { engine, gauge })
        })
        .collect()
}

#[must_use]
pub(crate) fn dollars(cost_usd: f64) -> String {
    format!("${cost_usd:.2}")
}

#[must_use]
pub fn spend_explanation(periods: &[SpendLine]) -> Vec<Phrase> {
    periods
        .iter()
        .enumerate()
        .flat_map(|(index, period)| {
            let lead = if index == 0 { "" } else { " · " };
            [
                Phrase::Muted(format!("{lead}{} ", period.label)),
                Phrase::Strong(dollars(period.cost_usd)),
            ]
        })
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
    let mut pools: Vec<&UsageWindow> = Vec::new();
    for window in readable
        .iter()
        .copied()
        .filter(|_| matches!(engine, EngineId::Claude | EngineId::Antigravity))
        .filter(|window| headline.is_none_or(|headline| headline.scope != window.scope))
    {
        match pools.iter_mut().find(|pool| pool.scope == window.scope) {
            Some(pool) if rank(window.cadence) < rank(pool.cadence) => *pool = window,
            Some(_) => {}
            None => pools.push(window),
        }
    }
    let models = pools.into_iter().filter_map(|window| {
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
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_precision_loss,
        reason = "limit windows span days, far inside f64's exact integer range"
    )]
    let runs_out = start + (elapsed as f64 * 100.0 / limit.used_percent).round() as i64;
    if runs_out < reset {
        Outlook::RunsOut(runs_out.max(now))
    } else {
        Outlook::Lasts(reset)
    }
}

#[must_use]
pub(crate) fn title(engine: EngineId, limit: &Limit) -> String {
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
    let (lead, at) = match outlook {
        Outlook::RunsOut(at) => (" and are on track to run out of usage ", Some(at)),
        Outlook::Lasts(at) => (" and are on track to last until it resets ", Some(at)),
        Outlook::Exhausted(at) => (". It resets ", Some(at)),
        Outlook::Unknown => ("", None),
    };
    [
        Phrase::Muted("You have used up ".to_owned()),
        Phrase::Strong(format!("{percent:.0}%")),
        Phrase::Muted(format!(" of your {allowance}{lead}")),
    ]
    .into_iter()
    .chain(at.map(|at| Phrase::Strong(when(at))))
    .chain([Phrase::Muted(".".to_owned())])
    .collect()
}

const fn cadence_word(cadence: UsageCadence) -> &'static str {
    match cadence {
        UsageCadence::Session => "session",
        UsageCadence::Week => "weekly",
        UsageCadence::Month => "monthly",
        UsageCadence::Other => "",
    }
}
