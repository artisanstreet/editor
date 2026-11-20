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
    click_node(app, node, key);
}

pub fn click_text(app: &mut UiApp, text: &str) {
    let root = app.tree().root().expect("a mounted tree has a root");
    let node = text_under(app, root, text).unwrap_or_else(|| panic!("{text:?} is not shown"));
    click_node(app, node, text);
}

fn text_under(app: &UiApp, under: NodeId, text: &str) -> Option<NodeId> {
    let tree = app.tree();
    let mut pending = vec![under];
    while let Some(node) = pending.pop() {
        let record = tree
            .get(node)
            .unwrap_or_else(|| panic!("invariant: traversed node {node:?} exists"));
        if matches!(&record.kind, NodeKind::Text(content) if content.text() == text) {
            return Some(node);
        }
        pending.extend(record.children.iter().copied());
    }
    None
}

fn click_node(app: &mut UiApp, node: NodeId, name: &str) {
    let bounds = app
        .tree()
        .visual_rect(node)
        .unwrap_or_else(|| panic!("{name:?} has no bounds"));
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

struct TwentyRows;

impl aqua_ui::collection::CollectionSource for TwentyRows {
    fn len(&self) -> usize {
        20
    }
    fn key(&self, index: usize) -> Key {
        index.into()
    }
    fn layout(&self, available_width: f32) -> aqua_ui::collection::CollectionLayout {
        aqua_ui::collection::CollectionLayout {
            content_width: available_width,
            column_widths: Vec::new(),
        }
    }
    fn header(&self, _: &aqua_ui::collection::CollectionLayout) -> Option<Element> {
        None
    }
    fn row(&self, _: usize, _: &aqua_ui::collection::CollectionLayout) -> Element {
        aqua_ui::element::Box::new()
            .style(
                aqua_ui::style::Style::new()
                    .height(aqua_ui::style::px(20.0))
                    .flex_shrink(0.0),
            )
            .build()
    }
    fn role(&self) -> aqua_ui::collection::CollectionRole {
        aqua_ui::collection::CollectionRole::List
    }
    fn validate(&self) -> Result<(), aqua_ui::collection::CollectionError> {
        Ok(())
    }
}

pub fn twenty_row_transcript() -> UiApp {
    let mut app = ui_app(100.0, 100.0);
    app.mount(|_| {
        Element::collection(aqua_ui::collection::CollectionDescriptor::new(
            std::rc::Rc::new(TwentyRows),
            vec![
                aqua_ui::style::Style::new()
                    .width(aqua_ui::style::px(100.0))
                    .height(aqua_ui::style::px(100.0)),
            ],
            true,
            20.0,
            2,
        ))
        .with_key("conversation-transcript")
    })
    .expect("mount the transcript");
    app.build_frame().expect("lay out the transcript");
    app
}
