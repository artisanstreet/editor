use aqua_core::NamedKey;
use test_support::editor::rich::{ITEM, MountedBody};

const FUNNEL: &str = r#"{"steps":[{"label":"Visits","value":100},{"label":"Signups","value":40},{"label":"Paid","value":50}]}"#;
const BASE: &str = "Funnel. Arrows explore steps; Escape clears the reading.";
const PAID: &str = "Paid: 50 · 50% of Visits · +10 (+25%) from Signups";

#[test]
fn equal_models_keep_selection_and_changed_models_reset_it_and_reveal() {
    let mut funnel = MountedBody::block("funnel", FUNNEL);
    funnel.press(NamedKey::End);
    let reveal = funnel.presenter().funnel_reveals(ITEM);
    assert_eq!(reveal.len(), 1);
    funnel.replace("funnel", FUNNEL);
    assert_eq!(funnel.label(), format!("{BASE} Paid: 50; {PAID}"));
    assert_eq!(funnel.presenter().funnel_reveals(ITEM), reveal);
    funnel.replace("funnel", r#"{"steps":[{"label":"Visits","value":20}]}"#);
    assert_eq!(funnel.label(), BASE);
    assert!(funnel.presenter().funnel_reveals(ITEM).is_empty());
    funnel.press(NamedKey::End);
    let fresh = funnel.presenter().funnel_reveals(ITEM);
    assert_eq!(fresh.len(), 1);
    assert_ne!(fresh[0].1, reveal[0].1);
}

#[test]
fn keyboard_clamps_and_reveal_retries_until_matching_acknowledgement() {
    let mut funnel = MountedBody::block("funnel", FUNNEL);
    funnel.press(NamedKey::Left);
    assert_eq!(
        funnel.label(),
        format!("{BASE} Visits: 100; Visits: 100 · 100% of Visits")
    );
    funnel.press(NamedKey::End);
    funnel.press(NamedKey::Right);
    assert_eq!(funnel.label(), format!("{BASE} Paid: 50; {PAID}"));
    let [(block, key, index)] = <[_; 1]>::try_from(funnel.presenter().funnel_reveals(ITEM))
        .expect("one funnel asks to reveal its selected step");
    funnel.presenter().funnel_revealed(&block, &key, 0);
    funnel
        .presenter()
        .funnel_revealed(&block, "old-list", index);
    assert_eq!(
        funnel.presenter().funnel_reveals(ITEM),
        [(block.clone(), key.clone(), index)]
    );
    funnel.presenter().funnel_revealed(&block, &key, index);
    assert!(funnel.presenter().funnel_reveals(ITEM).is_empty());
    funnel.press_alt(NamedKey::Left);
    assert_eq!(funnel.label(), BASE);
}

#[test]
fn hovering_between_rows_preserves_new_selection_and_reports_gain_and_loss() {
    let mut funnel = MountedBody::block("funnel", FUNNEL);
    let block = funnel.block_id("funnel");
    assert!(funnel.shows("50% of Visits reach Paid"));
    let signups = funnel.rect(&format!("{block}-step-1"));
    funnel.move_within(signups, 0.5, 0.5);
    assert!(funnel.shows("Signups: 40 · 40% of Visits · −60 (−60%) from Visits"));
    let paid = funnel.rect(&format!("{block}-step-2"));
    funnel.move_within(paid, 0.5, 0.5);
    assert!(funnel.shows(PAID));
    funnel.move_to(paid.x + paid.w / 2.0, 890.0);
    assert!(funnel.shows("50% of Visits reach Paid"));
    assert_eq!(funnel.label(), BASE);
}
