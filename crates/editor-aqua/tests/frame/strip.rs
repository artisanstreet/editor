use aqua::UiApp;
use aqua_text::{SharedFonts, SharedRuns};
use aqua_ui::element::{Box as UiBox, Key};
use aqua_ui::tree::{NodeKind, hash_key};
use artisan_api::{ProjectId, ThreadId};
use editor_aqua::frame::sidebar::Event;
use editor_aqua::frame::strip::{self, Mark, OUTLINE_CHEVRON, Tab};
use editor_aqua::sidebar::list::RowState;
use editor_aqua::ui::theme::ArtisanTheme;
use std::rc::Rc;

struct Placed {
    labels: Vec<(String, f32, f32)>,
    chevron: Option<(f32, f32)>,
}

fn layout(outline: Option<f32>) -> Placed {
    let fonts = SharedFonts::new();
    editor_aqua::ui::fonts::register_bundled_fonts(&fonts);
    let mut app = UiApp::new(fonts, SharedRuns::new());
    app.mount(move |_| {
        let tab = Tab {
            id: ThreadId::new(),
            project: ProjectId::new(),
            trail: Vec::new(),
            title: "Open PR list".to_owned(),
            location: Some("artisanstreet".to_owned()),
            url: None,
            mark: Mark::default(),
            state: RowState::Idle,
            active: true,
            outline,
        };
        let emit: Rc<dyn Fn(Event)> = Rc::new(|_| {});
        UiBox::new()
            .layout("row")
            .width(1200.0)
            .children(strip::strip(
                &ArtisanTheme::resolved(),
                &[tab],
                false,
                &emit,
            ))
            .build()
    })
    .unwrap();
    app.set_viewport(1200.0, 200.0, 1.0);
    app.build_frame().unwrap();
    let tree = app.tree();
    let labels = tree
        .traverse()
        .into_iter()
        .filter_map(|id| {
            let NodeKind::Text(text) = &tree.get(id)?.kind else {
                return None;
            };
            let rect = tree.rect(id)?;
            Some((text.text().to_owned(), rect.x, rect.right()))
        })
        .collect();
    let chevron_key = hash_key(Key::from(OUTLINE_CHEVRON));
    let chevron = tree
        .traverse()
        .into_iter()
        .find(|id| {
            tree.get(*id)
                .is_some_and(|node| node.key == Some(chevron_key))
        })
        .and_then(|id| tree.rect(id))
        .map(|rect| (rect.x, rect.right()));
    Placed { labels, chevron }
}

#[test]
fn the_outline_chevron_follows_the_title_without_squeezing_the_labels() {
    let plain = layout(None);
    let outlined = layout(Some(0.0));
    let width = |placed: &Placed, label: &str| {
        placed
            .labels
            .iter()
            .find(|(text, ..)| text == label)
            .map(|(_, left, right)| right - left)
            .unwrap()
    };
    for label in ["artisanstreet", "Open PR list"] {
        assert!(width(&plain, label) > 0.0);
        assert_eq!(width(&plain, label), width(&outlined, label), "{label}");
    }
    let title_right = outlined
        .labels
        .iter()
        .find(|(text, ..)| text == "Open PR list")
        .map(|(_, _, right)| *right)
        .unwrap();
    let (chevron_left, _) = outlined.chevron.unwrap();
    assert!(chevron_left >= title_right);
    assert!(chevron_left - title_right < 16.0);
}
