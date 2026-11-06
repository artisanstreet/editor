use std::{cell::RefCell, rc::Rc};

use aqua::{UiApp, prelude::DisplayItem};
use aqua_text::{SharedFonts, SharedRuns};
use aqua_ui::{
    element::{Box as UiBox, Key},
    style::Length,
    tree::hash_key,
};
use editor_aqua::ui::{
    tab_card::{GeometryCache, TabCard},
    theme::ArtisanTheme,
};

#[test]
fn tab_card_prepaints_measured_tab_before_paint_and_updates_after_layout() {
    let cache = Rc::new(RefCell::new(GeometryCache::default()));
    let mut app = UiApp::new(SharedFonts::new(), SharedRuns::new());
    app.mount(move |_| {
        TabCard {
            id: "card".to_owned(),
            tab: UiBox::new().width(80.0).height(20.0).build(),
            body: UiBox::new().width(180.0).height(60.0).build(),
            strip: None,
        }
        .build(ArtisanTheme::resolved(), cache)
    })
    .unwrap();
    app.set_viewport(320.0, 240.0, 1.0);
    app.build_frame().unwrap();
    let shell = |app: &mut UiApp| {
        app.tree_mut()
            .emit_scene()
            .into_iter()
            .filter_map(|(_, item)| match item {
                DisplayItem::GpuiPath { path, .. } => Some(path.commands_local.clone()),
                _ => None,
            })
            .nth(1)
            .expect("shell emitted on first frame")
    };
    let before = shell(&mut app);
    let tab_key = hash_key(Key::from("card-tab"));
    let tab = app
        .tree()
        .traverse()
        .into_iter()
        .find(|id| {
            app.tree()
                .get(*id)
                .is_some_and(|node| node.key == Some(tab_key))
        })
        .unwrap();
    let child = app.tree().get(tab).unwrap().children[0];
    let mut style = app.tree().get(child).unwrap().style.clone();
    style.width = Length::Px(120.0);
    app.tree_mut().set_style(child, style);
    app.build_frame().unwrap();
    assert_ne!(before, shell(&mut app));
}
