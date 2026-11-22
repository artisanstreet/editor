use aqua_core::NamedKey;
use test_support::editor::rich::{ITEM, MountedBody};

const READINGS: &str = "Distribution chart. Arrows explore readings; Escape clears the reading.";
const HISTOGRAM: &str = r#"{"mode":"histogram","bins":4,"series":[{"values":[1,2,3,4]}]}"#;

#[test]
fn equal_model_retains_threshold_and_hover_changed_model_resets_both() {
    let mut chart = MountedBody::block("distribution", HISTOGRAM);
    let block = chart.block_id("distribution");
    chart.press_on(&format!("{block}-slider"), NamedKey::End);
    let plot = chart.rect(&format!("{block}-plot"));
    chart.move_within(plot, 0.5, 0.5);
    let hovered = format!("{READINGS} 2.5 – 3.25; Series 1: 1 · 25%");
    assert_eq!(chart.label(), hovered);
    chart.replace("distribution", HISTOGRAM);
    assert!(chart.shows("≤ 4 · 100%"));
    assert_eq!(chart.label(), hovered);
    chart.replace(
        "distribution",
        r#"{"mode":"histogram","bins":4,"series":[{"values":[10,20,30,40]}]}"#,
    );
    assert!(chart.shows("≤ 25 · 50%"));
    assert_eq!(chart.label(), READINGS);
}

#[test]
fn histogram_and_cdf_keyboard_select_mode_specific_coordinates() {
    let mut histogram = MountedBody::block("distribution", HISTOGRAM);
    histogram.press(NamedKey::End);
    let last = format!("{READINGS} 3.25 – 4; Series 1: 1 · 25%");
    assert_eq!(histogram.label(), last);
    histogram.press(NamedKey::Right);
    assert_eq!(histogram.label(), last);
    let mut cdf = MountedBody::block(
        "distribution",
        r#"{"mode":"cdf","series":[{"values":[1,2]}]}"#,
    );
    cdf.press(NamedKey::Home);
    cdf.press(NamedKey::Right);
    assert_eq!(
        cdf.label(),
        format!("{READINGS} 1.01; Series 1: 50% at or below")
    );
    cdf.press_alt(NamedKey::Left);
    assert_eq!(cdf.label(), READINGS);
}

#[test]
fn box_keyboard_reveal_stays_pending_until_matching_ack() {
    let mut chart = MountedBody::block(
        "distribution",
        r#"{"mode":"box","series":[{"values":[1]},{"values":[2]},{"values":[3]}]}"#,
    );
    chart.press(NamedKey::End);
    assert!(
        chart.label().starts_with(
            "Box distributions. Arrows select rows; Escape clears the reading. Series 3; n: 1;"
        ),
        "{}",
        chart.label()
    );
    let [(block, key, row)] = <[_; 1]>::try_from(chart.presenter().distribution_reveals(ITEM))
        .expect("one box chart asks to reveal its selected row");
    assert_eq!(row, 2);
    chart.presenter().distribution_revealed(&block, "old", row);
    chart.presenter().distribution_revealed(&block, &key, 0);
    assert_eq!(
        chart.presenter().distribution_reveals(ITEM),
        [(block.clone(), key.clone(), row)]
    );
    chart.presenter().distribution_revealed(&block, &key, row);
    assert!(chart.presenter().distribution_reveals(ITEM).is_empty());
}
