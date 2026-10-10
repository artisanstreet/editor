//! Chart kinds: scales, stacking, hover lookup, statistics, sankey layout,
//! parsing, and the Markdown rendition.

use super::super::{RichFence, prepare};
use super::chart::model::{Model, XScale};
use super::chart::{ChartSpec, Mark};
use super::distribution::{Bins, share_at_or_below, summary};
use super::funnel::Funnel;
use super::heatmap::Heatmap;
use super::hierarchy::{Rect, partition, squarify, tree_for_tests, treemap};
use super::sankey::{Graph, SankeySpec, layout};
use super::scale::{Domain, nice_step, nice_ticks, quantile, ticks_within};
use super::slider::SliderState;
use super::spans::layout_for_tests;
use super::spans::time::{civil_from_days, days_from_civil};

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

fn close(a: f64, b: f64) -> bool {
    (a - b).abs() < 1e-9
}

#[test]
fn nice_steps_round_to_one_two_five() {
    assert!(close(nice_step(0.7), 1.0));
    assert!(close(nice_step(1.3), 2.0));
    assert!(close(nice_step(3.0), 5.0));
    assert!(close(nice_step(7.0), 10.0));
    assert!(close(nice_step(0.023), 0.05));
    assert!(close(nice_step(f64::NAN), 1.0));
    assert!(close(nice_step(0.0), 1.0));
}

#[test]
fn nice_ticks_extend_to_round_bounds_unless_pinned() {
    let ticks = nice_ticks(Domain::new(3.0, 97.0), 5, false, false);
    assert!(
        close(ticks.domain.lo, 0.0) && close(ticks.domain.hi, 100.0),
        "{ticks:?}"
    );
    assert_eq!(ticks.values, vec![0.0, 20.0, 40.0, 60.0, 80.0, 100.0]);
    let pinned = ticks_within(Domain::new(3.0, 97.0), 5);
    assert!(close(pinned.domain.lo, 3.0) && close(pinned.domain.hi, 97.0));
    assert!(
        pinned
            .values
            .iter()
            .all(|value| (3.0..=97.0).contains(value))
    );
    let fine = nice_ticks(Domain::new(0.0, 0.3), 3, false, false);
    assert!(
        fine.values.iter().any(|value| close(*value, 0.3)),
        "{fine:?}"
    );
}

#[test]
fn domains_never_collapse() {
    let flat = Domain::new(5.0, 5.0);
    assert!(flat.span() > 0.0 && flat.contains(5.0));
    let zero = Domain::new(0.0, 0.0);
    assert!(close(zero.lo, -1.0) && close(zero.hi, 1.0));
    let broken = Domain::new(f64::NAN, f64::INFINITY);
    assert!(broken.span() > 0.0 && broken.lo.is_finite());
    assert!(Domain::of([f64::NAN]).is_none());
    let scale = Domain::new(10.0, 20.0);
    assert!((scale.fraction(15.0) - 0.5).abs() < 1e-6);
    assert!((scale.fraction_down(20.0)).abs() < 1e-6);
    assert!(close(scale.value_at(0.25), 12.5));
}

#[test]
fn categories_and_values_share_one_scale() {
    let (_, model) = chart(
        r#"{"categories": ["Q1", "Q2", "Q3"], "series": [
            {"name": "A", "values": [1, null, 3]},
            {"name": "B", "points": [["Q3", 5], ["Q4", 6]]}
        ]}"#,
    );
    assert_eq!(
        model.x,
        XScale::Category(vec!["Q1".into(), "Q2".into(), "Q3".into(), "Q4".into()])
    );
    assert_eq!(model.xs, vec![0.0, 1.0, 2.0, 3.0]);
    assert_eq!(model.aligned(0), vec![Some(1.0), None, Some(3.0), None]);
    assert_eq!(model.aligned(1), vec![None, None, Some(5.0), Some(6.0)]);
    assert_eq!(model.x_label(3.0), "Q4");
}

#[test]
fn numeric_points_sort_and_union() {
    let (spec, model) = chart(
        r#"{"mark": "scatter", "series": [
            {"name": "a", "points": [[3, 1], [1, 2]]},
            {"name": "b", "points": [[2, 5], [3, null]]}
        ]}"#,
    );
    assert_eq!(model.x, XScale::Number);
    assert_eq!(model.xs, vec![1.0, 2.0, 3.0]);
    assert_eq!(
        model.series[0].points,
        vec![(1.0, Some(2.0)), (3.0, Some(1.0))]
    );
    assert_eq!(spec.mark, Mark::Scatter);
    assert!(close(model.band(), 1.0));
}

#[test]
fn stacking_piles_and_normalizes_visible_series() {
    let (_, model) = chart(
        r#"{"categories": ["a", "b"], "series": [
            {"name": "x", "values": [1, 2]},
            {"name": "y", "values": [3, null]},
            {"name": "z", "values": [-1, 2]}
        ]}"#,
    );
    let stacked = model.tracks(&[true, true, true], true, false);
    assert_eq!(stacked[0][0], (0.0, Some((0.0, 1.0))));
    assert_eq!(stacked[1][0], (0.0, Some((1.0, 4.0))));
    assert_eq!(stacked[2][0], (0.0, Some((0.0, -1.0))));
    assert_eq!(stacked[1][1], (1.0, None));
    assert_eq!(stacked[2][1], (1.0, Some((2.0, 4.0))));

    let hidden = model.tracks(&[true, false, true], true, false);
    assert!(hidden[1].is_empty());
    assert_eq!(hidden[2][1], (1.0, Some((2.0, 4.0))));

    let shares = model.tracks(&[true, true, false], false, true);
    let (_, top) = shares[1][0].1.expect("share");
    assert!(close(top, 100.0));
    let (_, first) = shares[0][0].1.expect("share");
    assert!(close(first, 25.0));
}

#[test]
fn hover_finds_the_nearest_x_in_view() {
    let (_, model) = chart(r#"{"series": [{"name": "a", "points": [[0, 1], [10, 2], [20, 3]]}]}"#);
    let everything = Domain::new(0.0, 20.0);
    assert_eq!(model.nearest(6.0, everything), Some(1));
    assert_eq!(model.nearest(-50.0, everything), Some(0));
    assert_eq!(model.nearest(14.0, Domain::new(15.0, 20.0)), Some(2));
    assert_eq!(model.nearest(14.0, Domain::new(11.0, 12.0)), None);
    assert_eq!(model.value_at(0, 10.0), Some(2.0));
    assert_eq!(model.value_at(0, 11.0), None);
}

#[test]
fn thresholds_count_points_at_or_above() {
    let (_, model) = chart(
        r#"{"categories": ["a", "b", "c"], "series": [
            {"name": "x", "values": [1, 5, null]},
            {"name": "y", "values": [5, 9, 2]}
        ]}"#,
    );
    assert_eq!(model.at_or_above(&[true, true], 5.0), (3, 5));
    assert_eq!(model.at_or_above(&[false, true], 5.0), (2, 3));
}

#[test]
fn sliders_snap_and_clamp() {
    let slider = SliderState::new(0.0, 10.0, Some(0.5), 3.3);
    assert!(close(slider.value, 3.5));
    assert!(close(slider.snap(42.0), 10.0));
    assert!(close(slider.snap(f64::NAN), 0.0));
    let backwards = SliderState::new(10.0, 0.0, None, 5.0);
    assert!(close(backwards.value, 5.0));
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
fn histogram_bins_cover_every_value() {
    let values: Vec<f64> = (0..100).map(f64::from).collect();
    let bins = Bins::new(&values, None);
    assert!((1..=50).contains(&bins.count), "{bins:?}");
    let counts = bins.counts(&values);
    assert_eq!(counts.iter().sum::<usize>(), 100);
    assert_eq!(bins.index(99.0), bins.count - 1);
    let requested = Bins::new(&values, Some(10));
    assert_eq!(requested.count, 10);
    assert_eq!(requested.counts(&values), vec![10; 10]);
    let same = Bins::new(&[3.0, 3.0, 3.0], None);
    assert_eq!(same.counts(&[3.0, 3.0, 3.0]).iter().sum::<usize>(), 3);
    let none = Bins::new(&[], None);
    assert_eq!(none.count, 1);
    assert_eq!(Bins::new(&values, Some(100_000)).count, 200);
}

#[test]
fn quartiles_whiskers_and_outliers() {
    let sorted = [1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 100.0];
    let summary = summary(&sorted).expect("summary");
    assert!(close(summary.q1, 3.0));
    assert!(close(summary.median, 5.0));
    assert!(close(summary.q3, 7.0));
    assert!(close(summary.whiskers.0, 1.0));
    assert!(close(summary.whiskers.1, 8.0));
    assert_eq!(summary.outliers, vec![100.0]);
    assert!(super::distribution::summary(&[]).is_none());
    let single = super::distribution::summary(&[4.0]).expect("single");
    assert!(close(single.median, 4.0) && single.outliers.is_empty());
    assert_eq!(quantile(&[1.0, 2.0], 0.5), Some(1.5));
}

#[test]
fn cdf_share_counts_at_or_below() {
    let sorted = [1.0, 2.0, 2.0, 3.0];
    assert!(close(share_at_or_below(&sorted, 2.0), 0.75));
    assert!(close(share_at_or_below(&sorted, 0.0), 0.0));
    assert!(close(share_at_or_below(&sorted, 9.0), 1.0));
    assert!(close(share_at_or_below(&[], 1.0), 0.0));
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
fn sankey_ranks_follow_the_longest_path() {
    let spec: SankeySpec = serde_json::from_str(
        r#"{"nodes": [{"id": "a", "label": "Start"}, {"id": "b"}, {"id": "c"}],
            "links": [{"from": "a", "to": "b", "value": 5}, {"from": "b", "to": "c", "value": 3},
                      {"from": "a", "to": "c", "value": 2}, {"from": "c", "to": "a", "value": 1},
                      {"from": "c", "to": "d", "value": 4}, {"from": "a", "to": "a", "value": 9},
                      {"from": "b", "to": "d", "value": 0}]}"#,
    )
    .expect("sankey");
    let graph = Graph::from_spec(&spec);
    assert_eq!(graph.labels, vec!["Start", "b", "c", "d"]);
    // The cycle-closing c → a, the self link, and the empty link drop out.
    assert_eq!(graph.links.len(), 4);
    assert_eq!(graph.ranks, vec![0, 1, 2, 3]);
    assert!(close(graph.throughput(2), 5.0));
    let laid = layout(&graph, 200.0, 10.0);
    assert_eq!(laid.ranks, 4);
    for node in &laid.nodes {
        assert!(node.y0 >= 0.0 && node.y1 <= 200.0 + 1e-3, "{node:?}");
    }
    // Each link band is as tall at both ends, and as tall as its share.
    for band in &laid.bands {
        let source = band.source.1 - band.source.0;
        let target = band.target.1 - band.target.0;
        assert!((source - target).abs() < 1e-3);
    }
    assert!((laid.column(0)).abs() < 1e-6 && (laid.column(3) - 1.0).abs() < 1e-6);
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

#[test]
fn end_labels_spread_apart_and_stay_inside_the_plot() {
    use super::frame::{EndLabel, spread};
    let label = |y: f32| EndLabel {
        y,
        text: gpui::SharedString::from("s"),
        color: gpui::Hsla::default(),
    };
    let mut labels = vec![label(0.98), label(0.5), label(0.97), label(0.51)];
    spread(&mut labels, 0.1);
    let ys: Vec<f32> = labels.iter().map(|label| label.y).collect();
    for pair in ys.windows(2) {
        assert!(pair[1] - pair[0] >= 0.099, "{ys:?}");
    }
    assert!(ys.iter().all(|y| (0.0..=1.0).contains(y)), "{ys:?}");
}

#[test]
fn concentration_reads_the_top_share_of_the_total() {
    use super::concentration::Concentration;
    let spec: Concentration = serde_json::from_value(serde_json::json!({
        "values": [50, 20, 10, 10, 5, 5, 0, 0, 0, 0],
        "items": "installs",
        "total": "turns"
    }))
    .unwrap();
    let curve = spec.curve().unwrap();
    assert_eq!(curve.first(), Some(&(0.0, 0.0)));
    assert_eq!(curve.last(), Some(&(1.0, 1.0)));
    // The top 10% is the first of ten installs: half of all turns.
    assert!((Concentration::share_of_total(&curve, 0.1) - 0.5).abs() < 1e-9);
    // Halfway between the first and second install.
    assert!((Concentration::share_of_total(&curve, 0.15) - 0.6).abs() < 1e-9);
    let empty: Concentration =
        serde_json::from_value(serde_json::json!({ "values": [0, 0] })).unwrap();
    assert!(empty.curve().is_none());
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
fn tree_sizes_take_own_value_or_children_sum() {
    let tree = tree_for_tests(&tree_json());
    let root = tree.node(0);
    assert!(close(root.size, 40.0));
    // Largest first; the empty child is dropped.
    let names: Vec<&str> = root
        .children
        .iter()
        .map(|child| tree.node(*child).name.as_ref())
        .collect();
    assert_eq!(names, ["core", "ui"]);
    let ui = root.children[1];
    assert!(
        close(tree.node(ui).size, 10.0),
        "own value 10 beats the sum 8"
    );
    assert_eq!(tree.branch(0, tree.node(ui).children[0]), Some(1));
    assert_eq!(tree.path(tree.node(ui).children[0]).len(), 3);
}

#[test]
fn squarify_fills_the_rect_with_the_given_areas() {
    let rect = Rect {
        x: 0.0,
        y: 0.0,
        w: 300.0,
        h: 200.0,
    };
    let items = [(0, 30_000.0), (1, 18_000.0), (2, 9_000.0), (3, 3_000.0)];
    let placed = squarify(&items, rect);
    assert_eq!(placed.len(), 4);
    for ((index, area), (placed_index, rect)) in items.iter().zip(&placed) {
        assert_eq!(index, placed_index);
        assert!(
            (f64::from(rect.w * rect.h) - area).abs() < area * 1e-3,
            "{rect:?}"
        );
        assert!(rect.x >= -1e-3 && rect.x + rect.w <= 300.0 + 1e-3);
        assert!(rect.y >= -1e-3 && rect.y + rect.h <= 200.0 + 1e-3);
    }
}

#[test]
fn treemap_leaves_a_parents_own_share_empty() {
    let tree = tree_for_tests(&tree_json());
    let cells = treemap(&tree, 0, 2, 400.0, 300.0);
    let ui = cells
        .iter()
        .find(|cell| tree.node(cell.node).name == "ui")
        .expect("ui");
    assert!(ui.group);
    let children: f32 = cells
        .iter()
        .filter(|cell| tree.node(cell.node).parent == Some(ui.node))
        .map(|cell| cell.rect.w * cell.rect.h)
        .sum();
    let inner = (ui.rect.w - 4.0) * (ui.rect.h - 20.0);
    assert!((children / inner - 0.8).abs() < 1e-3, "8 of 10 drawn");
}

#[test]
fn partition_splits_widths_by_share() {
    let tree = tree_for_tests(&tree_json());
    let bars = partition(&tree, 0, 6);
    assert_eq!(bars[0].level, 0);
    let core = bars
        .iter()
        .find(|bar| tree.node(bar.node).name == "core")
        .expect("core");
    assert!((core.x1 - core.x0 - 0.75).abs() < 1e-6);
    let chart = bars
        .iter()
        .find(|bar| tree.node(bar.node).name == "chart.rs")
        .expect("chart");
    assert_eq!(chart.level, 2);
    assert!((chart.x1 - chart.x0 - 0.15).abs() < 1e-6);
    let shallow = partition(&tree, 0, 1);
    assert_eq!(shallow.len(), 3);
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
fn civil_days_round_trip() {
    assert_eq!(days_from_civil(1970, 1, 1), 0);
    assert_eq!(days_from_civil(2000, 3, 1), 11_017);
    for days in [-1000, 0, 59, 60, 11_016, 20_733, 40_000] {
        let (year, month, day) = civil_from_days(days);
        assert_eq!(days_from_civil(year, month, day), days);
    }
}

#[test]
fn spans_without_lanes_are_a_waterfall() {
    let layout = layout_for_tests(&serde_json::json!({
        "unit": "ms",
        "spans": [
            { "label": "dns", "start": 0, "end": 12 },
            { "label": "tls", "start": 12, "end": 40 },
            { "label": "body", "start": 40, "end": 95 }
        ]
    }))
    .expect("layout");
    assert_eq!(layout.rows, 3);
    let rows: Vec<usize> = layout.spans.iter().map(|span| span.row).collect();
    assert_eq!(rows, [0, 1, 2]);
    assert!(layout.lanes.is_empty());
    assert_eq!(layout.duration(&layout.spans[2]), "55 ms");
}

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
