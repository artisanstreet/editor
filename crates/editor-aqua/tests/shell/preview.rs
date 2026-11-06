use super::RootViews;
use aqua_ui::element::Box as UiBox;

#[test]
fn preview_focus_wraps_within_the_overlay_and_recovers_from_outside_focus() {
    let mut resources = crate::shell::host::ShellResources::new(1.0).unwrap();
    let ui = &mut resources.ui;
    ui.set_viewport(600.0, 400.0, 1.0);
    ui.mount(|_| {
        UiBox::new()
            .child(UiBox::new().build().with_key("outside").focusable(true))
            .child(
                UiBox::new()
                    .child(UiBox::new().build().with_key("previous").focusable(true))
                    .child(UiBox::new().build().with_key("next").focusable(true))
                    .build()
                    .with_key("image-preview")
                    .focusable(true),
            )
            .build()
    })
    .unwrap();
    ui.build_frame().unwrap();
    assert!(ui.focus_key("image-preview"));
    let root = ui.tree().focused().unwrap();
    assert!(RootViews::cycle_preview_focus(ui, false));
    let previous = ui.tree().focused().unwrap();
    assert!(ui.focus_key("previous"));
    assert_eq!(ui.tree().focused(), Some(previous));
    assert!(RootViews::cycle_preview_focus(ui, false));
    let next = ui.tree().focused().unwrap();
    assert!(ui.focus_key("next"));
    assert_eq!(ui.tree().focused(), Some(next));
    assert!(RootViews::cycle_preview_focus(ui, false));
    assert_eq!(ui.tree().focused(), Some(root));
    assert!(RootViews::cycle_preview_focus(ui, true));
    assert_eq!(ui.tree().focused(), Some(next));
    assert!(ui.focus_key("outside"));
    assert!(RootViews::cycle_preview_focus(ui, false));
    assert_eq!(ui.tree().focused(), Some(root));
    assert!(ui.focus_key("outside"));
    assert!(RootViews::cycle_preview_focus(ui, true));
    assert_eq!(ui.tree().focused(), Some(next));
}

#[test]
fn preview_root_is_found_while_mounted_and_gone_once_a_rebuild_drops_it() {
    let mut resources = crate::shell::host::ShellResources::new(1.0).unwrap();
    let ui = &mut resources.ui;
    ui.set_viewport(600.0, 400.0, 1.0);
    let with_preview = || {
        UiBox::new()
            .child(UiBox::new().build().with_key("content"))
            .child(UiBox::new().build().with_key("image-preview"))
            .build()
    };
    ui.mount(|_| with_preview()).unwrap();
    ui.build_frame().unwrap();
    assert!(RootViews::preview_root(ui).is_some());
    ui.refresh(vec![
        UiBox::new()
            .child(UiBox::new().build().with_key("content"))
            .build(),
    ])
    .unwrap();
    ui.build_frame().unwrap();
    assert!(RootViews::preview_root(ui).is_none());
}
