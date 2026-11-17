use aqua::UiApp;
use aqua::prelude::DisplayItem;
use aqua_core::NodeId;
use aqua_core::{InputEvent, Point, PointerButton, PointerEvent, PointerKind};
use aqua_text::{SharedFonts, SharedRuns};
use aqua_ui::element::{Element, Key};
use aqua_ui::tree::{NodeKind, hash_key};

pub fn ui_app(width: f32, height: f32) -> UiApp {
    let mut app = UiApp::new(SharedFonts::new(), SharedRuns::new());
    app.set_viewport(width, height, 1.0);
    app
}

pub fn mount(app: &mut UiApp, element: Element) {
    app.mount(|_| element).expect("mount the element");
    app.layout().expect("lay out the mounted element");
}

pub fn mounted(width: f32, height: f32, element: Element) -> UiApp {
    let mut app = ui_app(width, height);
    mount(&mut app, element);
    app
}

pub fn node_with_key(app: &UiApp, key: &str) -> NodeId {
    let hash = hash_key(Key::from(key));
    app.tree()
        .traverse()
        .into_iter()
        .find(|node| {
            app.tree()
                .get(*node)
                .is_some_and(|record| record.key == Some(hash))
        })
        .unwrap_or_else(|| panic!("no mounted node has the key {key:?}"))
}

pub fn click(app: &mut UiApp, key: &str) {
    let node = node_with_key(app, key);
    let bounds = app
        .tree()
        .visual_rect(node)
        .unwrap_or_else(|| panic!("the node keyed {key:?} has no bounds"));
    let down = PointerEvent {
        contact: 1,
        kind: PointerKind::Mouse,
        position: Point::new(bounds.x + bounds.w / 2.0, bounds.y + bounds.h / 2.0),
        buttons: 1,
    };
    aqua_ui::events::dispatch_input(
        app.tree_mut(),
        &InputEvent::PointerDown(down, PointerButton::Primary),
    );
    aqua_ui::events::dispatch_input(
        app.tree_mut(),
        &InputEvent::PointerUp(PointerEvent { buttons: 0, ..down }, PointerButton::Primary),
    );
}

pub fn painted_texts(app: &UiApp) -> Vec<String> {
    app.tree()
        .traverse()
        .into_iter()
        .filter_map(|node| {
            let NodeKind::Text(content) = &app.tree().get(node)?.kind else {
                return None;
            };
            Some(content.text().to_owned())
        })
        .collect()
}

pub fn text_opacity(app: &mut UiApp, text: &str) -> f32 {
    let node = app
        .tree()
        .traverse()
        .into_iter()
        .find(|node| {
            app.tree().get(*node).is_some_and(
                |record| matches!(&record.kind, NodeKind::Text(content) if content.text() == text),
            )
        })
        .unwrap_or_else(|| panic!("{text:?} is not mounted"));
    let mut opacities = Vec::new();
    for (owner, item) in app.tree_mut().emit_scene() {
        if let DisplayItem::PushOpacity(opacity) = item {
            opacities.push(opacity);
        } else if matches!(item, DisplayItem::PopOpacity) {
            opacities
                .pop()
                .expect("invariant: every popped opacity was pushed");
        } else if let DisplayItem::Text { opacity, .. } = item
            && owner == node
        {
            return opacities.iter().product::<f32>() * opacity;
        }
    }
    panic!("{text:?} is mounted but was not emitted into the scene")
}
