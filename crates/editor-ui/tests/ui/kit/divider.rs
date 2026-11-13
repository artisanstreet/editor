use aqua_core::Size;
use aqua_reactive::Runtime;
use aqua_ui::binding::View;
use aqua_ui::element::{Box as UiBox, Key};
use aqua_ui::layout::NaiveMeasurer;
use aqua_ui::tree::{UiTree, hash_key};
use editor_ui::kit::Divider;
use editor_ui::theme::{ArtisanTheme, ThemeMode};

#[test]
fn a_rule_occupies_the_height_it_declares() {
    let runtime = Runtime::new();
    let mut tree = UiTree::new();
    let mut mounted = View::new(runtime.create_scope(), tree.command_sink());
    let theme = ArtisanTheme::for_mode(ThemeMode::Dark);
    let column = UiBox::new()
        .layout("column")
        .child(UiBox::new().height(1.0).build().with_key("above"))
        .child(Divider::Rule.build(&theme, 0.0))
        .child(UiBox::new().height(1.0).build().with_key("below"))
        .build();
    assert!(mounted.set_content(&mut tree, vec![column]).is_ok(), "the rule mounts");
    assert!(
        mounted
            .compute_layout(&mut tree, Size::new(200.0, 200.0), &NaiveMeasurer)
            .is_ok(),
        "the rule lays out"
    );
    let top = |key: &str| {
        tree.sole_node_with_key(hash_key(Key::from(key.to_owned())))
            .and_then(|node| tree.rect(node))
            .map(|rect| rect.y)
    };
    let (Some(above), Some(below)) = (top("above"), top("below")) else {
        panic!("both neighbours are laid out");
    };
    assert_eq!(
        below - above - 1.0,
        Divider::Rule.height(),
        "a virtualized list can reserve a rule's height without measuring it"
    );
}
