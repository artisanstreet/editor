use aqua::prelude::{AquaWindowId, ObservedWindowState, WindowEvent, WindowOptions};
use aqua::{Aqua, AquaConfig};
use aqua_protocol::{DialogOutcome, RequestId};
use editor_aqua::core::{Core, EditorFiles};
use editor_aqua::shell::{application::ShellApplication, host::ShellResources, root::RootViews};
use std::{cell::RefCell, rc::Rc};

pub(super) fn started() -> (Aqua, ShellApplication<RootViews>, AquaWindowId, tempfile::TempDir) {
    let directory = tempfile::tempdir().unwrap();
    let files = EditorFiles {
        dir: directory.path().to_owned(),
    };
    let resources = ShellResources::new(1.0).unwrap();
    let mut app = ShellApplication::new(files.clone(), resources, RootViews::default());
    let mut core = Core::for_tests(files).unwrap();
    core.set_image_assets(app.shell.images().clone());
    app.shell.core = core;
    app.shell.entry.sync(&app.shell.core);
    let mut aqua = Aqua::new(AquaConfig {
        headless: true,
        ..AquaConfig::default()
    })
    .unwrap();
    let id = AquaWindowId::new(1).unwrap();
    let mut size = WindowOptions::default().inner_size;
    size.width = 800.0;
    size.height = 600.0;
    aqua.add_headless_window(ObservedWindowState::new(id, "test".into(), size));
    aqua.dispatch_start(&mut app, id);
    (aqua, app, id, directory)
}

fn recorder() -> (Rc<RefCell<Vec<DialogOutcome>>>, impl FnOnce(DialogOutcome) + 'static) {
    let seen = Rc::new(RefCell::new(Vec::new()));
    let sink = seen.clone();
    (seen, move |outcome| sink.borrow_mut().push(outcome))
}

#[test]
fn a_dialog_outcome_reaches_its_route_once() {
    let (mut aqua, mut app, id, _directory) = started();
    let (seen, route) = recorder();
    app.shell.dialogs.route(RequestId(41), route);
    aqua.dispatch_dialog(&mut app, id, RequestId(41), DialogOutcome::Dismissed);
    aqua.dispatch_dialog(&mut app, id, RequestId(41), DialogOutcome::Dismissed);
    assert_eq!(*seen.borrow(), vec![DialogOutcome::Dismissed]);
}

#[test]
fn routes_are_distinguished_by_request() {
    let (mut aqua, mut app, id, _directory) = started();
    let (first, first_route) = recorder();
    let (second, second_route) = recorder();
    app.shell.dialogs.route(RequestId(1), first_route);
    app.shell.dialogs.route(RequestId(2), second_route);
    aqua.dispatch_dialog(&mut app, id, RequestId(2), DialogOutcome::Failed("no".into()));
    assert!(first.borrow().is_empty());
    assert_eq!(*second.borrow(), vec![DialogOutcome::Failed("no".into())]);
    assert!(app.shell.dialogs.complete(RequestId(1), DialogOutcome::Dismissed));
    assert_eq!(first.borrow().len(), 1);
}

#[test]
fn destroying_the_window_drops_pending_routes() {
    let (mut aqua, mut app, id, _directory) = started();
    let (seen, route) = recorder();
    app.shell.dialogs.route(RequestId(7), route);
    aqua.dispatch_window_event(&mut app, id, &WindowEvent::Destroyed(id));
    assert!(!app.shell.dialogs.complete(RequestId(7), DialogOutcome::Dismissed));
    assert!(seen.borrow().is_empty());
}
