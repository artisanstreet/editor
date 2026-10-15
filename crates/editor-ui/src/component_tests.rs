//! Interaction and geometry checks against production components.
use crate::{button::Button, dialog::Dialog, media_frame::MediaFrame, theme::ArtisanTheme};
use gpui::{
    Context, InteractiveElement as _, IntoElement, ParentElement as _, Render, Styled as _,
    TestAppContext, Window, div, px,
};
fn press_key(cx: &mut gpui::VisualTestContext, key: &str) {
    cx.update(|window, cx| {
        window.draw(cx).clear(cx);
    });
    let keystroke = gpui::Keystroke::parse(key).expect("valid test key");
    cx.update(|window, cx| {
        window.dispatch_event(
            gpui::PlatformInput::KeyDown(gpui::KeyDownEvent {
                keystroke: keystroke.clone(),
                is_held: false,
                prefer_character_input: false,
            }),
            cx,
        );
        window.dispatch_event(
            gpui::PlatformInput::KeyUp(gpui::KeyUpEvent { keystroke }),
            cx,
        );
    });
    cx.run_until_parked();
}
struct Gallery {
    clicks: [usize; 2],
    dialog: bool,
    loaded: bool,
}
impl Render for Gallery {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = ArtisanTheme::resolved();
        let mut page = div()
            .size_full()
            .flex()
            .flex_col()
            .child(
                Button::text("first", theme, "First")
                    .debug_selector("gallery-first")
                    .on_activate(cx.listener(|this, _, _, cx| {
                        this.clicks[0] += 1;
                        cx.notify();
                    })),
            )
            .child(
                Button::text("second", theme, "Second")
                    .debug_selector("gallery-second")
                    .on_activate(cx.listener(|this, _, _, cx| {
                        this.clicks[1] += 1;
                        cx.notify();
                    })),
            )
            .child(
                Button::text("open", theme, "Open dialog")
                    .debug_selector("gallery-open")
                    .on_activate(cx.listener(|this, _, _, cx| {
                        this.dialog = true;
                        cx.notify();
                    })),
            )
            .child(
                div().w(px(320.0)).child(
                    MediaFrame::new(theme, 1.6, if self.loaded { "Ready" } else { "Loading" })
                        .debug_selector("gallery-media"),
                ),
            );
        if self.dialog {
            page = page.child(
                Dialog::new(
                    "gallery-dialog",
                    theme,
                    "Dialog",
                    Button::text("inside", theme, "Inside"),
                )
                .on_dismiss({
                    let owner = cx.entity().downgrade();
                    move |_, _, cx| {
                        let _ = owner.update(cx, |this, cx| {
                            this.dialog = false;
                            cx.notify();
                        });
                    }
                }),
            );
        }
        page
    }
}
#[gpui::test]
fn controls_keep_distinct_focus_and_dialog_restores_opener(cx: &mut TestAppContext) {
    let (gallery, cx) = cx.add_window_view(|_, _| Gallery {
        clicks: [0, 0],
        dialog: false,
        loaded: false,
    });
    let first = cx.debug_bounds("gallery-first").unwrap();
    cx.simulate_click(first.center(), gpui::Modifiers::none());
    let first_focus = cx.update(|window, cx| window.focused(cx));
    let second = cx.debug_bounds("gallery-second").unwrap();
    cx.simulate_click(second.center(), gpui::Modifiers::none());
    let second_focus = cx.update(|window, cx| window.focused(cx));
    assert!(first_focus.is_some() && second_focus.is_some());
    assert_ne!(first_focus, second_focus);
    press_key(cx, "enter");
    gallery.read_with(cx, |gallery, _| assert_eq!(gallery.clicks, [1, 2]));
    let open = cx.debug_bounds("gallery-open").unwrap();
    cx.simulate_click(open.center(), gpui::Modifiers::none());
    cx.run_until_parked();
    assert!(gallery.read_with(cx, |gallery, _| gallery.dialog));
    press_key(cx, "escape");
    cx.run_until_parked();
    assert!(!gallery.read_with(cx, |gallery, _| gallery.dialog));
    press_key(cx, "enter");
    cx.run_until_parked();
    assert!(
        gallery.read_with(cx, |gallery, _| gallery.dialog),
        "Enter returns to the actual opener"
    );
}
#[gpui::test]
fn media_state_changes_preserve_the_outer_geometry(cx: &mut TestAppContext) {
    let (gallery, cx) = cx.add_window_view(|_, _| Gallery {
        clicks: [0, 0],
        dialog: false,
        loaded: false,
    });
    let before = cx.debug_bounds("gallery-media").unwrap();
    gallery.update(cx, |gallery, cx| {
        gallery.loaded = true;
        cx.notify();
    });
    cx.run_until_parked();
    assert_eq!(before, cx.debug_bounds("gallery-media").unwrap());
    assert_eq!(before.size.width, px(320.0));
    assert_eq!(before.size.height, px(200.0));
}

struct ImageGallery {
    image: Option<std::sync::Arc<gpui::RenderImage>>,
    error: bool,
}
impl Render for ImageGallery {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = ArtisanTheme::resolved();
        div()
            .size_full()
            .child(
                crate::attachment_tile::AttachmentTile::new(
                    "actual-attachment",
                    theme,
                    "test image",
                )
                .preview(self.image.clone(), None)
                .error(self.error.then(|| "Upload failed".into())),
            )
            .children(crate::image_preview::overlay(&theme, window, cx))
    }
}
#[gpui::test]
fn actual_decoded_attachment_preserves_geometry_and_opens_by_keyboard(cx: &mut TestAppContext) {
    let (gallery, cx) = cx.add_window_view(|_, _| ImageGallery {
        image: None,
        error: false,
    });
    cx.simulate_resize(gpui::size(px(480.), px(420.)));
    let loading = cx.debug_bounds("actual-attachment").unwrap();
    gallery.update(cx, |this, cx| {
        this.image = Some(std::sync::Arc::new(gpui::RenderImage::new(vec![
            image::Frame::new(image::RgbaImage::from_pixel(
                120,
                80,
                image::Rgba([255, 0, 0, 255]),
            )),
        ])));
        cx.notify();
    });
    cx.run_until_parked();
    assert_eq!(loading, cx.debug_bounds("actual-attachment").unwrap());
    cx.simulate_click(loading.center(), gpui::Modifiers::none());
    cx.run_until_parked();
    assert!(cx.debug_bounds("image-preview").is_some());
    press_key(cx, "escape");
    cx.run_until_parked();
    assert!(cx.debug_bounds("image-preview").is_none());
    press_key(cx, "enter");
    cx.run_until_parked();
    assert!(
        cx.debug_bounds("image-preview").is_some(),
        "focus returns to the image's keyboard action"
    );
    press_key(cx, "escape");
    gallery.update(cx, |this, cx| {
        this.error = true;
        cx.notify();
    });
    cx.run_until_parked();
    assert_eq!(loading, cx.debug_bounds("actual-attachment").unwrap());
}
struct KeyboardGallery {
    checked: bool,
    selected: u8,
}
impl Render for KeyboardGallery {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = ArtisanTheme::resolved();
        div()
            .flex()
            .flex_col()
            .size_full()
            .child(
                crate::switch::Switch::new(
                    "keyboard-switch",
                    theme,
                    crate::switch::SwitchSize::Default,
                    self.checked,
                    "Feature",
                )
                .debug_selector("keyboard-switch")
                .on_change({
                    let owner = cx.entity().downgrade();
                    move |checked, _, _, cx| {
                        let _ = owner.update(cx, |this, cx| {
                            this.checked = checked;
                            cx.notify();
                        });
                    }
                }),
            )
            .child(
                crate::select::Select::new(
                    "keyboard-select",
                    theme,
                    Some(self.selected),
                    vec![
                        crate::select::SelectEntry::item(1, "One"),
                        crate::select::SelectEntry::item(2, "Two").disabled(true),
                        crate::select::SelectEntry::item(3, "Three"),
                    ],
                )
                .debug_selector("keyboard-select")
                .on_change({
                    let owner = cx.entity().downgrade();
                    move |value, _, _, cx| {
                        let _ = owner.update(cx, |this, cx| {
                            this.selected = value;
                            cx.notify();
                        });
                    }
                }),
            )
    }
}
#[gpui::test]
fn switch_and_select_have_keyboard_equivalence_and_skip_disabled_options(cx: &mut TestAppContext) {
    let (gallery, cx) = cx.add_window_view(|_, _| KeyboardGallery {
        checked: false,
        selected: 1,
    });
    cx.simulate_resize(gpui::size(px(480.), px(420.)));
    let switch_center = cx.debug_bounds("keyboard-switch").unwrap().center();
    cx.simulate_click(switch_center, gpui::Modifiers::none());
    press_key(cx, "space");
    cx.run_until_parked();
    gallery.read_with(cx, |this, _| assert!(!this.checked));
    let select_center = cx.debug_bounds("keyboard-select-trigger").unwrap().center();
    cx.simulate_click(select_center, gpui::Modifiers::none());
    assert!(cx.debug_bounds("keyboard-select-popup").is_some());
    press_key(cx, "down");
    press_key(cx, "enter");
    cx.run_until_parked();
    gallery.read_with(cx, |this, _| assert_eq!(this.selected, 3));
    assert!(
        cx.debug_bounds("keyboard-select-popup").is_none(),
        "committing a choice must not reopen the popup on key release"
    );
    // The component owns keyboard activation, while pointer and accessibility
    // Click still open the same popup after a keyboard commit.
    cx.simulate_click(select_center, gpui::Modifiers::none());
    assert!(cx.debug_bounds("keyboard-select-popup").is_some());
    press_key(cx, "home");
    press_key(cx, "space");
    gallery.read_with(cx, |this, _| assert_eq!(this.selected, 1));
    assert!(cx.debug_bounds("keyboard-select-popup").is_none());
    press_key(cx, "space");
    assert!(cx.debug_bounds("keyboard-select-popup").is_some());
    press_key(cx, "escape");
    assert!(cx.debug_bounds("keyboard-select-popup").is_none());
}

struct NestedOverlays {
    dialog: bool,
    popup: bool,
}
impl Render for NestedOverlays {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = ArtisanTheme::resolved();
        let mut root = div().size_full().child(
            Button::text("open", theme, "Open")
                .debug_selector("nested-open")
                .on_activate(cx.listener(|this, _, _, cx| {
                    this.dialog = true;
                    cx.notify();
                })),
        );
        if self.dialog {
            root = root.child(
                Dialog::new(
                    "nested-dialog",
                    theme,
                    "Dialog",
                    Button::text("child", theme, "Child menu")
                        .debug_selector("nested-child")
                        .on_activate(cx.listener(|this, _, _, cx| {
                            this.popup = true;
                            cx.notify();
                        })),
                )
                .on_dismiss({
                    let owner = cx.entity().downgrade();
                    move |_, _, cx| {
                        let _ = owner.update(cx, |this, cx| {
                            this.dialog = false;
                            this.popup = false;
                            cx.notify();
                        });
                    }
                }),
            );
        }
        if self.popup {
            root = root.child(crate::popover::Popover::new(
                "nested-menu",
                "Menu",
                gpui::point(window.viewport_size().width / 2., px(120.)),
                Button::text("child", theme, "Menu item").debug_selector("nested-item"),
                {
                    let owner = cx.entity().downgrade();
                    move |_, cx| {
                        let _ = owner.update(cx, |this, cx| {
                            this.popup = false;
                            cx.notify();
                        });
                    }
                },
            ));
        }
        root
    }
}
#[gpui::test]
fn nested_overlay_ids_and_unmount_restore_the_actual_opener(cx: &mut TestAppContext) {
    let (gallery, cx) = cx.add_window_view(|_, _| NestedOverlays {
        dialog: false,
        popup: false,
    });
    cx.simulate_resize(gpui::size(px(480.), px(420.)));
    let open = cx.debug_bounds("nested-open").unwrap();
    cx.simulate_click(open.center(), gpui::Modifiers::none());
    cx.run_until_parked();
    let child = cx.debug_bounds("nested-child").unwrap();
    cx.simulate_click(child.center(), gpui::Modifiers::none());
    cx.run_until_parked();
    assert!(cx.debug_bounds("nested-item").is_some());
    press_key(cx, "escape");
    cx.run_until_parked();
    gallery.read_with(cx, |this, _| assert!(this.dialog && !this.popup));
    press_key(cx, "enter");
    cx.run_until_parked();
    gallery.read_with(cx, |this, _| assert!(this.popup));
    // Navigation/success can unmount both layers without invoking either dismiss callback.
    gallery.update(cx, |this, cx| {
        this.dialog = false;
        this.popup = false;
        cx.notify();
    });
    cx.run_until_parked();
    press_key(cx, "enter");
    cx.run_until_parked();
    gallery.read_with(cx, |this, _| assert!(this.dialog && !this.popup));
}

mod rich_reference;

struct CaptionGallery {
    maximized: bool,
    focus: gpui::FocusHandle,
}
impl Render for CaptionGallery {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .h(px(36.0))
            .flex()
            .track_focus(&self.focus)
            .tab_stop(false)
            .on_action(crate::focus_navigation::focus_next)
            .on_action(crate::focus_navigation::focus_previous)
            .child(crate::window_controls::WindowControls::new(
                ArtisanTheme::resolved(),
                self.maximized,
            ))
    }
}
#[gpui::test]
fn caption_state_and_keyboard_focus_preserve_geometry(cx: &mut TestAppContext) {
    cx.update(crate::focus_navigation::bind_actions);
    let (gallery, cx) = cx.add_window_view(|window, cx| CaptionGallery {
        maximized: false,
        focus: crate::focus_navigation::fixture_root_focus(window, cx),
    });
    let selectors = [
        "artisan-desktop-titlebar-minimize",
        "artisan-desktop-titlebar-maximize",
        "artisan-desktop-titlebar-close",
    ];
    let before = selectors.map(|id| cx.debug_bounds(id).unwrap());
    for bounds in before {
        assert_eq!(bounds.size, gpui::size(px(46.0), px(36.0)));
    }
    press_key(cx, "tab");
    let first = cx.update(|window, cx| window.focused(cx));
    press_key(cx, "tab");
    let second = cx.update(|window, cx| window.focused(cx));
    assert!(first.is_some() && second.is_some());
    assert_ne!(first, second);
    press_key(cx, "shift-tab");
    assert_eq!(cx.update(|window, cx| window.focused(cx)), first);
    press_key(cx, "tab");
    assert_eq!(cx.update(|window, cx| window.focused(cx)), second);
    gallery.update(cx, |gallery, cx| {
        gallery.maximized = true;
        cx.notify();
    });
    cx.run_until_parked();
    assert_eq!(before, selectors.map(|id| cx.debug_bounds(id).unwrap()));
}

struct PreviewOnly;
impl Render for PreviewOnly {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div().size_full().children(crate::image_preview::overlay(
            &ArtisanTheme::resolved(),
            window,
            cx,
        ))
    }
}
#[gpui::test]
fn replacing_and_closing_previews_release_reuploaded_textures(cx: &mut TestAppContext) {
    let (_, cx) = cx.add_window_view(|_, _| PreviewOnly);
    let image = || {
        std::sync::Arc::new(gpui::RenderImage::new(vec![image::Frame::new(
            image::RgbaImage::from_pixel(16, 12, image::Rgba([10, 20, 30, 255])),
        )]))
    };
    let first = image();
    let second = image();
    cx.update(|window, cx| {
        crate::image_preview::open(first.clone(), window, cx);
        window.draw(cx).clear(cx);
        assert!(window.has_image_atlas_entry(&first));
        // The owning cache evicts this image while the preview retains it.
        window.drop_image(first.clone()).unwrap();
        assert!(!window.has_image_atlas_entry(&first));
        window.refresh();
        window.draw(cx).clear(cx);
        assert!(window.has_image_atlas_entry(&first));
        crate::image_preview::open(second.clone(), window, cx);
    });
    cx.run_until_parked();
    cx.update(|window, cx| {
        window.draw(cx).clear(cx);
        assert!(!window.has_image_atlas_entry(&first));
        assert!(window.has_image_atlas_entry(&second));
        crate::image_preview::close(window, cx);
    });
    cx.run_until_parked();
    cx.update(|window, cx| {
        window.draw(cx).clear(cx);
        assert!(!window.has_image_atlas_entry(&second));
    });
}

#[gpui::test]
fn closing_a_preview_repaints_an_image_retained_by_another_window(cx: &mut TestAppContext) {
    let image = std::sync::Arc::new(gpui::RenderImage::new(vec![image::Frame::new(
        image::RgbaImage::from_pixel(16, 12, image::Rgba([10, 20, 30, 255])),
    )]));
    let preview = cx.add_window(|_, _| PreviewOnly);
    let attachment = cx.add_window(|_, _| ImageGallery {
        image: Some(image.clone()),
        error: false,
    });
    gpui::AnyWindowHandle::from(preview)
        .update(cx, |_, window, cx| {
            crate::image_preview::open(image.clone(), window, cx);
            window.draw(cx).clear(cx);
            assert!(window.has_image_atlas_entry(&image));
        })
        .unwrap();
    gpui::AnyWindowHandle::from(attachment)
        .update(cx, |_, window, cx| {
            window.draw(cx).clear(cx);
            assert!(window.has_image_atlas_entry(&image));
        })
        .unwrap();
    gpui::AnyWindowHandle::from(preview)
        .update(cx, |_, window, cx| crate::image_preview::close(window, cx))
        .unwrap();
    cx.run_until_parked();
    gpui::AnyWindowHandle::from(attachment)
        .update(cx, |_, window, cx| {
            window.draw(cx).clear(cx);
            assert!(
                window.has_image_atlas_entry(&image),
                "the remaining image owner must reupload after atlas invalidation"
            );
        })
        .unwrap();
}

struct SliderGallery {
    slider: gpui::Entity<crate::slider::SliderState>,
    disabled: bool,
}
impl Render for SliderGallery {
    fn render(&mut self, _: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let root = div().w(px(480.0)).child(
            crate::slider::Slider::new(
                "threshold",
                ArtisanTheme::resolved(),
                "Threshold",
                "Value",
                self.slider.clone(),
            )
            .disabled(self.disabled)
            .debug_selector("gallery-slider"),
        );
        crate::slider::Slider::follow_drag(root, &[self.slider.clone()])
    }
}
#[gpui::test]
fn slider_pointer_keyboard_and_disabled_keep_one_geometry(cx: &mut TestAppContext) {
    use gpui::AppContext as _;
    let (gallery, cx) = cx.add_window_view(|_, cx| SliderGallery {
        slider: cx.new(|_| {
            crate::slider::SliderState::new(
                crate::slider::SliderRange::new(0.0, 1.0, Some(0.3)),
                0.0,
            )
        }),
        disabled: false,
    });
    let bounds = cx.debug_bounds("gallery-slider").unwrap();
    cx.simulate_click(bounds.center(), gpui::Modifiers::none());
    assert!(cx.update(|window, cx| window.focused(cx)).is_some());
    press_key(cx, "home");
    press_key(cx, "right");
    gallery.read_with(cx, |gallery, cx| {
        assert_eq!(gallery.slider.read(cx).value(), 0.3)
    });
    press_key(cx, "end");
    gallery.read_with(cx, |gallery, cx| {
        assert_eq!(gallery.slider.read(cx).value(), 1.0)
    });
    // Keyboard focus never changes track borders or its bounds.
    assert_eq!(bounds.size, cx.debug_bounds("gallery-slider").unwrap().size);
    gallery.update(cx, |gallery, cx| {
        gallery.disabled = true;
        cx.notify();
    });
    cx.run_until_parked();
    let disabled = cx.debug_bounds("gallery-slider").unwrap();
    assert_eq!(bounds.size, disabled.size);
    cx.simulate_click(disabled.center(), gpui::Modifiers::none());
    press_key(cx, "home");
    gallery.read_with(cx, |gallery, cx| {
        assert_eq!(gallery.slider.read(cx).value(), 1.0)
    });
}

struct SharedControlsGallery {
    selection: u8,
    disabled: bool,
    expanded: bool,
    opens: usize,
    tree_focus: crate::tree_row::TreeFocus,
}
impl Render for SharedControlsGallery {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        use crate::{
            segmented_control::{Segment, SegmentedControl},
            tree_row::TreeRow,
        };
        let owner = cx.entity().downgrade();
        let branches = cx.entity().downgrade();
        let leaves = cx.entity().downgrade();
        let modes = SegmentedControl::new(
            "modes",
            ArtisanTheme::resolved(),
            "Mode",
            self.selection,
            Segment::new(0, "First"),
        )
        .options([Segment::new(1, "Second"), Segment::new(2, "Third")])
        .disabled(self.disabled)
        .debug_selector("gallery-mode")
        .on_change(move |selected, _, cx| {
            let _ = owner.update(cx, |this, cx| {
                this.selection = selected;
                cx.notify();
            });
        });
        let branch = TreeRow::branch(
            self.tree_focus.node("branch"),
            ArtisanTheme::resolved(),
            0,
            "Directory",
            self.expanded,
            move |expanded, _, cx| {
                let _ = branches.update(cx, |this, cx| {
                    this.expanded = expanded;
                    cx.notify();
                });
            },
        )
        .debug_selector("gallery-branch");
        let leaf = TreeRow::leaf(
            self.tree_focus.node("leaf"),
            ArtisanTheme::resolved(),
            1,
            "File",
            "directory/file.rs",
            move |_, cx| {
                let _ = leaves.update(cx, |this, cx| {
                    this.opens += 1;
                    cx.notify();
                });
            },
        )
        .debug_selector("gallery-leaf");
        div()
            .w(px(480.0))
            .flex()
            .flex_col()
            .child(modes)
            .child(branch)
            .child(leaf)
    }
}
#[gpui::test]
fn segmented_controls_roving_focus_and_disabled_selection(cx: &mut TestAppContext) {
    let (gallery, cx) = cx.add_window_view(|_, _| SharedControlsGallery {
        selection: 0,
        disabled: false,
        expanded: false,
        opens: 0,
        tree_focus: Default::default(),
    });
    let first = cx.debug_bounds("gallery-mode-0").unwrap();
    cx.simulate_click(first.center(), gpui::Modifiers::none());
    let before = cx.update(|window, cx| window.focused(cx));
    press_key(cx, "right");
    gallery.read_with(cx, |this, _| assert_eq!(this.selection, 1));
    let after = cx.update(|window, cx| window.focused(cx));
    assert!(before.is_some() && after.is_some());
    assert_ne!(before, after);
    press_key(cx, "end");
    gallery.read_with(cx, |this, _| assert_eq!(this.selection, 2));
    press_key(cx, "right");
    gallery.read_with(cx, |this, _| assert_eq!(this.selection, 0));
    gallery.update(cx, |this, cx| {
        this.disabled = true;
        cx.notify();
    });
    cx.run_until_parked();
    let second = cx.debug_bounds("gallery-mode-1").unwrap();
    cx.simulate_click(second.center(), gpui::Modifiers::none());
    press_key(cx, "right");
    gallery.read_with(cx, |this, _| assert_eq!(this.selection, 0));
}
#[gpui::test]
fn tree_disclosure_keys_and_activation_keep_row_pitch(cx: &mut TestAppContext) {
    let (gallery, cx) = cx.add_window_view(|_, _| SharedControlsGallery {
        selection: 0,
        disabled: false,
        expanded: false,
        opens: 0,
        tree_focus: Default::default(),
    });
    let branch = cx.debug_bounds("gallery-branch").unwrap();
    cx.simulate_click(branch.center(), gpui::Modifiers::none());
    gallery.read_with(cx, |this, _| assert!(this.expanded));
    press_key(cx, "left");
    gallery.read_with(cx, |this, _| assert!(!this.expanded));
    press_key(cx, "right");
    gallery.read_with(cx, |this, _| assert!(this.expanded));
    assert_eq!(branch, cx.debug_bounds("gallery-branch").unwrap());
    let leaf = cx.debug_bounds("gallery-leaf").unwrap();
    assert_eq!(branch.size.height, leaf.size.height);
    cx.simulate_click(leaf.center(), gpui::Modifiers::none());
    press_key(cx, "enter");
    gallery.read_with(cx, |this, _| assert_eq!(this.opens, 2));
}
