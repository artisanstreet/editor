use super::RootViews;
use crate::{
    conversation::{presentation::PresentationScope, view::factory::BlobAction},
    core::{Core, EditorFiles, ForgeClient, HostConfig, HostId, HostKind, Screen},
    shell::host::{Shell, ShellResources},
};
use artisan_api::{BlobHash, Item, ItemId, ItemKind, Status, ThreadId};
use std::sync::Arc;

#[test]
fn blob_preview_requires_current_item_revision_hash_and_host_lifetime() {
    let directory = tempfile::tempdir().unwrap();
    let files = EditorFiles {
        dir: directory.path().to_owned(),
    };
    let mut resources = ShellResources::new(1.0).unwrap();
    let (mut shell, _) = Shell::new(files.clone(), resources.images.clone());
    shell.core = Core::for_tests(files).unwrap();
    let config = HostConfig {
        id: HostId("blob-host".into()),
        name: "Forge".into(),
        address: "127.0.0.1:1".into(),
        fingerprint: "0".repeat(64),
        kind: HostKind::Remote,
    };
    let client = ForgeClient::new(
        &config.address,
        &config.fingerprint,
        shell.core.identity.as_ref().unwrap(),
    )
    .unwrap();
    shell.core.add_test_host(&config, client.clone());
    let thread = ThreadId("blob-thread".into());
    shell.core.navigate(Screen::Thread {
        host: config.id.clone(),
        thread: thread.clone(),
    });
    shell.entry.sync(&shell.core);
    let workspace = shell.entry.workspace_mut().unwrap();
    workspace
        .conversation
        .open_thread(&mut shell.core, config.id.clone(), thread.clone());
    let id = ItemId("blob-item".into());
    let hash = BlobHash("image-hash".into());
    workspace
        .conversation
        .view
        .as_mut()
        .unwrap()
        .context
        .insert(
            id.clone(),
            Arc::new(Item {
                id: id.clone(),
                thread_id: thread,
                position: 1,
                seq: 3,
                run_id: None,
                parent_id: None,
                status: Status::Done,
                created_at: 0,
                updated_at: 0,
                kind: ItemKind::Image {
                    blob: hash.clone(),
                    caption: None,
                },
            }),
        );
    let scope = PresentationScope::capture(&workspace.conversation, &shell.core).unwrap();
    let image = resources
        .images
        .insert(aqua_media::decode::DecodedImage::from_rgba(1, 1, vec![10, 20, 30, 255]).unwrap())
        .unwrap();
    let action = |hash| BlobAction::Open {
        hash,
        image: image.clone(),
        opener_key: "image-opener".into(),
    };
    let mut root = RootViews::default();
    assert!(!root.dispatch_blob(
        &mut shell,
        &mut resources.ui,
        scope.clone(),
        id.clone(),
        2,
        action(hash.clone())
    ));
    assert!(!root.dispatch_blob(
        &mut shell,
        &mut resources.ui,
        scope.clone(),
        id.clone(),
        3,
        action(BlobHash("other".into()))
    ));
    assert!(root.image_preview.is_none());
    assert!(root.dispatch_blob(
        &mut shell,
        &mut resources.ui,
        scope.clone(),
        id.clone(),
        3,
        action(hash.clone())
    ));
    let preview = root.image_preview.take().unwrap();
    assert_eq!(preview.view.opener_key(), Some("image-opener"));
    assert!(preview.owner.is_none());
    shell.core.remove_host(&config.id).unwrap();
    shell.core.add_test_host(&config, client);
    assert!(!root.dispatch_blob(&mut shell, &mut resources.ui, scope, id, 3, action(hash)));
    assert!(root.image_preview.is_none());
    assert!(!preview.view.is_live(&shell.core));
}
