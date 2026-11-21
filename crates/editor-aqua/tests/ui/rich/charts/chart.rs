use aqua_core::NamedKey;
use test_support::editor::rich::{MountedBody, fence};

const CHART: &str = r#"{"mark":"line","series":[{"values":[1,2,3,4,5]}]}"#;
const BASE: &str =
    "Chart. Arrows explore values; Home and End select endpoints; Escape resets zoom.";

fn drag_across(chart: &mut MountedBody, from: f32, to: f32) {
    let plot = chart.rect(&format!("{}-plot", chart.block_id("chart")));
    let y = plot.y + plot.h / 2.0;
    chart.drag(
        (plot.w.mul_add(from, plot.x), y),
        (plot.w.mul_add(to, plot.x), y),
    );
}

fn zoomed(chart: &MountedBody) -> bool {
    chart
        .keyed(&format!("{}-reset", chart.block_id("chart")))
        .is_some()
}

#[test]
fn categorical_zoom_snaps_to_points_and_escape_resets_without_clearing_selection() {
    let mut chart = MountedBody::block("chart", CHART);
    drag_across(&mut chart, 1.1 / 4.0, 3.1 / 4.0);
    assert!(zoomed(&chart));
    chart.press(NamedKey::End);
    assert_eq!(chart.label(), format!("{BASE} 4; Series 1: 4"));
    chart.press(NamedKey::Home);
    assert_eq!(chart.label(), format!("{BASE} 3; Series 1: 3"));
    chart.press(NamedKey::Escape);
    assert!(!zoomed(&chart));
    assert_eq!(chart.label(), format!("{BASE} 3; Series 1: 3"));
}

#[test]
fn short_drag_does_not_zoom_and_model_identity_controls_reset() {
    let mut chart = MountedBody::block("chart", CHART);
    drag_across(&mut chart, 0.0, 0.0025);
    assert!(!zoomed(&chart));
    let block = chart.block_id("chart");
    chart.click(&format!("{block}-legend-0"));
    chart.press(NamedKey::End);
    let hidden = format!("{BASE} 5; No values: ");
    assert_eq!(chart.label(), hidden);
    chart.edit(&fence("chart", CHART));
    assert_eq!(chart.label(), hidden);
    chart.replace(
        "chart",
        r#"{"mark":"line","series":[{"values":[1,2,3,4,6]}]}"#,
    );
    assert_eq!(chart.label(), BASE);
    chart.press(NamedKey::End);
    assert_eq!(chart.label(), format!("{BASE} 5; Series 1: 6"));
}
