use artisan_api::{
    ContentId, ContentRef, Item, ItemId, ItemKind, NoticeKind, Status, TextContent, ThreadId,
};
use editor_aqua::conversation::content::{SourceField, SourceOwners};
use editor_aqua::core::{Core, EditorFiles, ForgeClient, HostConfig, HostId, HostKind};
use std::sync::Arc;

fn source(digit: char) -> ContentRef {
    ContentRef {
        id: ContentId(digit.to_string().repeat(64)),
        bytes: 10,
        lines: 1,
    }
}

fn notice(reference: ContentRef) -> Item {
    Item {
        id: ItemId("notice".into()),
        thread_id: ThreadId("thread".into()),
        position: 1,
        seq: 1,
        run_id: None,
        parent_id: None,
        status: Status::Done,
        created_at: 0,
        updated_at: 0,
        kind: ItemKind::Notice {
            kind: NoticeKind::Info,
            text: TextContent::Stored(reference),
        },
    }
}

#[test]
fn source_owner_updates_revision_and_drops_recreated_host() {
    let dir = tempfile::tempdir().unwrap();
    let mut core = Core::for_tests(EditorFiles {
        dir: dir.path().to_owned(),
    })
    .unwrap();
    let config = HostConfig {
        id: HostId("source-host".into()),
        name: "Forge".into(),
        address: "127.0.0.1:1".into(),
        fingerprint: "0".repeat(64),
        kind: HostKind::Remote,
    };
    let client = ForgeClient::new(
        &config.address,
        &config.fingerprint,
        core.identity.as_ref().unwrap(),
    )
    .unwrap();
    core.add_test_host(&config, client.clone());
    let instance = core.host(&config.id).unwrap().instance();
    let first = notice(source('1'));
    let mut owners = SourceOwners::default();
    assert!(owners.open(&core, &config.id, instance, &first, SourceField::Notice));
    assert_eq!(
        owners
            .get(&first.id, &SourceField::Notice)
            .unwrap()
            .window()
            .source(),
        &source('1')
    );
    let next = Arc::new(notice(source('2')));
    owners.retain(&core, |_| Some(Arc::clone(&next)));
    assert_eq!(
        owners
            .get(&first.id, &SourceField::Notice)
            .unwrap()
            .window()
            .source(),
        &source('2')
    );
    core.add_test_host(&config, client);
    owners.retain(&core, |_| Some(Arc::clone(&next)));
    assert!(owners.get(&first.id, &SourceField::Notice).is_none());
    assert!(!owners.open(&core, &config.id, instance, &next, SourceField::Notice));
}
