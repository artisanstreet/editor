use std::{cell::Cell, rc::Rc, time::Duration};

use aqua::{Instant, RuntimeInput, UiApp};
use aqua_text::{SharedFonts, SharedRuns};
use aqua_ui::{
    element::{Box as UiBox, Element},
    popup::PopupFocus,
};
use editor_aqua::ui::popover::{AnimatedPickerHost, PickerEntrance};

fn app() -> (UiApp, aqua_core::NodeId) {
    let mut app = UiApp::new(SharedFonts::new(), SharedRuns::new());
    app.mount(|_| {
        UiBox::new()
            .width(400.0)
            .height(300.0)
            .child(
                UiBox::new()
                    .width(80.0)
                    .height(30.0)
                    .build()
                    .with_key("owner")
                    .focusable(true),
            )
            .build()
    })
    .unwrap();
    app.set_viewport(400.0, 300.0, 1.0);
    app.build_frame().unwrap();
    assert!(app.focus_key("owner"));
    let owner = app.tree().focused().unwrap();
    (app, owner)
}

fn panel() -> Element {
    UiBox::new().width(120.0).height(80.0).build()
}

fn open(app: &mut UiApp, owner: aqua_core::NodeId, closed: Rc<Cell<usize>>) -> AnimatedPickerHost {
    AnimatedPickerHost::open_above(
        app,
        owner,
        panel,
        8.0,
        PopupFocus::Root,
        PickerEntrance::Slide,
        Duration::from_millis(100),
        Duration::from_millis(100),
        0.0,
        move |_| closed.set(closed.get() + 1),
    )
    .unwrap()
}

#[test]
fn disposing_during_entrance_cancels_motion_and_restores_focus() {
    let (mut app, owner) = app();
    let closed = Rc::new(Cell::new(0));
    let host = open(&mut app, owner, closed.clone());
    assert!(host.is_animating(&app));
    assert_ne!(app.tree().focused(), Some(owner));
    host.dispose(&mut app);
    assert_eq!(app.tree().focused(), Some(owner));
    assert_eq!(closed.get(), 1);
    assert!(app.host().motion().is_empty());
    assert!(app.host().service().is_empty());
    app.drive(RuntimeInput::Tick(Instant::from_secs_f64(1.0)));
    assert!(app.host().motion().is_empty());
    assert!(app.host().service().is_empty());
    assert_eq!(closed.get(), 1);
}

#[test]
fn scheduling_tracks_entrance_and_exit_only() {
    let (mut app, owner) = app();
    let mut host = open(&mut app, owner, Rc::new(Cell::new(0)));
    assert!(host.is_animating(&app));
    host.advance(&mut app, 0.1).unwrap();
    assert!(host.is_open(&app));
    assert!(!host.is_animating(&app));
    host.begin_close(&mut app, 0.2).unwrap();
    assert!(host.is_animating(&app));
    host.advance(&mut app, 0.31).unwrap();
    assert!(!host.is_open(&app));
    assert!(!host.is_animating(&app));
}

#[test]
fn live_content_refresh_survives_entrance_and_exit_frames() {
    let (mut app, owner) = app();
    let mut host = open(&mut app, owner, Rc::new(Cell::new(0)));
    host.advance(&mut app, 0.05).unwrap();
    let root = app
        .host()
        .popup_root(app.host().service().topmost().unwrap())
        .unwrap();
    let frame = |app: &UiApp| {
        let node = app.tree().get(root).unwrap();
        match &node.kind {
            aqua_ui::tree::NodeKind::Box(content) => {
                (content.paint().opacity, content.paint().transform)
            }
            _ => (node.paint().opacity, node.paint().transform),
        }
    };
    let before = frame(&app);
    let content = UiBox::new()
        .width(120.0)
        .height(80.0)
        .child(
            UiBox::new()
                .width(10.0)
                .height(10.0)
                .build()
                .with_key("live-content"),
        )
        .build();
    host.refresh_content(&mut app, content).unwrap();
    assert_eq!(frame(&app), before);
    let has_content = |app: &UiApp| {
        let key = aqua_ui::tree::hash_key(aqua_ui::element::Key::from("live-content"));
        app.tree().traverse().into_iter().any(|node| {
            app.tree()
                .get(node)
                .is_some_and(|record| record.key == Some(key))
        })
    };
    assert!(has_content(&app));
    host.advance(&mut app, 0.11).unwrap();
    assert!(has_content(&app));
    host.begin_close(&mut app, 0.2).unwrap();
    host.advance(&mut app, 0.25).unwrap();
    assert!(has_content(&app));
    host.advance(&mut app, 0.31).unwrap();
    assert!(!host.is_open(&app));
}

#[test]
fn direct_content_is_retained_without_a_factory_during_motion() {
    let (mut app, owner) = app();
    let content = UiBox::new()
        .width(120.0)
        .height(80.0)
        .child(
            UiBox::new()
                .width(10.0)
                .height(10.0)
                .build()
                .with_key("direct-content"),
        )
        .build();
    let mut host = AnimatedPickerHost::open_content(
        &mut app,
        owner,
        content,
        8.0,
        PopupFocus::Preserve,
        PickerEntrance::Slide,
        Duration::from_millis(100),
        Duration::from_millis(100),
        0.0,
        |_| {},
    )
    .unwrap();
    let root = host.root(&app).unwrap();
    let key = aqua_ui::tree::hash_key(aqua_ui::element::Key::from("direct-content"));
    let child = app
        .tree()
        .get(root)
        .unwrap()
        .children
        .iter()
        .copied()
        .find(|node| {
            app.tree()
                .get(*node)
                .is_some_and(|record| record.key == Some(key))
        })
        .unwrap();
    assert!(host.refresh(&mut app).is_err());
    host.advance(&mut app, 0.11).unwrap();
    assert_eq!(host.root(&app), Some(root));
    assert!(app.tree().get(child).is_some());
    host.begin_close(&mut app, 0.2).unwrap();
    host.advance(&mut app, 0.25).unwrap();
    assert!(app.tree().get(child).is_some());
    host.advance(&mut app, 0.31).unwrap();
    assert!(host.root(&app).is_none());
    assert!(app.tree().get(child).is_none());
}

#[test]
fn reopening_during_exit_reverses_from_the_displayed_frame() {
    let (mut app, owner) = app();
    let closed = Rc::new(Cell::new(0));
    let mut host = open(&mut app, owner, closed.clone());
    let root = host.root(&app).unwrap();
    host.advance(&mut app, 0.11).unwrap();
    host.begin_close(&mut app, 0.2).unwrap();
    host.advance(&mut app, 0.25).unwrap();
    assert!(host.is_closing());
    let before = app.tree().paint(root).unwrap();
    host.begin_open(&mut app, 0.25).unwrap();
    let after = app.tree().paint(root).unwrap();
    assert_eq!(before.opacity, after.opacity);
    assert_eq!(before.transform, after.transform);
    assert!(!host.is_closing());
    assert_eq!(host.root(&app), Some(root));
    host.advance(&mut app, 0.31).unwrap();
    assert!(host.is_open(&app));
    assert_eq!(closed.get(), 0);
    host.advance(&mut app, 0.36).unwrap();
    assert!(!host.is_animating(&app));
    assert_eq!(app.tree().focused(), Some(root));
    host.dispose(&mut app);
}

#[test]
fn disposing_stale_host_cannot_close_replacement_overlay() {
    let (mut app, owner) = app();
    let old_closed = Rc::new(Cell::new(0));
    let old = open(&mut app, owner, old_closed.clone());
    let old_root = old.root(&app).unwrap();
    let old_id = app.host().service().topmost().unwrap();
    app.close_popup(old_id);
    assert!(old.root(&app).is_none());
    let new_closed = Rc::new(Cell::new(0));
    let replacement = open(&mut app, owner, new_closed.clone());
    let new_id = app.host().service().topmost().unwrap();
    assert_ne!(old_id, new_id);
    assert_ne!(replacement.root(&app), Some(old_root));
    old.dispose(&mut app);
    assert!(replacement.is_open(&app));
    assert_eq!(app.host().service().topmost(), Some(new_id));
    assert_eq!(old_closed.get(), 1);
    assert_eq!(new_closed.get(), 0);
    assert!(replacement.is_animating(&app));
    replacement.dispose(&mut app);
    assert_eq!(app.tree().focused(), Some(owner));
    assert!(app.host().motion().is_empty());
}

#[test]
fn removed_owner_generation_does_not_own_replacement_popup() {
    let (mut app, owner) = app();
    let old_closed = Rc::new(Cell::new(0));
    let old = open(&mut app, owner, old_closed.clone());
    let root = app.tree().root().unwrap();
    assert!(app.tree_mut().remove(owner));
    let mut replacement_owner = None;
    for _ in 0..32 {
        let replacement_element = UiBox::new().width(80.0).height(30.0).build();
        let mut specs = aqua_ui::element::lower_specs(&replacement_element).unwrap();
        let index = app.tree().get(root).unwrap().children.len();
        let node = app
            .tree_mut()
            .insert_spec(root, index, specs.remove(0))
            .unwrap();
        if node.raw().index == owner.raw().index {
            replacement_owner = Some(node);
            break;
        }
    }
    let replacement_owner = replacement_owner.expect("removed owner slot must be reused");
    app.tree_mut().set_focusable(replacement_owner, true);
    app.build_frame().unwrap();
    app.drive(RuntimeInput::Tick(Instant::from_secs_f64(0.01)));
    assert_eq!(owner.raw().index, replacement_owner.raw().index);
    assert_ne!(owner, replacement_owner);
    assert!(app.tree_mut().set_focused(Some(replacement_owner)));
    let new_closed = Rc::new(Cell::new(0));
    let replacement = open(&mut app, replacement_owner, new_closed.clone());
    old.dispose(&mut app);
    assert!(replacement.is_open(&app));
    assert_eq!(old_closed.get(), 1);
    assert_eq!(new_closed.get(), 0);
    replacement.dispose(&mut app);
    assert_eq!(app.tree().focused(), Some(replacement_owner));
}

#[test]
fn menus_keep_their_surface_and_callback_until_the_exit_finishes() {
    let (mut app, owner) = app();
    let closed = Rc::new(Cell::new(0));
    let capture = closed.clone();
    let mut options = aqua_ui::popup::PopupOptions::menu();
    options.on_close = Some(Rc::new(move |_| capture.set(capture.get() + 1)));
    let mut host =
        AnimatedPickerHost::open_options(&mut app, owner, panel(), options, 0.0).unwrap();
    host.advance(&mut app, 0.25).unwrap();
    let root = host.root(&app).unwrap();
    host.begin_close(&mut app, 0.25).unwrap();
    host.advance(&mut app, 0.30).unwrap();
    assert!(host.is_open(&app));
    assert_eq!(closed.get(), 0);
    let paint = app.tree().paint(root).unwrap();
    assert!(paint.opacity.unwrap() > 0.0 && paint.opacity.unwrap() < 1.0);
    assert!(paint.transform.unwrap().a > 0.99 && paint.transform.unwrap().a < 1.0);
    host.advance(&mut app, 0.41).unwrap();
    assert!(!host.is_open(&app));
    assert_eq!(closed.get(), 1);
}
