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
    Search(String),
    RequestFrame,
    Dismissed,
}

struct Panel {
    menu: ShowCopyMenu,
    search_icon: Option<aqua::ImageHandle>,
    theme: ArtisanTheme,
    effect: Rc<dyn Fn(ColorCopyEffect)>,
    alive: Rc<Cell<bool>>,
    pending_close: Rc<Cell<bool>>,
}

impl Panel {
    fn rows(&self) -> Element {
        let theme = self.theme;
        let mut rows =
            UiBox::new().style(Style::new().axis(Axis::Column).min_width(Length::Px(160.0)));
        for (index, entry) in self.menu.entries.iter().enumerate() {
            let kind = entry.kind;
            if index > 0 && self.menu.entries[index - 1].kind != kind {
                rows = rows.child(Divider::Rule.build(&theme, Surface::Menu.padding_px()));
            }
            let icon = UiBox::new()
                .width(IconSize::Md.px())
                .height(IconSize::Md.px())
                .style(Style::new().flex_shrink(0.0));
            let glyph = |asset| {
                source_asset_glyph(
                    asset,
                    IconSize::Md.px(),
                    theme.colors.text_muted.to_reference_paint(),
                )
            };
            let icon = match (kind, &self.search_icon) {
                (CopyKind::Primary, _) => icon.child(glyph(AssetId::TABLER_COPY)),
                (CopyKind::Search, Some(image)) => icon.child(
                    aqua_ui::element::Media::new(image.clone())
                        .width(IconSize::Md.px())
                        .height(IconSize::Md.px())
                        .fit(aqua_core::image::ImageFit::Contain)
                        .build(),
                ),
                (CopyKind::Search, None) => icon.child(glyph(AssetId::TABLER_SEARCH)),
                (CopyKind::Alternative, _) => icon,
            };
            let text = entry.text.clone();
            let select_alive = self.alive.clone();
            let select_pending = self.pending_close.clone();
            let select_effect = self.effect.clone();
            let hover_alive = self.alive.clone();
            let hover_effect = self.effect.clone();
            let row = RowItem::new(
                format!("copy-value-{index}"),
                theme,
                Row::MenuItem,
                Surface::Menu,
                entry.label.clone(),
            )
            .leading(icon.build())
            .role(Role::MenuItem)
            .on_activate(move || {
                if select_alive.get() && !select_pending.get() {
                    select_effect(match kind {
                        CopyKind::Primary | CopyKind::Alternative => {
                            ColorCopyEffect::Clipboard(text.clone())
                        }
                        CopyKind::Search => ColorCopyEffect::Search(text.clone()),
                    });
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
        rows.build().with_key("copy-menu-rows")
    }

    fn popover(&self, viewport: aqua_core::Size) -> Popover {
        let dismiss_pending = self.pending_close.clone();
        let dismiss_alive = self.alive.clone();
        let dismiss_effect = self.effect.clone();
        Popover::new(
            "copy-menu",
            "Copy value",
            self.menu.position,
            self.theme,
            self.rows(),
            viewport,
            move || {
                if dismiss_alive.get() {
                    dismiss_pending.set(true);
                    dismiss_effect(ColorCopyEffect::RequestFrame);
                }
            },
        )
    }
}

pub struct ColorCopyMenuHost {
    overlay: OverlayId,
    panel: Panel,
    popup: AnimatedPickerHost,
    now: f64,
}

impl ColorCopyMenuHost {
    pub fn open(
        app: &mut aqua::UiApp,
        owner: NodeId,
        menu: ShowCopyMenu,
        search_icon: Option<aqua::ImageHandle>,
        theme: ArtisanTheme,
        reduced_motion: bool,
        now: f64,
        effect: impl Fn(ColorCopyEffect) + 'static,
    ) -> Result<Option<Self>, String> {
        if menu.entries.is_empty() {
            return Ok(None);
        }
        let panel = Panel {
            menu,
            search_icon,
            theme,
            effect: Rc::new(effect),
            alive: Rc::new(Cell::new(true)),
            pending_close: Rc::new(Cell::new(false)),
        };
        let (element, mut options) = panel.popover(app.viewport()).into_parts();
        let closed_alive = panel.alive.clone();
        let closed_effect = panel.effect.clone();
        options.on_close = Some(Rc::new(move |_| {
            if closed_alive.replace(false) {
                closed_effect(ColorCopyEffect::Dismissed);
            }
        }));
        let mut popup = AnimatedPickerHost::open_options(app, owner, element, options, now)?;
        popup.set_reduced_motion(reduced_motion);
        let overlay = popup.overlay();
        (panel.effect)(ColorCopyEffect::RequestFrame);
        let mut host = Self {
            overlay,
            panel,
            popup,
            now,
        };
        host.advance(app, now)?;
        Ok(Some(host))
    }

    pub fn root(&self, app: &aqua::UiApp) -> Option<NodeId> {
        app.host().popup_root(self.overlay)
    }

    pub fn is_open(&self, app: &aqua::UiApp) -> bool {
        self.panel.alive.get() && app.host().is_open(self.overlay)
    }

    pub fn dispose(self, app: &mut aqua::UiApp) {
        self.popup.dispose(app);
        self.panel.alive.set(false);
    }

    pub fn sync(&mut self, app: &mut aqua::UiApp) {
        if self.panel.pending_close.replace(false) && self.is_open(app) {
            let _ = self.popup.begin_close(app, self.now);
        }
    }

    pub fn intercept(&mut self, app: &mut aqua::UiApp, event: &InputEvent) -> bool {
        if self.popup.is_closing() || app.host().service().topmost() != Some(self.overlay) {
            return false;
        }
        if self.popup.request_dismissal(app, event).unwrap_or(false) {
            (self.panel.effect)(ColorCopyEffect::RequestFrame);
            return true;
        }
        let InputEvent::KeyDown(key) = event else {
            return false;
        };
        let backwards = match key.key {
            InputKey::Named(NamedKey::Down) => false,
            InputKey::Named(NamedKey::Up) => true,
            _ => return false,
        };
        let Some(root) = self.root(app) else {
            return false;
        };
        let entries = self.panel.menu.entries.len();
        let mut stack = vec![root];
        let mut nodes = vec![None; entries];
        while let Some(node) = stack.pop() {
            if let Some(record) = app.tree().get(node) {
                stack.extend(record.children.iter().copied());
                for (index, slot) in nodes.iter_mut().enumerate() {
                    if record.key == Some(hash_key(Key::from(format!("copy-value-{index}")))) {
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
            (Some(0) | None, true) => entries - 1,
            (Some(index), true) => index - 1,
            (Some(index), false) => (index + 1) % entries,
            (None, false) => 0,
        };
        nodes[next].is_some_and(|node| app.tree_mut().set_focused(Some(node)))
    }

    pub fn handle_input(&mut self, app: &mut aqua::UiApp, event: &InputEvent) -> bool {
        if self.intercept(app, event) {
            return true;
        }
        if app.host().service().topmost() != Some(self.overlay) {
            return false;
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
