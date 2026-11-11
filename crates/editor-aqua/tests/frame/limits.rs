use artisan_api::{EngineId, UsageCadence, UsageLimits, UsageWindow};
use editor_aqua::frame::limits::{Limit, Outlook, essentials, explanation, outlook};
use editor_aqua::ui::hint_card::Phrase;

fn window(cadence: UsageCadence, scope: Option<&str>, used_percent: f64) -> UsageWindow {
    UsageWindow {
        label: String::new(),
        cadence,
        scope: scope.map(str::to_owned),
        used_percent,
        resets_at: None,
    }
}

fn usage(windows: Vec<UsageWindow>) -> UsageLimits {
    UsageLimits {
        windows,
        spend: None,
        read_at: 0,
    }
}

fn limit(label: &str, used_percent: f64) -> (String, f64) {
    (label.to_owned(), used_percent)
}

fn shown(engine: EngineId, usage: &UsageLimits) -> Vec<(String, f64)> {
    essentials(engine, usage)
        .into_iter()
        .map(|limit| (limit.label, limit.used_percent))
        .collect()
}

const HOUR: i64 = 3_600_000;
const DAY: i64 = 24 * HOUR;

fn weekly(used_percent: f64, resets_at: Option<i64>) -> Limit {
    Limit {
        label: "Weekly".to_owned(),
        cadence: UsageCadence::Week,
        scope: None,
        used_percent,
        resets_at,
    }
}

fn sentence(limit: &Limit, outlook: Outlook) -> String {
    explanation(limit, outlook, |at| format!("at {at}"))
        .into_iter()
        .map(|phrase| match phrase {
            Phrase::Muted(words) | Phrase::Strong(words) => words,
        })
        .collect()
}

#[test]
fn claude_shows_its_session_and_model_limits_but_not_the_all_models_week() {
    let limits = usage(vec![
        window(UsageCadence::Week, None, 40.0),
        window(UsageCadence::Week, Some("Fable"), 70.0),
        window(UsageCadence::Session, None, 12.0),
    ]);
    assert_eq!(
        shown(EngineId::Claude, &limits),
        vec![limit("Session", 12.0), limit("Fable", 70.0)]
    );
}

#[test]
fn claude_without_a_session_limit_falls_back_to_the_week() {
    let limits = usage(vec![
        window(UsageCadence::Week, Some("Fable"), 70.0),
        window(UsageCadence::Week, None, 40.0),
    ]);
    assert_eq!(
        shown(EngineId::Claude, &limits),
        vec![limit("Weekly", 40.0), limit("Fable", 70.0)]
    );
}

#[test]
fn codex_shows_only_its_shortest_limit() {
    let both = usage(vec![
        window(UsageCadence::Week, Some("codex"), 30.0),
        window(UsageCadence::Session, Some("codex"), 5.0),
    ]);
    assert_eq!(shown(EngineId::Codex, &both), vec![limit("Session", 5.0)]);
    let weekly = usage(vec![window(UsageCadence::Week, Some("codex"), 30.0)]);
    assert_eq!(shown(EngineId::Codex, &weekly), vec![limit("Weekly", 30.0)]);
}

#[test]
fn opencode_shows_its_session_and_unreadable_windows_are_skipped() {
    let limits = usage(vec![
        window(UsageCadence::Session, Some("OpenCode Go"), f64::NAN),
        window(UsageCadence::Month, Some("OpenCode Go"), 2.0),
        window(UsageCadence::Session, Some("OpenCode Go"), 8.0),
    ]);
    assert_eq!(
        shown(EngineId::OpenCode, &limits),
        vec![limit("Session", 8.0)]
    );
}

#[test]
fn a_fast_week_projects_running_out_before_the_reset() {
    let now = 10 * DAY;
    let limit = weekly(50.0, Some(now + 5 * DAY));
    assert_eq!(outlook(&limit, now), Outlook::RunsOut(now + 2 * DAY));
    assert_eq!(
        sentence(&limit, Outlook::RunsOut(now + 2 * DAY)),
        format!(
            "You have used up 50% of your weekly usage and are on track to run out of usage at {}.",
            now + 2 * DAY
        )
    );
}

#[test]
fn a_slow_week_lasts_until_the_reset() {
    let now = 10 * DAY;
    let reset = now + 2 * DAY;
    let limit = weekly(30.0, Some(reset));
    assert_eq!(outlook(&limit, now), Outlook::Lasts(reset));
}

#[test]
fn a_spent_or_unscheduled_limit_says_no_projection() {
    let now = 10 * DAY;
    assert_eq!(
        outlook(&weekly(100.0, Some(now + DAY)), now),
        Outlook::Exhausted(now + DAY)
    );
    assert_eq!(outlook(&weekly(40.0, None), now), Outlook::Unknown);
    assert_eq!(
        outlook(&weekly(40.0, Some(now - HOUR)), now),
        Outlook::Unknown
    );
    assert_eq!(
        outlook(&weekly(0.0, Some(now + DAY)), now),
        Outlook::Lasts(now + DAY)
    );
}

#[test]
fn model_limits_name_their_scope() {
    let fable = Limit {
        label: "Fable".to_owned(),
        scope: Some("Fable".to_owned()),
        ..weekly(49.4, None)
    };
    assert_eq!(
        sentence(&fable, Outlook::Unknown),
        "You have used up 49% of your weekly Fable usage."
    );
}
