use aqua_core::NamedKey;
use editor_aqua::ui::rich::charts::FunnelFixture;

const FUNNEL: &str = r#"{"steps":[{"label":"Visits","value":100},{"label":"Signups","value":40},{"label":"Paid","value":50}]}"#;

#[test]
fn equal_models_keep_selection_and_changed_models_reset_it_and_reveal() {
    let mut funnel = FunnelFixture::new(FUNNEL);
    funnel.key(NamedKey::End, false);
    let reveal = funnel.pending_reveal();
    funnel.replace(FUNNEL);
    assert_eq!(funnel.selected(), Some(2));
    assert_eq!(funnel.pending_reveal(), reveal);
    assert_eq!(funnel.generation(), 0);
    funnel.replace(r#"{"steps":[{"label":"Visits","value":20}]}"#);
    assert_eq!(funnel.selected(), None);
    assert_eq!(funnel.pending_reveal(), None);
    assert_eq!(funnel.generation(), 1);
}

#[test]
fn keyboard_clamps_and_reveal_retries_until_matching_acknowledgement() {
    let funnel = FunnelFixture::new(FUNNEL);
    funnel.key(NamedKey::Left, false);
    assert_eq!(funnel.selected(), Some(0));
    funnel.key(NamedKey::End, false);
    funnel.key(NamedKey::Right, false);
    assert_eq!(funnel.selected(), Some(2));
    let (key, index) = funnel.pending_reveal().unwrap();
    funnel.revealed(&key, 0);
    funnel.revealed("old-list", index);
    assert_eq!(funnel.pending_reveal(), Some((key.clone(), index)));
    funnel.revealed(&key, index);
    assert_eq!(funnel.pending_reveal(), None);
    funnel.key(NamedKey::Left, true);
    assert_eq!(funnel.selected(), None);
}

#[test]
fn hovering_between_rows_preserves_new_selection_and_reports_gain_and_loss() {
    let funnel = FunnelFixture::new(FUNNEL);
    assert_eq!(funnel.summary(), "50% of Visits reach Paid");
    assert!(funnel.hover(1, true));
    assert_eq!(
        funnel.summary(),
        "Signups: 40 · 40% of Visits · −60 (−60%) from Visits"
    );
    assert!(funnel.hover(2, true));
    assert!(!funnel.hover(1, false));
    assert_eq!(funnel.selected(), Some(2));
    assert_eq!(
        funnel.summary(),
        "Paid: 50 · 50% of Visits · +10 (+25%) from Signups"
    );
    assert!(funnel.hover(2, false));
    assert_eq!(funnel.selected(), None);
}
