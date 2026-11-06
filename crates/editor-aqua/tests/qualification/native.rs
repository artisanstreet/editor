use super::*;
use crate::core::{Core, EditorFiles};
use crate::shell::{host::ShellResources, root::RootViews};
use aqua::prelude::{ObservedWindowState, WindowOptions};
use aqua::{Aqua, AquaConfig};

#[test]
fn qualification_frames_never_initialize_native_notifications() {
    let directory = tempfile::tempdir().unwrap();
    let core = Core::for_tests(EditorFiles {
        dir: directory.path().to_owned(),
    })
    .unwrap();
    let resources = ShellResources::new(1.0).unwrap();
    let mut app = ShellApplication::for_qualification(core, resources, RootViews::default());
    let mut runtime = Aqua::new(AquaConfig {
        headless: true,
        ..AquaConfig::default()
    })
    .unwrap();
    let id = AquaWindowId::new(1).unwrap();
    runtime.add_headless_window(ObservedWindowState::new(
        id,
        "qualification".into(),
        WindowOptions::default().inner_size,
    ));
    runtime.dispatch_start(&mut app, id);
    for sequence in 1..=3 {
        runtime.dispatch_frame_completed(&mut app, id, sequence);
    }
    assert!(app.notifications.is_none());
    assert!(!app.notifications_started);
}
