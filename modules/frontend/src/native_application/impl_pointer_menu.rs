//! Menus that open at the pointer on a right-click: a glass panel with a
//! sliding hover pill and the dropdown motion, answering the arrow keys,
//! Enter, and Escape.
//!
//! The sidebar's thread rows open Settle Thread and, past a separator,
//! Archive Thread in a muted red. A reply asks for a copy menu
//! ([`ShowCopyMenu`]): a color value as written with the copy icon, then,
//! past a separator, the color in each notation, indented as if they had an
//! icon; an image's or model's path.

use super::*;
use artisan_domain::ThreadChange;
use artisan_ui::markdown_renderer::{CopyEntry, CopyKind, ShowCopyMenu};
use artisan_ui::theme::Oklch;
use gpui::prelude::FluentBuilder as _;

/// Width of a thread menu's panel; copy menus size to their labels.
const THREAD_MENU_WIDTH_PX: f32 = 208.0;

/// Debug selector of the menu panel.
pub(super) const POINTER_MENU_SELECTOR: &str = "artisan-pointer-menu";

/// What one item does.
#[derive(Clone, Debug, Eq, PartialEq)]
enum PointerMenuAction {
    Settle(ThreadId),
    Archive(ThreadId),
    Copy(SharedString),
}

/// One item of an open menu, in menu order.
#[derive(Clone, Debug)]
struct PointerMenuItem {
    /// Stable within the menu: keys the hover pill and the debug selector.
    id: SharedString,
    label: SharedString,
    /// The glyph beside the label; `None` keeps its slot empty so labels
    /// still line up.
    icon: Option<AssetId>,
    /// Whether a separator stands above the item.
    separated: bool,
    action: PointerMenuAction,
}

impl PointerMenuItem {
    /// Archive is the one destructive item.
    fn destructive(&self) -> bool {
        matches!(self.action, PointerMenuAction::Archive(_))
    }
}

/// An open menu: its items, where it opened, and its width.
#[derive(Clone, Debug)]
struct PointerMenuTarget {
    items: Rc<[PointerMenuItem]>,
    position: gpui::Point<gpui::Pixels>,
    width: Option<gpui::Pixels>,
}

#[derive(Default)]
pub(super) struct PointerMenuState {
    /// The open menu; `None` once it closes.
    target: Option<PointerMenuTarget>,
    /// The menu the panel paints, kept while it closes.
    shown: Option<PointerMenuTarget>,
    /// The keyboard's row, if the keyboard moved.
    highlighted: Option<usize>,
    motion: Rc<RefCell<PickerMenuMotion>>,
    motion_task: Option<Task<()>>,
    focus: Option<FocusHandle>,
    hover: Rc<RefCell<SlidingHoverState>>,
    surface: Rc<RefCell<Option<Bounds<gpui::Pixels>>>>,
}

impl NativeApplication {
    /// Opens the thread row menu for `thread` at `position`.
    pub(super) fn open_thread_menu(
        &mut self,
        thread: ThreadId,
        position: gpui::Point<gpui::Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let items = [
            PointerMenuItem {
                id: SharedString::from("settle"),
                label: SharedString::from("Settle Thread"),
                icon: Some(AssetId::TABLER_CIRCLE_CHECK),
                separated: false,
                action: PointerMenuAction::Settle(thread.clone()),
            },
            PointerMenuItem {
                id: SharedString::from("archive"),
                label: SharedString::from("Archive Thread"),
                icon: Some(AssetId::TABLER_ARCHIVE),
                // The destructive item stands apart.
                separated: true,
                action: PointerMenuAction::Archive(thread),
            },
        ];
        let target = PointerMenuTarget {
            items: Rc::from(items),
            position,
            width: Some(px(THREAD_MENU_WIDTH_PX)),
        };
        self.open_pointer_menu(target, window, cx);
    }

    /// Opens the copy menu a rich block asked for.
    pub(super) fn open_copy_menu(
        &mut self,
        action: &ShowCopyMenu,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let items = copy_menu_items(&action.entries);
        if items.is_empty() {
            return;
        }
        let target = PointerMenuTarget {
            items,
            position: action.position,
            width: None,
        };
        self.open_pointer_menu(target, window, cx);
    }

    fn open_pointer_menu(
        &mut self,
        target: PointerMenuTarget,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let focus = self
            .pointer_menu
            .focus
            .get_or_insert_with(|| cx.focus_handle())
            .clone();
        let menu = &mut self.pointer_menu;
        menu.target = Some(target.clone());
        menu.shown = Some(target);
        menu.highlighted = None;
        menu.hover.borrow_mut().clear();
        self.animate_pointer_menu(true, cx);
        window.focus(&focus, cx);
        cx.notify();
    }

    /// Closes the menu, letting it fade out.
    pub(super) fn close_pointer_menu(&mut self, cx: &mut Context<Self>) {
        if self.pointer_menu.target.take().is_none() {
            return;
        }
        self.animate_pointer_menu(false, cx);
        cx.notify();
    }

    fn animate_pointer_menu(&mut self, opening: bool, cx: &mut Context<Self>) {
        let menu = &mut self.pointer_menu;
        let generation = if opening {
            menu.motion.borrow_mut().begin_open()
        } else {
            menu.motion.borrow_mut().begin_close()
        };
        let settle = move |app: &mut Self| {
            let menu = &mut app.pointer_menu;
            let settled = if opening {
                menu.motion.borrow_mut().finish_open(generation)
            } else {
                menu.motion.borrow_mut().finish_close(generation)
            };
            if settled && !opening {
                menu.shown = None;
            }
            settled
        };
        if cx.reduce_motion() {
            settle(self);
            self.pointer_menu.motion_task = None;
            return;
        }
        let duration = if opening {
            PROFILE_MENU_OPEN
        } else {
            PROFILE_MENU_CLOSE
        };
        self.pointer_menu.motion_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(duration).await;
            let _ = this.update(cx, |app, cx| {
                if settle(app) {
                    app.pointer_menu.motion_task = None;
                    cx.notify();
                }
            });
        }));
    }

    /// Carries out the item at `index` of the open menu.
    fn choose_pointer_menu_item(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(item) = self
            .pointer_menu
            .target
            .as_ref()
            .and_then(|target| target.items.get(index))
            .cloned()
        else {
            return;
        };
        self.close_pointer_menu(cx);
        match item.action {
            PointerMenuAction::Settle(thread) => {
                self.change_thread(thread, ThreadChange::Settle, cx);
            }
            PointerMenuAction::Archive(thread) => {
                self.change_thread(thread, ThreadChange::Archive, cx);
            }
            PointerMenuAction::Copy(text) => {
                cx.write_to_clipboard(gpui::ClipboardItem::new_string(text.to_string()));
            }
        }
    }

    fn handle_pointer_menu_key(
        &mut self,
        event: &gpui::KeyDownEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(items) = self
            .pointer_menu
            .target
            .as_ref()
            .map(|target| Rc::clone(&target.items))
        else {
            return;
        };
        let count = items.len();
        let menu = &mut self.pointer_menu;
        let next = match event.keystroke.key.as_str() {
            "escape" | "tab" => {
                cx.stop_propagation();
                self.close_pointer_menu(cx);
                return;
            }
            "enter" | "space" => {
                cx.stop_propagation();
                if let Some(index) = menu.highlighted {
                    self.choose_pointer_menu_item(index, cx);
                }
                return;
            }
            "down" => menu.highlighted.map_or(0, |index| (index + 1) % count),
            "up" => menu
                .highlighted
                .map_or(count - 1, |index| (index + count - 1) % count),
            _ => return,
        };
        cx.stop_propagation();
        menu.highlighted = Some(next);
        menu.hover
            .borrow_mut()
            .set_active(items[next].id.to_string());
        cx.notify();
    }

    /// The menu panel, while it is open or fading out.
    pub(super) fn pointer_menu_panel(&self, cx: &Context<Self>) -> Option<AnyElement> {
        let menu = &self.pointer_menu;
        let shown = menu.shown.as_ref()?;
        let interactive = menu.target.is_some();
        let theme = self.desktop_theme;
        let radius = RadiusTokens::value(RadiusStep::X2l);
        let surface = Rc::clone(&menu.surface);
        let surface_probe = canvas(
            |_, _, _| {},
            move |bounds, (), window, cx| {
                if surface.replace(Some(bounds)) != Some(bounds) {
                    window.defer(cx, |window, _| window.refresh());
                }
            },
        )
        .absolute()
        .top_0()
        .left_0()
        .size_full();
        let mut rows = div()
            .id("artisan-pointer-menu-rows")
            .relative()
            .flex()
            .flex_col()
            .child(surface_probe)
            .child(render_picker_hover_pill(
                &self.theme,
                &menu.hover,
                "pointer-menu",
                RadiusTokens::value(RadiusStep::Xl),
                cx.reduce_motion(),
            ))
            .on_hover(cx.listener(|app, hovered: &bool, window, cx| {
                if !*hovered && !window.last_input_was_keyboard() {
                    app.pointer_menu.hover.borrow_mut().clear();
                    cx.notify();
                }
            }));
        for (index, item) in shown.items.iter().enumerate() {
            if item.separated && index > 0 {
                rows = rows.child(
                    div()
                        .debug_selector(|| "artisan-pointer-menu-separator".to_owned())
                        .h(px(1.0))
                        .my(px(4.0))
                        .bg(self.profile_separator_paint()),
                );
            }
            rows = rows.child(self.pointer_menu_row(index, item, theme, interactive, cx));
        }
        let mut panel = div()
            .id(POINTER_MENU_SELECTOR)
            .debug_selector(|| POINTER_MENU_SELECTOR.to_owned())
            .map(|panel| match shown.width {
                Some(width) => panel.w(width),
                None => panel.min_w(px(160.0)),
            })
            .p(px(4.0))
            .rounded(radius)
            .border_1()
            .border_color(theme.line)
            .backdrop_blur(glass_blur_radius(GlassStrength::Quiet))
            .bg(glass_foreground_base(&self.theme))
            .shadow(glass_card_shadows())
            .font_family(self.theme.typography.body().family)
            .relative()
            .flex()
            .flex_col()
            .child(glass_material_layer(GlassStrength::Quiet, radius))
            .child(glass_highlight_layer(GlassStrength::Quiet, radius))
            .child(rows);
        // A menu fading out lets the pointer through to what is beneath.
        if interactive {
            panel = panel.block_mouse_except_scroll();
            if let Some(focus) = &menu.focus {
                panel = panel.track_focus(focus);
            }
            panel = panel
                .on_key_down(cx.listener(Self::handle_pointer_menu_key))
                .on_mouse_down_out(cx.listener(|app, _: &gpui::MouseDownEvent, _, cx| {
                    app.close_pointer_menu(cx);
                }));
        }
        let snapshot = *menu.motion.borrow();
        Some(
            gpui::deferred(
                gpui::anchored()
                    .position(shown.position)
                    .snap_to_window_with_margin(px(8.0))
                    .child(animate_picker_menu_timed(
                        panel,
                        Rc::clone(&menu.motion),
                        snapshot,
                        "pointer-menu",
                        // It opens below and right of the pointer, so it
                        // grows from its top-left corner.
                        PickerMenuEntrance::Scale {
                            origin: gpui::point(0.0, 0.0),
                        },
                        PROFILE_MENU_OPEN,
                        PROFILE_MENU_CLOSE,
                    )),
            )
            .with_priority(1)
            .into_any_element(),
        )
    }

    fn pointer_menu_row(
        &self,
        index: usize,
        item: &PointerMenuItem,
        theme: DesktopTheme,
        interactive: bool,
        cx: &Context<Self>,
    ) -> Stateful<Div> {
        let menu = &self.pointer_menu;
        let selector = format!("artisan-pointer-menu-{}", item.id);
        let (glyph, label) = if item.destructive() {
            let red = self.theme.colors.destructive;
            // A lighter, quieter red than the label, so the icon reads as
            // muted beside it.
            let muted = Oklch::new((red.l + 0.08).min(1.0), red.c * 0.7, red.h);
            (
                item.icon.map(|icon| {
                    asset_glyph(icon)
                        .size(px(15.0))
                        .text_color(muted.to_paint())
                }),
                red.to_paint(),
            )
        } else {
            (
                item.icon
                    .map(|icon| asset_glyph(icon).size(px(15.0)).text_color(theme.secondary)),
                theme.foreground,
            )
        };
        let id = item.id.to_string();
        let mut row = div()
            .id(("artisan-pointer-menu-item", index))
            .debug_selector(move || selector.clone())
            .role(gpui::Role::MenuItem)
            .aria_label(item.label.clone())
            .relative()
            .px(px(12.0))
            .py(px(8.0))
            .flex()
            .items_center()
            .gap(px(8.0))
            .rounded(RadiusTokens::value(RadiusStep::Xl))
            .cursor_pointer()
            .child(sidebar_hover_probe(
                Rc::clone(&menu.hover),
                Rc::clone(&menu.surface),
                &id,
            ))
            .child(
                div()
                    .size(px(16.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .flex_shrink_0()
                    .children(glyph),
            )
            .child(
                div()
                    .text_size(px(14.0))
                    .line_height(px(20.0))
                    .whitespace_nowrap()
                    .text_color(label)
                    .child(item.label.clone()),
            );
        if interactive {
            row = row
                .on_hover(cx.listener(move |app, hovered: &bool, _, cx| {
                    if *hovered {
                        let menu = &mut app.pointer_menu;
                        menu.highlighted = Some(index);
                        menu.hover.borrow_mut().set_active(id.clone());
                        cx.notify();
                    }
                }))
                .on_click(cx.listener(move |app, _, _, cx| {
                    cx.stop_propagation();
                    app.choose_pointer_menu_item(index, cx);
                }));
        }
        row
    }
}

/// A copy menu's rows: primary entries with the copy icon, then, past one
/// separator, the alternative forms with an empty icon slot.
fn copy_menu_items(entries: &[CopyEntry]) -> Rc<[PointerMenuItem]> {
    entries
        .iter()
        .enumerate()
        .map(|(index, entry)| {
            let primary = entry.kind == CopyKind::Primary;
            let separated = !primary && index > 0 && entries[index - 1].kind == CopyKind::Primary;
            PointerMenuItem {
                id: SharedString::from(format!("copy-{index}")),
                label: entry.label.clone(),
                icon: primary.then_some(AssetId::TABLER_COPY),
                separated,
                action: PointerMenuAction::Copy(entry.text.clone()),
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_color_menu_leads_with_the_value_then_separates_its_notations() {
        let entries = [
            CopyEntry::primary("Copy Value", "#ff0000"),
            CopyEntry::alternative("Copy as RGB", "rgb(255 0 0)"),
            CopyEntry::alternative("Copy as OKLCH", "oklch(0.628 0.258 29.2)"),
        ];
        let items = copy_menu_items(&entries);
        let shape: Vec<_> = items
            .iter()
            .map(|item| (item.icon.is_some(), item.separated))
            .collect();
        assert_eq!(shape, [(true, false), (false, true), (false, false)]);
        assert_eq!(
            items[1].action,
            PointerMenuAction::Copy(SharedString::from("rgb(255 0 0)"))
        );
    }

    #[test]
    fn a_path_menu_is_one_item_with_its_icon() {
        let items = copy_menu_items(&[CopyEntry::primary("Copy Image Path", "/a.png")]);
        assert_eq!(items.len(), 1);
        assert!(items[0].icon.is_some() && !items[0].separated);
    }
}
