use aqua_core::NamedKey;
use editor_aqua::ui::rich::charts::ConcentrationFixture;

#[test]
fn hovered_share_interpolates_ranked_curve_and_rejects_outside_points() {
    let chart = ConcentrationFixture::new(r#"{"values":[1,3,2]}"#);
    assert!(chart.hover(0.5, 0.5));
    assert!((chart.held().unwrap() - 2.0 / 3.0).abs() < 1e-9);
    assert!(chart.hover(1.0, 1.0));
    assert_eq!(chart.held(), Some(1.0));
    assert!(chart.hover(1.001, 0.5));
    assert_eq!(chart.selected(), None);
}

#[test]
fn keyboard_rounds_hover_to_percent_and_clamps_while_model_replacement_retains_state() {
    let mut chart = ConcentrationFixture::new(r#"{"values":[1,3,2]}"#);
    chart.hover(0.256, 0.5);
    chart.key(NamedKey::Right, false);
    assert_eq!(chart.selected(), Some(0.27));
    chart.replace(r#"{"values":[10,5]}"#);
    assert_eq!(chart.selected(), Some(0.27));
    chart.key(NamedKey::End, false);
    chart.key(NamedKey::Right, false);
    assert_eq!(chart.selected(), Some(1.0));
    chart.key(NamedKey::Left, true);
    assert_eq!(chart.selected(), None);
}

#[test]
fn curve_sampling_is_bounded_and_keeps_both_endpoints() {
    let values = vec![1; 1000];
    let chart = ConcentrationFixture::new(&serde_json::json!({"values": values}).to_string());
    let samples = chart.sample_points(400);
    assert_eq!(samples.len(), 400);
    assert_eq!(samples.first(), Some(&(0.0, 0.0)));
    assert_eq!(samples.last(), Some(&(1.0, 1.0)));
}
