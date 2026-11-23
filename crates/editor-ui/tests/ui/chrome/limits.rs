use artisan_api::{EngineId, SpendLine, UsageCadence, UsageLimits, UsageSpend, UsageWindow};
use editor_ui::chrome::limits::{Gauge, resolve, spend_explanation};
use editor_ui::hint_card::Phrase;

fn line(label: &str, cost_usd: f64) -> SpendLine {
    SpendLine {
        label: label.into(),
        cost_usd,
        tokens: 0,
    }
}

fn spend() -> UsageSpend {
    UsageSpend {
        periods: vec![
            line("Today", 0.0),
            line("Last 7 days", 3.2),
            line("Last 30 days", 16.884),
        ],
        models: Vec::new(),
    }
}

#[test]
fn an_engine_with_only_spend_shows_its_longest_period_and_one_with_limits_keeps_its_meters() {
    let limited = UsageLimits {
        windows: vec![UsageWindow {
            label: "5 hours".into(),
            cadence: UsageCadence::Session,
            scope: Some("Claude and GPT models".into()),
            used_percent: 40.0,
            resets_at: None,
        }],
        spend: Some(Box::new(spend())),
        read_at: 0,
    };
    let spend_only = UsageLimits {
        windows: Vec::new(),
        spend: Some(Box::new(spend())),
        read_at: 0,
    };
    let nothing = UsageLimits {
        windows: Vec::new(),
        spend: Some(Box::new(UsageSpend::default())),
        read_at: 0,
    };
    let resolved = resolve([
        (EngineId::Antigravity, &limited),
        (EngineId::OpenCode, &spend_only),
        (EngineId::Codex, &nothing),
    ]);
    assert_eq!(
        resolved
            .iter()
            .map(|engine| engine.engine)
            .collect::<Vec<_>>(),
        [EngineId::Antigravity, EngineId::OpenCode],
        "an engine with neither limits nor spend takes no row"
    );
    assert!(
        matches!(&resolved[0].gauge, Gauge::Meters(meters) if meters.len() == 1 && meters[0].label == "Session")
    );
    assert!(matches!(
        &resolved[1].gauge,
        Gauge::Spend { headline, periods } if headline.label == "Last 30 days" && periods.len() == 3
    ));
}

#[test]
fn spend_lists_every_period_with_its_cost() {
    let text: String = spend_explanation(&spend().periods)
        .into_iter()
        .map(|phrase| match phrase {
            Phrase::Muted(text) | Phrase::Strong(text) => text,
        })
        .collect();
    assert_eq!(
        text,
        "Today $0.00 · Last 7 days $3.20 · Last 30 days $16.88"
    );
}

fn window(cadence: UsageCadence, scope: Option<&str>, used_percent: f64) -> UsageWindow {
    UsageWindow {
        label: String::new(),
        cadence,
        scope: scope.map(str::to_owned),
        used_percent,
        resets_at: None,
    }
}

#[test]
fn antigravity_shows_its_claude_and_gpt_session_and_one_gemini_row() {
    let usage = UsageLimits {
        windows: vec![
            window(UsageCadence::Week, Some("Claude and GPT models"), 10.0),
            window(UsageCadence::Session, Some("Claude and GPT models"), 30.0),
            window(UsageCadence::Session, Some("Gemini"), 60.0),
        ],
        spend: None,
        read_at: 0,
    };
    let resolved = resolve([(EngineId::Antigravity, &usage)]);
    let Gauge::Meters(meters) = &resolved[0].gauge else {
        panic!("Antigravity has limits, not spend: {resolved:?}");
    };
    assert_eq!(
        meters
            .iter()
            .map(|meter| (meter.label.as_str(), meter.used_percent))
            .collect::<Vec<_>>(),
        [("Session", 30.0), ("Gemini", 60.0)],
        "the weekly Claude and GPT window stays in its group's tooltip pool, not a second row"
    );
}

#[test]
fn claude_keeps_one_row_per_model_limit() {
    let usage = UsageLimits {
        windows: vec![
            window(UsageCadence::Session, None, 3.0),
            window(UsageCadence::Week, None, 34.0),
            window(UsageCadence::Week, Some("Fable"), 0.0),
        ],
        spend: None,
        read_at: 0,
    };
    let resolved = resolve([(EngineId::Claude, &usage)]);
    let Gauge::Meters(meters) = &resolved[0].gauge else {
        panic!("Claude has limits: {resolved:?}");
    };
    assert_eq!(
        meters
            .iter()
            .map(|meter| meter.label.as_str())
            .collect::<Vec<_>>(),
        ["Session", "Fable"]
    );
}
