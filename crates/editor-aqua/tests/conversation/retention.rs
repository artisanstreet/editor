use artisan_api::{Item, ItemId, ItemKind, Status, ThreadId, ToolKind};
use editor_aqua::conversation::adapter::{RowLayout, RowModel};
use editor_aqua::conversation::disclosure::RowDisclosure;
use editor_aqua::conversation::fold::{Row, Turns};
use editor_aqua::conversation::thread::{ItemUi, ThreadView};
use editor_aqua::conversation::view::factory::{RowFingerprint, row_settles_statically};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

fn item(name: &str, status: Status) -> Arc<Item> {
    Arc::new(Item {
        id: ItemId(name.into()),
        thread_id: ThreadId("thread".into()),
        position: 1,
        seq: 1,
        run_id: None,
        parent_id: None,
        status,
        created_at: 0,
        updated_at: 0,
        kind: ItemKind::Tool {
            tool: ToolKind::Read {
                path: "file".into(),
                range: None,
            },
            title: None,
            output: None,
            exit_code: None,
            approval: None,
        },
    })
}

fn model(status: Status) -> RowModel {
    let item = item("tool", status);
    RowModel {
        row: Row::WorkEntry {
            id: item.id.clone(),
            run: None,
        },
        version: 3,
        items: HashMap::from([(item.id.clone(), item)]),
        live: HashMap::new(),
        ui: HashMap::new(),
        answers: HashMap::new(),
        run: None,
        author: None,
        gap: 8.0,
        end: 0.0,
    }
}

fn view_with(status: Status) -> ThreadView {
    let (mut view, _) = ThreadView::open(ThreadId("thread".into()));
    view.apply_item((*item("tool", status)).clone(), None);
    view
}

fn entry(version: u64) -> (Row, u64) {
    (
        Row::WorkEntry {
            id: ItemId("tool".into()),
            run: None,
        },
        version,
    )
}

fn fingerprint(
    layout: &RowLayout<'_>,
    disclosure: RowDisclosure,
    held: bool,
    body: Option<u64>,
    epoch: u64,
) -> u64 {
    RowFingerprint {
        disclosure,
        held,
        body_version: body,
        style_epoch: epoch,
    }
    .value(layout, layout.rows.len() - 1)
    .expect("the last row has a revision")
}

#[test]
fn a_row_revision_is_stable_for_equal_inputs_and_tracks_every_input() {
    let view = view_with(Status::Done);
    let turns = Turns::default();
    let rows = [(Row::Gap, 1), entry(3)];
    let layout = RowLayout {
        view: &view,
        turns: &turns,
        rows: &rows,
        agent: false,
        composer_height: None,
    };
    let disclosure = RowDisclosure::default();
    let reference = fingerprint(&layout, disclosure, false, Some(1), 0);
    let same_view = view_with(Status::Done);
    assert_eq!(
        reference,
        fingerprint(
            &RowLayout {
                view: &same_view,
                ..layout
            },
            disclosure,
            false,
            Some(1),
            0
        )
    );
    let versioned = [(Row::Gap, 1), entry(4)];
    let first = [entry(3)];
    let mut opened = view_with(Status::Done);
    opened.ui.insert(
        ItemId("tool".into()),
        ItemUi {
            expanded: Some(true),
            ..Default::default()
        },
    );
    let hidden = RowDisclosure {
        hidden: true,
        ..Default::default()
    };
    let changed = [
        fingerprint(
            &RowLayout {
                rows: &versioned,
                ..layout
            },
            disclosure,
            false,
            Some(1),
            0,
        ),
        fingerprint(
            &RowLayout {
                rows: &first,
                ..layout
            },
            disclosure,
            false,
            Some(1),
            0,
        ),
        fingerprint(
            &RowLayout {
                composer_height: Some(300.0),
                ..layout
            },
            disclosure,
            false,
            Some(1),
            0,
        ),
        fingerprint(
            &RowLayout {
                view: &opened,
                ..layout
            },
            disclosure,
            false,
            Some(1),
            0,
        ),
        fingerprint(&layout, hidden, false, Some(1), 0),
        fingerprint(&layout, disclosure, true, Some(1), 0),
        fingerprint(&layout, disclosure, false, Some(2), 0),
        fingerprint(&layout, disclosure, false, None, 0),
        fingerprint(&layout, disclosure, false, Some(1), 1),
    ];
    assert!(changed.iter().all(|value| *value != reference));
}

#[test]
fn only_quiescent_rows_settle() {
    let now = Instant::now();
    assert!(row_settles_statically(&model(Status::Done), now));
    assert!(!row_settles_statically(&model(Status::Running), now));

    let mut fresh = model(Status::Done);
    fresh.ui.insert(
        ItemId("tool".into()),
        ItemUi {
            arrived: Some(now),
            ..Default::default()
        },
    );
    assert!(!row_settles_statically(&fresh, now));

    let mut gap = model(Status::Done);
    gap.row = Row::Gap;
    assert!(!row_settles_statically(&gap, now));
}
