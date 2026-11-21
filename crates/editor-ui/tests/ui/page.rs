use super::*;
use aqua_core::SemanticNode;
use aqua_text::{SharedFonts, SharedRuns};

fn semantics(element: Element) -> Vec<SemanticNode> {
    let mut app = aqua::UiApp::new(SharedFonts::new(), SharedRuns::new());
    app.mount(move |_| element).unwrap();
    app.set_viewport(800.0, 600.0, 1.0);
    app.build_frame().unwrap();
    app.tree().emit_semantics()
}

#[test]
fn page_and_section_titles_are_headings() {
    let theme = ArtisanTheme::resolved();
    let page = UiBox::new()
        .child(header(&theme, "Settings", "Tune the editor").build())
        .child(section_shell(&theme, "Appearance", None, None, UiBox::new().build()).build())
        .build();
    let headings: Vec<_> = semantics(page)
        .into_iter()
        .filter_map(|node| match node.role {
            Role::Heading(level) => Some((level, node.label)),
            Role::Text
            | Role::PasswordField
            | Role::ComboBox
            | Role::Option
            | Role::Separator
            | Role::Tooltip
            | Role::Group
            | Role::Button
            | Role::Checkbox
            | Role::Switch
            | Role::RadioButton
            | Role::TextField
            | Role::List
            | Role::ListBox
            | Role::RadioGroup
            | Role::ListItem
            | Role::Tree
            | Role::TreeItem
            | Role::Table
            | Role::Row
            | Role::ColumnHeader
            | Role::Cell
            | Role::Slider
            | Role::ProgressIndicator
            | Role::TabList
            | Role::Tab
            | Role::Image
            | Role::ScrollRegion
            | Role::Dialog
            | Role::Menu
            | Role::MenuItem
            | Role::Link => None,
        })
        .collect();
    assert_eq!(
        headings,
        vec![(1, "Settings".to_owned()), (2, "Appearance".to_owned())]
    );
}
