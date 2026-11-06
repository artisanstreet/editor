use aqua::UiApp;
use aqua_core::{Point, PointerEvent, PointerKind};
use aqua_text::{SharedFonts, SharedRuns};
use aqua_ui::{
    element::Box as UiBox,
    input::{DragPayload, DragSnapshot},
};
use artisan_api::{
    ThreadId,
    tls::{Identity, fingerprint},
};
use editor_aqua::{
    core::{Core, DraggedThread, EditorFiles, ForgeClient, HostConfig, HostId, HostKind},
    sidebar::drag_preview::DragPreviewHost,
    ui::theme::ArtisanTheme,
};

#[test]
fn preview_tracks_offset_without_blocking_hit_testing_and_closes_on_drag_end() {
    let dir = tempfile::tempdir().unwrap();
    let mut core = Core::for_tests(EditorFiles {
        dir: dir.path().join("editor"),
    })
    .unwrap();
    let identity = Identity::generate("drag-preview").unwrap();
    let fingerprint = fingerprint(identity.certificate_der().unwrap().as_ref());
    let client = ForgeClient::new("127.0.0.1:1", &fingerprint, &identity).unwrap();
    let id = HostId("host".into());
    core.add_test_host(
        &HostConfig {
            id: id.clone(),
            name: "Forge".into(),
            address: "127.0.0.1:1".into(),
            fingerprint,
            kind: HostKind::Local,
        },
        client,
    );
    let instance = core.host(&id).unwrap().instance();
    let mut app = UiApp::new(SharedFonts::new(), SharedRuns::new());
    app.mount(|_| {
        UiBox::new()
            .width(300.0)
            .height(200.0)
            .child(
                UiBox::new()
                    .width(120.0)
                    .height(50.0)
                    .build()
                    .with_key("owner")
                    .focusable(true),
            )
            .build()
    })
    .unwrap();
    app.set_viewport(300.0, 200.0, 1.0);
    app.build_frame().unwrap();
    assert!(app.focus_key("owner"));
    let owner = app.tree().focused().unwrap();
    let mut snapshot = DragSnapshot {
        source: owner,
        payload: DragPayload::new(DraggedThread {
            host: id,
            thread: ThreadId::new(),
            title: "Thread".into(),
        }),
        pointer: PointerEvent {
            contact: 0,
            kind: PointerKind::Mouse,
            position: Point::new(80.0, 30.0),
            buttons: 1,
        },
        cursor_offset: Point::new(70.0, 20.0),
    };
    let mut preview = DragPreviewHost::open(
        &mut app,
        owner,
        instance,
        &snapshot,
        ArtisanTheme::resolved(),
    )
    .unwrap()
    .unwrap();
    app.build_frame().unwrap();
    let preview_root = preview.root(&app).unwrap();
    assert_eq!(
        aqua_ui::events::hit_test(app.tree(), Point::new(15.0, 15.0)),
        Some(owner)
    );
    snapshot.pointer.position = Point::new(35.0, 5.0);
    assert!(
        preview
            .sync(&mut app, Some(instance), Some(&snapshot))
            .unwrap()
    );
    app.build_frame().unwrap();
    let bounds = app.tree().visual_rect(preview_root).unwrap();
    assert_eq!(bounds.x, -35.0);
    assert_eq!(bounds.y, -15.0);
    app.build_frame().unwrap();
    assert_eq!(app.tree().visual_rect(preview_root).unwrap(), bounds);
    assert!(!preview.sync(&mut app, Some(instance), None).unwrap());
    assert!(preview.root(&app).is_none());
    assert_eq!(app.tree().focused(), Some(owner));
}
