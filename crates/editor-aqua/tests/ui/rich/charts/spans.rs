use aqua_core::NamedKey;
use editor_aqua::ui::rich::charts::SpansFixture;

const SPANS: &str = r#"{"spans":[{"label":"first","lane":"cpu","start":0,"end":10},{"label":"overlap","lane":"cpu","start":5,"end":8},{"label":"after","lane":"cpu","start":10,"end":12}]}"#;

#[test]
fn equal_layout_retains_selection_and_changed_layout_resets_reveal() {
    let mut chart = SpansFixture::new(SPANS);
    chart.key(NamedKey::End, false);
    let reveal = chart.pending_reveal();
    chart.replace(SPANS);
    assert_eq!(chart.selected(), Some(2));
    assert_eq!(chart.pending_reveal(), reveal);
    assert_eq!(chart.generation(), 0);
    chart.replace(r#"{"spans":[{"label":"other","start":0,"end":2}]}"#);
    assert_eq!(chart.selected(), None);
    assert_eq!(chart.pending_reveal(), None);
    assert_eq!(chart.generation(), 1);
}
#[test]
fn keyboard_reveals_selected_spans_assigned_row_and_ack_retries() {
    let chart = SpansFixture::new(SPANS);
    chart.key(NamedKey::End, false);
    let (key, row) = chart.pending_reveal().unwrap();
    assert_eq!(row, chart.row(2));
    chart.revealed("old", row);
    chart.revealed(&key, row + 10);
    assert_eq!(chart.pending_reveal(), Some((key.clone(), row)));
    chart.revealed(&key, row);
    assert_eq!(chart.pending_reveal(), None);
    chart.key(NamedKey::Left, true);
    assert_eq!(chart.selected(), None);
}
#[test]
fn pointer_hit_respects_packed_rows_and_bar_vertical_inset() {
    let chart = SpansFixture::new(SPANS);
    for index in 0..3 {
        let row = chart.row(index);
        let x = chart.bar_center(index);
        assert_eq!(chart.hit(row, x, 16.0), Some(index));
        assert_eq!(chart.hit(row, x, 1.0), None);
    }
    assert_ne!(chart.row(0), chart.row(1));
}
