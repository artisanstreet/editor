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
