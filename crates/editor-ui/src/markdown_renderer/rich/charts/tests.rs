//! Chart kinds: parsing, layout and the Markdown rendition.

use super::super::{RichFence, prepare};
use super::chart::ChartSpec;
use super::chart::model::Model;
use super::funnel::Funnel;
use super::heatmap::Heatmap;
use super::spans::layout_for_tests;

fn markdown(kind: &str, json: &str) -> String {
    match prepare(Some(&format!("artisan-{kind}")), json, true) {
        Some(RichFence::Ready(prepared)) => prepared.markdown().to_string(),
        other => panic!("expected artisan-{kind} to parse, got {other:?}"),
    }
}

fn invalid(kind: &str, json: &str) -> String {
    match prepare(Some(&format!("artisan-{kind}")), json, true) {
        Some(RichFence::Invalid { note }) => note.to_string(),
        other => panic!("expected artisan-{kind} to be invalid, got {other:?}"),
    }
}

fn chart(json: &str) -> (ChartSpec, Model) {
    let spec: ChartSpec = serde_json::from_str(json).expect("chart spec");
    let model = Model::from_spec(&spec);
    (spec, model)
}

#[test]
fn odd_charts_do_not_panic() {
    for json in [
        r#"{"series": []}"#,
        r#"{"series": [{"name": "a"}]}"#,
        r#"{"series": [{"name": "a", "values": [null, null]}]}"#,
        r#"{"series": [{"name": "a", "points": [[1, 4]]}], "mark": "bar"}"#,
        r#"{"series": [{"name": "a", "values": [2, 2, 2]}], "y": {"min": 5, "max": 1}}"#,
        r#"{"series": [{"name": "a", "points": [["2026-01", 1], ["2026-02", 2]]}], "annotations": [{"x": "2026-02", "label": "launch"}]}"#,
    ] {
        let (spec, model) = chart(json);
        let tracks = model.tracks(&[true], spec.stacks(), spec.normalize);
        let view = model.x_domain(spec.mark);
        let ticks = super::chart::model::y_ticks(&tracks, view, spec.mark, spec.stacks(), &spec.y);
        assert!(ticks.domain.span() > 0.0, "{json}");
        assert!(view.span() > 0.0, "{json}");
        let _ = super::chart::markdown(&spec, &model);
    }
}

#[test]
fn chart_parses_and_renders_markdown() {
    let text = markdown(
        "chart",
        r#"{"title": "Latency", "mark": "bar", "categories": ["a", "b"],
            "series": [{"name": "p50", "values": [12, 14.5]}, {"name": "p99", "values": [40, null]}],
            "y": {"unit": "ms"},
            "annotations": [{"y": 30, "label": "SLO"}, {"x": "a", "x_end": "b", "label": "rollout"}],
            "params": [{"id": "t", "kind": "threshold", "label": "Budget", "min": 0, "max": 50, "value": 25}]}"#,
    );
    assert!(text.starts_with("**Latency**"), "{text}");
    assert!(text.contains("| Category | p50 | p99 |"), "{text}");
    assert!(text.contains("| b | 14.5 |  |"), "{text}");
    assert!(text.contains("y in ms"), "{text}");
    assert!(text.contains("- SLO (y = 30)"), "{text}");
    assert!(text.contains("- rollout (x a to b)"), "{text}");
    assert!(text.contains("- Budget: 25"), "{text}");
}

#[test]
fn required_fields_name_the_problem() {
    assert!(invalid("chart", r#"{"mark": "line"}"#).contains("series"));
    assert!(invalid("chart", r#"{"mark": "pie", "series": []}"#).contains("pie"));
    assert!(invalid("heatmap", r#"{"x": ["a"]}"#).contains("values"));
    assert!(invalid("distribution", r#"{"mode": "box"}"#).contains("series"));
    assert!(invalid("funnel", r#"{"steps": [{"label": "a"}]}"#).contains("value"));
    assert!(invalid("sankey", r#"{"nodes": []}"#).contains("links"));
}

#[test]
fn heatmap_colors_and_markdown() {
    let spec: Heatmap = serde_json::from_str(
        r#"{"x": ["mon", "tue"], "y": ["am", "pm"], "values": [[1, null], [3, 5]], "scale": "diverging"}"#,
    )
    .expect("heatmap");
    assert_eq!((spec.rows(), spec.columns()), (2, 2));
    assert_eq!(spec.cell(0, 1), None);
    assert!((spec.position(5.0) - 1.0).abs() < 1e-6);
    assert!((spec.position(-5.0) + 1.0).abs() < 1e-6);
    let text = markdown(
        "heatmap",
        r#"{"x": ["mon", "tue"], "y": ["am"], "values": [[1, null]], "unit": "ms"}"#,
    );
    assert!(text.contains("|  | mon | tue |"), "{text}");
    assert!(text.contains("| am | 1 |  |"), "{text}");
    assert!(text.contains("_Values in ms_"), "{text}");
    let flat: Heatmap = serde_json::from_str(r#"{"values": [[2, 2]]}"#).expect("flat");
    assert!(flat.position(2.0).is_finite());
}

#[test]
fn distribution_markdown_summarizes_each_series() {
    let text = markdown(
        "distribution",
        r#"{"series": [{"name": "build", "values": [1, 2, 3, 4, null]}], "bins": 2, "unit": "s", "threshold": 2}"#,
    );
    assert!(
        text.contains("| Series | n | min | q1 | median | q3 | max | mean |"),
        "{text}"
    );
    assert!(text.contains("| build | 4 | 1 |"), "{text}");
    assert!(text.contains("| Bin | build |"), "{text}");
    assert!(text.contains("- Threshold: 2"), "{text}");
    assert!(text.contains("build: 50% at or below"), "{text}");
}

#[test]
fn funnel_reads_shares_and_drops() {
    let funnel: Funnel = serde_json::from_str(
        r#"{"steps": [{"label": "Visit", "value": 1000}, {"label": "Sign up", "value": 250}, {"label": "Pay", "value": 50}]}"#,
    )
    .expect("funnel");
    let pay = funnel.reading(2);
    assert_eq!(pay.of_first, Some(0.05));
    assert_eq!(pay.drop, Some((200.0, Some(0.8))));
    assert_eq!(funnel.reading(0).drop, None);
    assert!((funnel.widths()[1] - 0.25).abs() < 1e-6);
    let empty: Funnel =
        serde_json::from_str(r#"{"steps": [{"label": "a", "value": 0}]}"#).expect("zero");
    assert_eq!(empty.widths(), vec![0.0]);
    assert_eq!(empty.reading(0).of_first, None);
    let text = markdown(
        "funnel",
        r#"{"steps": [{"label": "Visit", "value": 10}, {"label": "Pay", "value": 4}]}"#,
    );
    assert!(text.contains("| Pay | 4 | 40% | −6 (−60%) |"), "{text}");
}

#[test]
fn sankey_markdown_lists_flows() {
    let text = markdown(
        "sankey",
        r#"{"nodes": [{"id": "a", "label": "Inbox"}], "links": [{"from": "a", "to": "b", "value": 2}]}"#,
    );
    assert!(text.contains("| From | To | Value |"), "{text}");
    assert!(text.contains("| Inbox | b | 2 |"), "{text}");
}

// ---------------------------------------------------------------------------
// Hierarchy

fn tree_json() -> serde_json::Value {
    serde_json::json!({
        "name": "src",
        "children": [
            { "name": "ui", "value": 10, "children": [
                { "name": "chart.rs", "value": 6 },
                { "name": "scene.rs", "value": 2 }
            ] },
            { "name": "core", "value": 30 },
            { "name": "empty", "value": 0 }
        ]
    })
}

#[test]
fn hierarchy_needs_a_size_and_outlines_as_markdown() {
    let out = markdown("hierarchy", &tree_json().to_string());
    assert!(out.contains("- **src** \u{2014} 40 (100%)"), "{out}");
    assert!(out.contains("  - **core** \u{2014} 30 (75%)"), "{out}");
    assert!(invalid("hierarchy", r#"{"name": "x"}"#).contains("positive"));
    assert!(invalid("hierarchy", r#"{"value": 1, "layout": "pie"}"#).contains("pie"));
}

// ---------------------------------------------------------------------------
// Spans

#[test]
fn laned_spans_stack_when_they_overlap() {
    let layout = layout_for_tests(&serde_json::json!({
        "spans": [
            { "label": "child", "start": 10, "end": 20, "lane": "api" },
            { "label": "request", "start": 0, "end": 100, "lane": "api" },
            { "label": "later", "start": 30, "end": 40, "lane": "api" },
            { "label": "query", "start": 5, "end": 50, "lane": "db" },
            { "label": "loose", "start": 0, "end": 5 }
        ]
    }))
    .expect("layout");
    let row = |label: &str| {
        layout
            .spans
            .iter()
            .find(|span| span.label == label)
            .map(|span| span.row)
            .expect(label)
    };
    // The parent sits over its children, which share the row beneath.
    assert_eq!(row("request"), 0);
    assert_eq!(row("child"), 1);
    assert_eq!(row("later"), 1);
    assert_eq!(row("query"), 2);
    assert_eq!(row("loose"), 3);
    let lanes: Vec<(&str, usize, usize)> = layout
        .lanes
        .iter()
        .map(|lane| (lane.name.as_ref(), lane.first_row, lane.rows))
        .collect();
    assert_eq!(lanes, [("api", 0, 2), ("db", 2, 1), ("loose", 3, 1)]);
    assert_eq!(layout.rows, 4);
}

#[test]
fn spans_read_dates_and_refuse_mixed_times() {
    let layout = layout_for_tests(&serde_json::json!({
        "spans": [{ "label": "Build", "start": "2026-10-01", "end": "2026-10-08" }],
        "markers": [{ "at": "2026-10-07", "label": "Today" }]
    }))
    .expect("layout");
    assert!(layout.dates);
    assert_eq!(layout.duration(&layout.spans[0]), "7 days");
    assert_eq!(layout.time(layout.markers[0].at), "Oct 7, 2026");
    let mixed = layout_for_tests(&serde_json::json!({
        "spans": [{ "start": 0, "end": "2026-10-08" }]
    }));
    assert!(mixed.expect_err("mixed").contains("mixes"));
    assert!(invalid("spans", r#"{"spans": [{"start": "soon", "end": 1}]}"#).contains("ISO date"));
    let out = markdown(
        "spans",
        r#"{"spans": [{"label": "a", "start": 1, "end": 3, "lane": "x"}]}"#,
    );
    assert!(out.contains("| a | x | 1 | 3 | 2 |"), "{out}");
}
