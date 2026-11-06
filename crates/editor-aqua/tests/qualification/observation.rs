use editor_aqua::conversation::presentation::PresentationScope;
use editor_aqua::core::{Core, EditorFiles, ForgeClient, HostConfig, HostId, HostKind};
use editor_aqua::qualification::observation::{PaintCollector, PaintedRow};

#[test]
fn paint_generation_tracks_completed_frames_and_revokes_replaced_scopes() {
    let directory = tempfile::tempdir().unwrap();
    let mut core = Core::for_tests(EditorFiles {
        dir: directory.path().to_owned(),
    })
    .unwrap();
    let config = HostConfig {
        id: HostId("qualification".into()),
        name: "Qualification".into(),
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
    core.host_mut(&config.id).unwrap().test_stop_connection();
    let scope = PresentationScope {
        host: config.id.clone(),
        instance: core.host(&config.id).unwrap().instance(),
        thread: "thread".into(),
        agent: None,
        generation: 1,
    };
    let row = || PaintedRow {
        key: "row".into(),
        item: Some("item".into()),
        item_seq: Some(1),
        observed_seq: Some(2),
        bounds_bits: [0; 4],
        clip_bits: [0; 4],
        revealed: true,
    };
    let mut collector = PaintCollector::default();
    assert!(collector.arm(Some(scope.clone()), 7, 4));
    collector.record(5, row());
    assert!(collector.current(&scope).unwrap().rows.is_empty());
    collector.begin_frame(&scope, 10);
    collector.record(4, row());
    assert!(collector.current(&scope).unwrap().rows.is_empty());
    collector.record(5, row());
    let captured = collector.current(&scope).unwrap();
    assert_eq!(captured.completed_frame, Some(10));
    assert_eq!(captured.rows.len(), 1);
    assert_eq!(captured.rows[0].item_seq, Some(1));
    assert_eq!(captured.rows[0].observed_seq, Some(2));
    collector.begin_frame(&scope, 11);
    assert!(collector.current(&scope).unwrap().rows.is_empty());
    collector.record(5, row());
    collector.arm(Some(scope.clone()), 8, 5);
    assert_eq!(collector.current(&scope).unwrap().completed_frame, None);
    let mut replacement = scope.clone();
    replacement.generation += 1;
    assert!(collector.current(&replacement).is_none());
    collector.begin_frame(&replacement, 12);
    collector.record(5, row());
    assert!(collector.current(&scope).is_none());
    assert!(!collector.arm(None, 9, 5));
}
