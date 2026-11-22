use aqua_core::NamedKey;
use test_support::editor::rich::MountedBody;

const BASE: &str =
    "Concentration chart. Arrows explore percentage shares; Escape clears the reading.";

fn hover(chart: &mut MountedBody, x: f32, y: f32) {
    let plot = chart.rect(&format!("{}-plot", chart.block_id("concentration")));
    chart.move_within(plot, x, y);
}

#[test]
fn hovered_share_interpolates_ranked_curve_and_rejects_outside_points() {
    let mut chart = MountedBody::block("concentration", r#"{"values":[1,3,2]}"#);
    hover(&mut chart, 0.5, 0.5);
    assert_eq!(
        chart.label(),
        format!("{BASE} Top 50% of items: 67% of the total")
    );
    hover(&mut chart, 0.999, 0.999);
    assert_eq!(
        chart.label(),
        format!("{BASE} Top 100% of items: 100% of the total")
    );
    hover(&mut chart, 1.001, 0.5);
    assert_eq!(chart.label(), BASE);
}

#[test]
fn keyboard_rounds_hover_to_percent_and_clamps_while_model_replacement_retains_state() {
    let mut chart = MountedBody::block("concentration", r#"{"values":[1,3,2]}"#);
    hover(&mut chart, 0.256, 0.5);
    chart.press(NamedKey::Right);
    assert!(
        chart
            .label()
            .starts_with(&format!("{BASE} Top 27% of items: ")),
        "{}",
        chart.label()
    );
    chart.replace("concentration", r#"{"values":[10,5]}"#);
    assert_eq!(
        chart.label(),
        format!("{BASE} Top 27% of items: 36% of the total")
    );
    chart.press(NamedKey::End);
    chart.press(NamedKey::Right);
    assert_eq!(
        chart.label(),
        format!("{BASE} Top 100% of items: 100% of the total")
    );
    chart.press_alt(NamedKey::Left);
    assert_eq!(chart.label(), BASE);
}
