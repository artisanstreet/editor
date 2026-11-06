use aqua_core::NamedKey;
use editor_aqua::ui::rich::charts::ChartFixture;

const CHART: &str = r#"{"mark":"line","series":[{"values":[1,2,3,4,5]}]}"#;

#[test]
fn categorical_zoom_snaps_to_points_and_escape_resets_without_clearing_selection() {
    let chart = ChartFixture::new(CHART);
    chart.drag(1.1, 3.1, 480.0);
    assert_eq!(chart.zoom(), Some((1.5, 3.5)));
    chart.key(NamedKey::Home, false);
    assert_eq!(chart.selected(), Some(2));
    chart.key(NamedKey::Escape, false);
    assert_eq!(chart.zoom(), None);
    assert_eq!(chart.selected(), Some(2));
}
#[test]
fn short_drag_does_not_zoom_and_model_identity_controls_reset() {
    let mut chart = ChartFixture::new(CHART);
    chart.drag(0.0, 0.01, 480.0);
    assert_eq!(chart.zoom(), None);
    chart.toggle(0);
    chart.key(NamedKey::End, false);
    chart.resync();
    assert_eq!(chart.hidden(), vec![true]);
    assert_eq!(chart.selected(), Some(4));
    chart.replace(CHART);
    assert_eq!(chart.hidden(), vec![false]);
    assert_eq!(chart.selected(), None);
}
