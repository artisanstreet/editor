use aqua::UiApp;
use aqua_core::{NodeId, Point, Rect};
use aqua_text::{SharedFonts, SharedRuns};
use aqua_ui::element::{Box as UiBox, Key};
use aqua_ui::events::hit_test;
use aqua_ui::style::Length;
use aqua_ui::tree::{UiTree, hash_key};
use editor_aqua::conversation::view::thread_frame;
use editor_aqua::ui::kit::surface::drawer;
use editor_aqua::ui::theme::ArtisanTheme;

const WIDTH: f32 = 900.0;
const HEIGHT: f32 = 600.0;
const PANEL: f32 = 320.0;

fn keyed(tree: &UiTree, key: &str) -> NodeId {
    let hash = hash_key(Key::from(key));
    tree.traverse()
        .into_iter()
        .find(|id| tree.get(*id).is_some_and(|node| node.key == Some(hash)))
        .unwrap()
}

fn within(tree: &UiTree, mut node: NodeId, ancestor: NodeId) -> bool {
    loop {
        if node == ancestor {
            return true;
        }
        match tree.get(node).and_then(|record| record.parent) {
            Some(parent) => node = parent,
            None => return false,
        }
    }
}

fn frame(progress: f32) -> UiApp {
    let mut app = UiApp::new(SharedFonts::new(), SharedRuns::new());
    app.mount(move |_| {
        let theme = ArtisanTheme::resolved();
        let composer = UiBox::new()
            .width(Length::Pct(100.0))
            .height(120.0)
            .build()
            .with_key("composer");
        let panel = drawer::build(&theme, PANEL, progress, Vec::new()).with_key("drawer");
        UiBox::new()
            .width(WIDTH)
            .height(HEIGHT)
            .child(thread_frame(
                UiBox::new().build(),
                Vec::new(),
                Some(composer),
                Some(panel),
                WIDTH,
                1.0,
            ))
            .build()
    })
    .unwrap();
    app.set_viewport(WIDTH, HEIGHT, 1.0);
    app.build_frame().unwrap();
    app
}

fn sheet(tree: &UiTree) -> Rect {
    let clip = keyed(tree, "drawer");
    let sheet = tree.get(clip).unwrap().children[0];
    tree.visual_rect(sheet).unwrap()
}

#[test]
fn the_open_drawer_runs_the_full_height_of_the_card_over_the_composer() {
    let app = frame(1.0);
    let tree = app.tree();
    let clip = tree.rect(keyed(tree, "drawer")).unwrap();
    assert_eq!((clip.x, clip.y, clip.bottom()), (0.0, 0.0, HEIGHT));
    let sheet = sheet(tree);
    assert!(sheet.x < 0.0, "the sheet tucks under the card's left edge");
    assert_eq!(sheet.right(), PANEL);
    assert_eq!((sheet.y, sheet.bottom()), (0.0, HEIGHT));
    let composer = tree.rect(keyed(tree, "composer")).unwrap();
    let probe = Point::new(PANEL / 2.0, composer.y + composer.h / 2.0);
    assert!(composer.contains(probe));
    let hit = hit_test(tree, probe).unwrap();
    assert!(within(tree, hit, keyed(tree, "drawer")));
}

#[test]
fn the_closed_drawer_hides_behind_the_card_edge() {
    let app = frame(0.0);
    assert!(sheet(app.tree()).right() <= 0.0);
}
