use aqua_core::Rect;
use aqua_text::document::SelectionDocumentId;
use aqua_text::{SharedFonts, SharedRuns};
use aqua_ui::collection_runtime::{CollectionViewport, FollowState};
use aqua_ui::virtualization::RowRange;
use artisan_api::{RunId, ThreadId};
use editor_aqua::conversation::controller::Conversation;
use editor_aqua::conversation::fold::Row;
use editor_aqua::conversation::presentation::{MarkdownOwners, PresentationScope};
use editor_aqua::conversation::view::transcript::TranscriptObservations;
use editor_aqua::conversation::viewport::{Fetch, Input};
use editor_aqua::core::{Core, EditorFiles, ForgeClient, HostConfig, HostId, HostKind};

#[test]
fn a_work_only_tail_keeps_loading_history_without_waiting_for_text() {
    let directory = tempfile::tempdir().unwrap();
    let mut core = Core::for_tests(EditorFiles {
        dir: directory.path().to_owned(),
    })
    .unwrap();
    let config = HostConfig {
        id: HostId("work-only-host".into()),
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
    core.add_test_host(&config, client);
    let mut conversation = Conversation::new(&core);
    conversation.open_thread(&mut core, config.id.clone(), ThreadId::from("thread"));
    conversation.viewport(&core, Input::Loaded {
        fetch: Fetch::Tail,
        first: Some(50),
        last: Some(60),
        more: true,
    });
    conversation.rows = vec![
        (
            Row::Work {
                run: Some(RunId::from("done")),
                segment: 0,
                entries: Vec::new(),
                unloaded: Vec::new(),
            },
            1,
        ),
        (Row::Status(RunId::from("live")), 1),
    ];
    assert!(!conversation.view.as_ref().unwrap().viewport.in_flight);
    let scope = PresentationScope::capture(&conversation, &core).unwrap();
    let app = aqua::UiApp::new(SharedFonts::new(), SharedRuns::new());
    let markdown = MarkdownOwners::new(SelectionDocumentId(900));
    TranscriptObservations::default()
        .observe_viewport(
            &mut conversation,
            &core,
            &app,
            &markdown,
            &scope,
            CollectionViewport {
                visible: RowRange { start: 0, end: 2 },
                mounted: RowRange { start: 0, end: 2 },
                scroll_y: 0.0,
                viewport_height: 600.0,
                distance_from_start: 0.0,
                distance_from_end: 0.0,
                follow_state: FollowState::Tail { is_following: true },
            },
            Rect {
                x: 0.0,
                y: 0.0,
                w: 800.0,
                h: 600.0,
            },
        )
        .unwrap();
    assert!(
        conversation.view.as_ref().unwrap().viewport.in_flight,
        "a work-only page must request the preceding history"
    );
}
