use artisan_api::{Item, ItemId, ItemKind, Phase, Status, TextContent, ThreadId};
use editor_aqua::conversation::fold::{Row, Slot};
use editor_aqua::conversation::presentation::BodyKey;
use editor_aqua::core::{Core, EditorFiles, ForgeClient, HostConfig, HostId, HostKind, Screen};
use editor_aqua::shell::application::SectionViews;
use editor_aqua::shell::host::{Shell, ShellResources};
use editor_aqua::shell::root::RootViews;
use std::sync::Arc;

#[test]
fn mounted_root_prepares_only_realized_bodies_and_releases_sources_on_navigation() {
    let directory = tempfile::tempdir().unwrap();
    let files = EditorFiles {
        dir: directory.path().to_owned(),
    };
    let mut resources = ShellResources::new(1.0).unwrap();
    let (mut shell, _) = Shell::new(files.clone(), resources.images.clone());
    shell.core = Core::for_tests(files).unwrap();
    shell.core.set_image_assets(resources.images.clone());
    let config = HostConfig {
        id: HostId("root-host".into()),
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
    shell.core.add_test_host(&config, client);
    let thread = ThreadId("root-thread".into());
    shell.core.navigate(Screen::Thread {
        host: config.id.clone(),
        thread: thread.clone(),
    });
    shell.entry.sync(&shell.core);
    let workspace = shell.entry.workspace_mut().unwrap();
    workspace.show_sidebar = false;
    workspace
        .conversation
        .open_thread(&mut shell.core, config.id, thread.clone());
    workspace.sync_composer(&shell.core, &mut resources.ui);
    let document = workspace.selections.conversation;
    let markdown = workspace.markdown.clone();
    for index in 0..1000 {
        let id = ItemId(format!("reply-{index}"));
        let item = Arc::new(Item {
            id: id.clone(),
            thread_id: thread.clone(),
            position: index + 1,
            seq: index + 1,
            run_id: None,
            parent_id: None,
            status: Status::Done,
            created_at: 0,
            updated_at: 0,
            kind: ItemKind::AssistantMessage {
                text: TextContent::Inline(format!("Reply {index}.")),
                phase: Phase::Final,
            },
        });
        workspace
            .conversation
            .view
            .as_mut()
            .unwrap()
            .context
            .insert(id.clone(), item);
        workspace.conversation.rows.push((
            Row::Text {
                id,
                slot: Slot::Prose,
                block: 0,
                run: None,
            },
            1,
        ));
    }
    let id = aqua::prelude::AquaWindowId::new(1).unwrap();
    let mut size = aqua::prelude::WindowOptions::default().inner_size;
    size.width = 1000.0;
    size.height = 700.0;
    let window = aqua::prelude::ObservedWindowState::new(id, "root".into(), size);
    resources.ui.set_viewport(1000.0, 700.0, 1.0);
    let mut root = RootViews::default();
    let element = root.build(&mut shell, &mut resources.ui, &window).unwrap();
    resources.ui.mount(|_| element).unwrap();
    resources.ui.layout().unwrap();
    let key = |index| BodyKey {
        item: ItemId(format!("reply-{index}")),
        slot: Slot::Prose,
    };
    assert!(markdown.borrow().projected_range(&key(0), 0).is_some());
    assert!(markdown.borrow().projected_range(&key(999), 0).is_none());
    let sources: Vec<_> = resources
        .ui
        .tree()
        .traverse()
        .into_iter()
        .filter_map(|node| {
            let aqua_ui::tree::NodeKind::Text(text) = &resources.ui.tree().get(node)?.kind else {
                return None;
            };
            text.selection_source
                .as_ref()
                .filter(|binding| binding.document == document)
                .map(|binding| binding.slice.source)
        })
        .collect();
    assert!(!sources.is_empty());
    assert!(sources.len() < 100);
    let removed = ItemId("reply-0".into());
    let workspace = shell.entry.workspace_mut().unwrap();
    workspace
        .conversation
        .view
        .as_mut()
        .unwrap()
        .context
        .remove(&removed);
    workspace.conversation.rows.remove(0);
    let element = root.build(&mut shell, &mut resources.ui, &window).unwrap();
    resources.ui.refresh(vec![element]).unwrap();
    resources.ui.layout().unwrap();
    assert!(markdown.borrow().projected_range(&key(0), 0).is_none());
    assert!(markdown.borrow().projected_range(&key(1), 0).is_some());
    shell.core.navigate(Screen::Home);
    let element = root.build(&mut shell, &mut resources.ui, &window).unwrap();
    resources.ui.refresh(vec![element]).unwrap();
    assert!(markdown.borrow().projected_range(&key(1), 0).is_none());
    for source in sources {
        assert!(
            !resources
                .ui
                .unregister_selection_source_in(document, source)
        );
    }
}
