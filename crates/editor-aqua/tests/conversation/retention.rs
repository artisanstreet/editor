use artisan_api::{Item, ItemId, ItemKind, Status, ThreadId, ToolKind};
use editor_aqua::conversation::adapter::RowModel;
use editor_aqua::conversation::disclosure::RowDisclosure;
use editor_aqua::conversation::fold::Row;
use editor_aqua::conversation::thread::ItemUi;
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
        run: None,
        author: None,
        gap: 8.0,
        end: 0.0,
    }
}

fn fingerprint(model: &RowModel, disclosure: &RowDisclosure, held: bool, body: Option<u64>, epoch: u64) -> u64 {
    RowFingerprint {
        model,
        disclosure,
        held,
        body_version: body,
        style_epoch: epoch,
    }
    .value()
}

#[test]
fn fingerprint_is_stable_for_equal_inputs_and_tracks_every_input() {
    let base = model(Status::Done);
    let disclosure = RowDisclosure::default();
    let reference = fingerprint(&base, &disclosure, false, Some(1), 0);
    assert_eq!(reference, fingerprint(&model(Status::Done), &disclosure, false, Some(1), 0));

    let mut versioned = model(Status::Done);
    versioned.version = 4;
    let mut spaced = model(Status::Done);
    spaced.gap = 12.0;
    let mut ended = model(Status::Done);
    ended.end = 40.0;
    let mut sending = model(Status::Done);
    sending.ui.insert(
        ItemId("tool".into()),
        ItemUi {
            sending: true,
            ..Default::default()
        },
    );
    let hidden = RowDisclosure {
        hidden: true,
        ..Default::default()
    };
    let changed = [
        fingerprint(&versioned, &disclosure, false, Some(1), 0),
        fingerprint(&spaced, &disclosure, false, Some(1), 0),
        fingerprint(&ended, &disclosure, false, Some(1), 0),
        fingerprint(&sending, &disclosure, false, Some(1), 0),
        fingerprint(&base, &hidden, false, Some(1), 0),
        fingerprint(&base, &disclosure, true, Some(1), 0),
        fingerprint(&base, &disclosure, false, Some(2), 0),
        fingerprint(&base, &disclosure, false, None, 0),
        fingerprint(&base, &disclosure, false, Some(1), 1),
    ];
    assert!(changed.iter().all(|value| *value != reference));
}

#[test]
fn fingerprint_ignores_ui_map_iteration_order() {
    let build = |names: &[&str]| {
        let mut model = model(Status::Done);
        for name in names {
            model.ui.insert(
                ItemId((*name).into()),
                ItemUi {
                    expanded: Some(true),
                    ..Default::default()
                },
            );
        }
        model
    };
    let disclosure = RowDisclosure::default();
    let names = ["a", "b", "c", "d", "e", "f", "g", "h"];
    let reversed: Vec<_> = names.iter().rev().copied().collect();
    assert_eq!(
        fingerprint(&build(&names), &disclosure, false, None, 0),
        fingerprint(&build(&reversed), &disclosure, false, None, 0)
    );
}

#[test]
fn only_quiescent_rows_settle() {
    assert!(row_settles_statically(&model(Status::Done)));
    assert!(!row_settles_statically(&model(Status::Running)));

    let mut fresh = model(Status::Done);
    fresh.ui.insert(
        ItemId("tool".into()),
        ItemUi {
            arrived: Some(Instant::now()),
            ..Default::default()
        },
    );
    assert!(!row_settles_statically(&fresh));

    let mut gap = model(Status::Done);
    gap.row = Row::Gap;
    assert!(!row_settles_statically(&gap));
}
