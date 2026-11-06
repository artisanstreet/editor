use editor_aqua::conversation::fold::{Row, Slot};
use editor_aqua::conversation::projection::{Projections, ShapeSource, ShapeText};

fn expand(projections: &mut Projections, source: &str) -> Vec<(Row, u64)> {
    expand_with(projections, source, true)
}

fn expand_with(projections: &mut Projections, source: &str, running: bool) -> Vec<(Row, u64)> {
    let id = artisan_api::ItemId("reply".into());
    let row = Row::Text {
        id,
        slot: Slot::Prose,
        block: 0,
        run: None,
    };
    projections.expand(
        vec![(row, 1)],
        |_, _| None,
        |_, _| {
            Some(ShapeSource {
                text: ShapeText::Inline(source),
                message: false,
                seq: 1,
                running,
            })
        },
    )
}

fn versions(rows: &[(Row, u64)]) -> Vec<u64> {
    rows.iter().map(|(_, version)| *version).collect()
}

#[test]
fn streaming_into_the_last_block_leaves_sealed_blocks_untouched() {
    let mut projections = Projections::default();
    let first = versions(&expand(&mut projections, "alpha\n\nbeta\n\ngam"));
    let second = versions(&expand(&mut projections, "alpha\n\nbeta\n\ngamma"));
    assert_eq!(first.len(), 3);
    assert_eq!(second.len(), 3);
    assert_eq!(first[..2], second[..2]);
    assert_ne!(first[2], second[2]);
}

#[test]
fn a_new_block_reseals_only_the_previous_last_block() {
    let mut projections = Projections::default();
    let before = versions(&expand(&mut projections, "alpha\n\nbeta\n\ngamma"));
    let after = versions(&expand(&mut projections, "alpha\n\nbeta\n\ngamma\n\ndelta"));
    assert_eq!(after.len(), 4);
    assert_eq!(before[..2], after[..2]);
    assert_ne!(before[2], after[2]);
}

#[test]
fn editing_a_block_leaves_the_blocks_before_it_untouched() {
    let mut projections = Projections::default();
    let before = versions(&expand(&mut projections, "alpha\n\nbeta\n\ngamma"));
    let after = versions(&expand(&mut projections, "alpha\n\nbetter\n\ngamma"));
    assert_eq!(before[0], after[0]);
    assert_ne!(before[1], after[1]);
}

#[test]
fn shaped_bodies_report_their_leaf_count() {
    let mut projections = Projections::default();
    let key = editor_aqua::conversation::presentation::BodyKey {
        item: artisan_api::ItemId("reply".into()),
        slot: Slot::Prose,
    };
    assert_eq!(projections.leaf_count(&key), None);
    expand(&mut projections, "one\n\ntwo");
    assert_eq!(projections.leaf_count(&key), Some(2));
    assert!(projections.shaped(&key));
}

#[test]
fn finishing_a_reply_reversions_only_its_last_block() {
    let mut projections = Projections::default();
    let live = versions(&expand_with(&mut projections, "alpha\n\nbeta\n\ngamma", true));
    let done = versions(&expand_with(&mut projections, "alpha\n\nbeta\n\ngamma", false));
    assert_eq!(live[..2], done[..2]);
    assert_ne!(live[2], done[2]);
}
