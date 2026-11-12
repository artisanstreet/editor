use std::{cell::Cell, rc::Rc};

use aqua_core::{InputEvent, Key as InputKey, NamedKey, NodeId, Role};
use aqua_ui::{
    element::{Box as UiBox, Element, Key},
    overlay::OverlayId,
    style::{Axis, Length, Style},
    tree::hash_key,
};
use editor_assets::AssetId;

use super::{
    asset::source_asset_glyph,
    kit::{Divider, IconSize, Row, RowItem, Surface},
    popover::{AnimatedPickerHost, Popover},
    rich::actions::{CopyKind, ShowCopyMenu},
    theme::ArtisanTheme,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ColorCopyEffect {
    Clipboard(String),
    RequestFrame,
    Dismissed,
}

pub struct ColorCopyMenuHost {
    overlay: OverlayId,
    pending_close: Rc<Cell<bool>>,
    alive: Rc<Cell<bool>>,
    popup: AnimatedPickerHost,
    now: f64,
    effect: Rc<dyn Fn(ColorCopyEffect)>,
    entries: usize,
}

impl ColorCopyMenuHost {
    pub fn open(
        app: &mut aqua::UiApp,
        owner: NodeId,
        menu: ShowCopyMenu,
        theme: ArtisanTheme,
        reduced_motion: bool,
        now: f64,
        effect: impl Fn(ColorCopyEffect) + 'static,
    ) -> Result<Option<Self>, String> {
        if menu.entries.is_empty() {
            return Ok(None);
        }
        let effect: Rc<dyn Fn(ColorCopyEffect)> = Rc::new(effect);
        let alive = Rc::new(Cell::new(true));
        let pending_close = Rc::new(Cell::new(false));
        let mut rows =
            UiBox::new().style(Style::new().axis(Axis::Column).min_width(Length::Px(160.0)));
        let entries = menu.entries.len();
        for (index, entry) in menu.entries.iter().enumerate() {
            let primary = entry.kind == CopyKind::Primary;
            if !primary && index > 0 && menu.entries[index - 1].kind == CopyKind::Primary {
                rows = rows.child(separator(&theme));
            }
            let id = format!("copy-value-{index}");
            let mut icon = UiBox::new()
                .width(IconSize::Md.px())
                .height(IconSize::Md.px())
                .style(Style::new().flex_shrink(0.0));
            if primary {
                icon = icon.child(source_asset_glyph(
                    AssetId::TABLER_COPY,
                    IconSize::Md.px(),
                    theme.colors.text_muted.to_reference_paint(),
                ));
            }
            let text = entry.text.clone();
            let select_alive = alive.clone();
            let select_pending = pending_close.clone();
            let select_effect = effect.clone();
            let hover_alive = alive.clone();
            let hover_effect = effect.clone();
            let row = RowItem::new(id, theme, Row::MenuItem, Surface::Menu, entry.label.clone())
                .leading(icon.build())
                .role(Role::MenuItem)
                .on_activate(move || {
                    if select_alive.get() && !select_pending.get() {
                        select_effect(ColorCopyEffect::Clipboard(text.clone()));
                        select_pending.set(true);
                        select_effect(ColorCopyEffect::RequestFrame);
                    }
                })
                .on_hover(move |_| {
                    if hover_alive.get() {
                        hover_effect(ColorCopyEffect::RequestFrame);
                    }
                })
                .build()
                .focusable(true);
            rows = rows.child(row);
        }
        let rows = rows.build().with_key("copy-menu-rows");
        let dismiss_pending = pending_close.clone();
        let dismiss_alive = alive.clone();
        let dismiss_effect = effect.clone();
        let popover = Popover::new(
            "copy-menu",
            "Copy value",
            menu.position,
            theme,
            rows,
            app.viewport(),
            move || {
                if dismiss_alive.get() {
                    dismiss_pending.set(true);
                    dismiss_effect(ColorCopyEffect::RequestFrame);
                }
            },
        );
        let (panel, mut options) = popover.into_parts();
        let closed_alive = alive.clone();
        let closed_effect = effect.clone();
        options.on_close = Some(Rc::new(move |_| {
            if closed_alive.replace(false) {
                closed_effect(ColorCopyEffect::Dismissed);
            }
        }));
        let mut popup = AnimatedPickerHost::open_options(app, owner, panel, options, now)?;
        popup.set_reduced_motion(reduced_motion);
        let overlay = popup.overlay();
        effect(ColorCopyEffect::RequestFrame);
        let mut host = Self {
            overlay,
            pending_close,
            alive,
            popup,
            now,
            effect,
            entries,
        };
        host.advance(app, now)?;
        Ok(Some(host))
    }

    pub fn root(&self, app: &aqua::UiApp) -> Option<NodeId> {
        app.host().popup_root(self.overlay)
    }

    pub fn is_open(&self, app: &aqua::UiApp) -> bool {
        self.alive.get() && app.host().is_open(self.overlay)
    }

    pub fn dispose(self, app: &mut aqua::UiApp) {
        self.popup.dispose(app);
        self.alive.set(false);
    }

    pub fn sync(&mut self, app: &mut aqua::UiApp) {
        if self.pending_close.replace(false) && self.is_open(app) {
            let _ = self.popup.begin_close(app, self.now);
        }
    }

    pub fn handle_input(&mut self, app: &mut aqua::UiApp, event: &InputEvent) -> bool {
        if app.host().service().topmost() != Some(self.overlay) {
            return false;
        }
        if self.popup.request_dismissal(app, event).unwrap_or(false) {
            (self.effect)(ColorCopyEffect::RequestFrame);
            return true;
        }
        if let InputEvent::KeyDown(key) = event {
            let backwards = match key.key {
                InputKey::Named(NamedKey::Down) => Some(false),
                InputKey::Named(NamedKey::Up) => Some(true),
                _ => None,
            };
            if let Some(backwards) = backwards {
                if let Some(root) = self.root(app) {
                    let mut stack = vec![root];
                    let mut nodes = vec![None; self.entries];
                    while let Some(node) = stack.pop() {
                        if let Some(record) = app.tree().get(node) {
                            stack.extend(record.children.iter().copied());
                            for (index, slot) in nodes.iter_mut().enumerate() {
                                if record.key
                                    == Some(hash_key(Key::from(format!("copy-value-{index}"))))
                                {
                                    *slot = Some(node);
                                }
                            }
                        }
                    }
                    let focused = app.tree().focused();
                    let current = nodes
                        .iter()
                        .position(|node| *node == focused && node.is_some());
                    let next = match (current, backwards) {
                        (Some(0), true) | (None, true) => self.entries - 1,
                        (Some(index), true) => index - 1,
                        (Some(index), false) => (index + 1) % self.entries,
                        (None, false) => 0,
                    };
                    if let Some(node) = nodes[next] {
                        app.tree_mut().set_focused(Some(node));
                        return true;
                    }
                }
            }
        }
        let handled = app.handle_input(event);
        self.sync(app);
        handled
    }

    pub fn advance(&mut self, app: &mut aqua::UiApp, now: f64) -> Result<bool, String> {
        self.now = now;
        self.sync(app);
        if self.popup.is_open(app) {
            self.popup.advance(app, now)?;
        }
        Ok(self.root(app).is_some() && self.popup.is_animating(app))
    }
}

fn separator(theme: &ArtisanTheme) -> Element {
    Divider::Rule.build(theme, Surface::Menu.padding_px())
}
