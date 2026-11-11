use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};

use aqua_core::{CursorIcon, InputEvent, Key as InputKey, NamedKey, NodeId, Role};
use aqua_text::{FontFamily, TextStyle};
use aqua_ui::{
    AlignItems,
    element::{Box as UiBox, Button, Element, Key, Text},
    overlay::OverlayId,
    style::{Axis, Length, Style},
    tree::hash_key,
};
use editor_assets::AssetId;

use super::{
    asset::source_asset_glyph,
    hover::HoverTone,
    picker_motion::HoverSurface,
    popover::{AnimatedPickerHost, Popover},
    rich::actions::{CopyKind, ShowCopyMenu},
    theme::{ArtisanTheme, Opacity, RadiusStep, RadiusTokens},
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
    hover: Rc<RefCell<HoverSurface>>,
    popup: AnimatedPickerHost,
    now: f64,
    effect: Rc<dyn Fn(ColorCopyEffect)>,
    theme: ArtisanTheme,
    entries: usize,
    reduced_motion: bool,
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
        let hover = Rc::new(RefCell::new(HoverSurface::new("copy-menu-hover")));
        let mut rows = UiBox::new()
            .style(Style::new().axis(Axis::Column).min_width(Length::Px(160.0)))
            .child(
                hover
                    .borrow()
                    .pill(&theme, RadiusTokens::value(RadiusStep::LgMid), true),
            );
        let entries = menu.entries.len();
        for (index, entry) in menu.entries.iter().enumerate() {
            let primary = entry.kind == CopyKind::Primary;
            if !primary && index > 0 && menu.entries[index - 1].kind == CopyKind::Primary {
                rows = rows.child(separator(&theme));
            }
            let id = format!("copy-value-{index}");
            let mut icon = UiBox::new()
                .width(16.0)
                .height(16.0)
                .style(Style::new().flex_shrink(0.0));
            if primary {
                icon = icon.child(source_asset_glyph(
                    AssetId::TABLER_COPY,
                    15.0,
                    theme.colors.text_muted.to_reference_paint(),
                ));
            }
            let label = Text::new(entry.label.clone())
                .text_style(TextStyle {
                    families: vec![FontFamily::Named(theme.typography.sans.family.to_owned())],
                    size_px: 14.0,
                    line_height_px: 20.0,
                    ..TextStyle::default()
                })
                .gpui_source_color(theme.colors.text.to_reference_text_color())
                .build();
            let text = entry.text.clone();
            let select_alive = alive.clone();
            let select_pending = pending_close.clone();
            let select_effect = effect.clone();
            let hover_state = hover.clone();
            let hover_alive = alive.clone();
            let hover_effect = effect.clone();
            let hover_id = id.clone();
            let row = Button::new()
                .style(
                    Style::new()
                        .align(AlignItems::CENTER)
                        .gap(8.0)
                        .padding_insets(aqua_core::Insets {
                            left: 12.0,
                            right: 12.0,
                            top: 8.0,
                            bottom: 8.0,
                        })
                        .radius(Length::Px(RadiusTokens::value(RadiusStep::LgMid))),
                )
                .child(icon.build())
                .child(label)
                .on_activate(move || {
                    if select_alive.get() && !select_pending.get() {
                        select_effect(ColorCopyEffect::Clipboard(text.clone()));
                        select_pending.set(true);
                        select_effect(ColorCopyEffect::RequestFrame);
                    }
                })
                .build()
                .with_key(id)
                .role(Role::MenuItem)
                .aria_label(entry.label.clone())
                .focusable(true)
                .cursor(CursorIcon::Pointer)
                .on_hover_change(move |hovered| {
                    if hover_alive.get() {
                        hover_state
                            .borrow()
                            .hovered(&hover_id, hovered, HoverTone::Neutral);
                        hover_effect(ColorCopyEffect::RequestFrame);
                    }
                });
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
        )
        .menu_padding(4.0)
        .quiet_glass_menu();
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
            hover,
            popup,
            now,
            effect,
            theme,
            entries,
            reduced_motion,
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
        self.hover.borrow_mut().dispose(app);
        self.popup.dispose(app);
        self.alive.set(false);
    }

    pub fn sync(&mut self, app: &mut aqua::UiApp) {
        if self.pending_close.replace(false) && self.is_open(app) {
            let _ = self.popup.begin_close(app, self.now);
        }
        if !self.is_open(app) {
            self.hover.borrow_mut().dispose(app);
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
        let Some(root) = self.root(app) else {
            return Ok(false);
        };
        let animating = self.popup.is_animating(app);
        let rows_key = hash_key(Key::from("copy-menu-rows"));
        let mut stack = vec![root];
        while let Some(node) = stack.pop() {
            if let Some(record) = app.tree().get(node) {
                stack.extend(record.children.iter().copied());
                if record.key == Some(rows_key) {
                    let frame = self.hover.borrow_mut().advance_frame(
                        app,
                        node,
                        &self.theme,
                        true,
                        now,
                        self.reduced_motion,
                    )?;
                    return Ok(animating || frame.changed || frame.animating);
                }
            }
        }
        Ok(animating)
    }
}

fn separator(theme: &ArtisanTheme) -> Element {
    UiBox::new()
        .height(1.0)
        .style(
            Style::new()
                .flex_shrink(0.0)
                .margin_insets(aqua_core::Insets {
                    top: 4.0,
                    bottom: 4.0,
                    left: 0.0,
                    right: 0.0,
                })
                .gpui_background(aqua_core::GpuiBackground::Solid(
                    theme.colors.text.at(Opacity::Hairline).to_reference_paint(),
                )),
        )
        .build()
}
