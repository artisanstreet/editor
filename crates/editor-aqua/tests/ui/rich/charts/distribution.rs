use aqua_core::NamedKey;
use editor_aqua::ui::rich::charts::DistributionFixture;

#[test]
fn equal_model_retains_threshold_and_hover_changed_model_resets_both() {
    let source = r#"{"mode":"histogram","bins":4,"series":[{"values":[1,2,3,4]}]}"#;
    let mut chart = DistributionFixture::new(source);
    chart.set_threshold(3.0);
    chart.hover(0.5, 0.5);
    chart.replace(source);
    assert_eq!(chart.threshold(), 3.0);
    assert_eq!(chart.selected(), Some((0.5, 0.5)));
    assert_eq!(chart.generation(), 0);
    chart.replace(r#"{"mode":"histogram","bins":4,"series":[{"values":[10,20,30,40]}]}"#);
    assert_eq!(chart.threshold(), 25.0);
    assert_eq!(chart.selected(), None);
    assert_eq!(chart.generation(), 1);
}

#[test]
fn histogram_and_cdf_keyboard_select_mode_specific_coordinates() {
    let histogram = DistributionFixture::new(
        r#"{"mode":"histogram","bins":4,"series":[{"values":[1,2,3,4]}]}"#,
    );
    histogram.key(NamedKey::End, false);
    assert_eq!(histogram.selected(), Some((0.875, 0.5)));
    histogram.key(NamedKey::Right, false);
    assert_eq!(histogram.selected(), Some((0.875, 0.5)));
    let cdf = DistributionFixture::new(r#"{"mode":"cdf","series":[{"values":[1,2]}]}"#);
    cdf.key(NamedKey::Home, false);
    cdf.key(NamedKey::Right, false);
    assert_eq!(cdf.selected(), Some((0.01, 0.5)));
    cdf.key(NamedKey::Left, true);
    assert_eq!(cdf.selected(), None);
}

#[test]
fn box_keyboard_reveal_stays_pending_until_matching_ack() {
    let chart = DistributionFixture::new(
        r#"{"mode":"box","series":[{"values":[1]},{"values":[2]},{"values":[3]}]}"#,
    );
    chart.key(NamedKey::End, false);
    assert_eq!(chart.selected(), Some((0.5, 5.0 / 6.0)));
    let (key, row) = chart.pending_reveal().unwrap();
    chart.revealed("old", row);
    chart.revealed(&key, 0);
    assert_eq!(chart.pending_reveal(), Some((key.clone(), row)));
    chart.revealed(&key, row);
    assert_eq!(chart.pending_reveal(), None);
}
