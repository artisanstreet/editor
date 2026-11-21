use super::{Projections, ShapeRevision, ShapeSource, ShapeText};
use crate::conversation::fold::{Row, Slot};
use std::fmt::Write as _;

fn row() -> Row {
    Row::Text {
        id: artisan_api::ItemId("reply".into()),
        slot: Slot::Prose,
        block: 0,
        run: None,
    }
}

fn expand_source(projections: &mut Projections, source: ShapeSource<'_>) -> Vec<(Row, u64)> {
    projections.expand(vec![(row(), 1)], |_, _| None, |_, _| Some(source))
}

fn expand_live(projections: &mut Projections, epoch: u64, text: &str) -> Vec<(Row, u64)> {
    let rope = ropey::Rope::from_str(text);
    expand_source(
        projections,
        ShapeSource {
            text: ShapeText::Live(&rope),
            revision: ShapeRevision::Live {
                epoch,
                bytes: rope.len_bytes(),
            },
            message: false,
            seq: 1,
            running: true,
        },
    )
}

fn expand_saved(
    projections: &mut Projections,
    seq: u64,
    text: &str,
    running: bool,
) -> Vec<(Row, u64)> {
    expand_source(
        projections,
        ShapeSource {
            text: ShapeText::Inline(text),
            revision: ShapeRevision::Saved(seq),
            message: false,
            seq: 1,
            running,
        },
    )
}

fn expand(projections: &mut Projections, source: &str) -> Vec<(Row, u64)> {
    expand_live(projections, 1, source)
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
    let before = versions(&expand_live(&mut projections, 1, "alpha\n\nbeta\n\ngamma"));
    let after = versions(&expand_live(
        &mut projections,
        2,
        "alpha\n\nbetter\n\ngamma",
    ));
    assert_eq!(before[0], after[0]);
    assert_ne!(before[1], after[1]);
}

#[test]
fn shaped_bodies_report_their_leaf_count() {
    let mut projections = Projections::default();
    let key = crate::conversation::presentation::BodyKey {
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
    let live = versions(&expand(&mut projections, "alpha\n\nbeta\n\ngamma"));
    let done = versions(&expand_saved(
        &mut projections,
        2,
        "alpha\n\nbeta\n\ngamma",
        false,
    ));
    assert_eq!(live[..2], done[..2]);
    assert_ne!(live[2], done[2]);
}

#[test]
fn a_live_reply_shaped_delta_by_delta_matches_one_shaped_whole() {
    let mut reply = String::new();
    for step in 0..40 {
        write!(
            reply,
            "## Step {step}\n\n- one\n- two with `code`\n\n> quoted {step}\n>\n> more\n\n```ts\ntype T = {{ [k: string]: {step} }};\n```\n\n"
        )
        .expect("writing into a String cannot fail");
    }
    let mut streamed = Projections::default();
    let mut end = 0;
    while end < reply.len() {
        end = (end + 7).min(reply.len());
        expand(&mut streamed, reply.get(..end).expect("the reply is ASCII"));
    }
    let mut whole = Projections::default();
    assert_eq!(
        versions(&expand(&mut streamed, &reply)),
        versions(&expand_saved(&mut whole, 1, &reply, true))
    );
}
