use std::{
    cell::{Cell, RefCell},
    rc::Rc,
    time::Instant,
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
    popover::{AnimatedPickerHost, Popover},
    rich::actions::{CopyKind, ShowCopyMenu},
    chrome::menu::Highlight,
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
    reduced_motion: bool,
    effect: Rc<dyn Fn(ColorCopyEffect)>,
    alive: Rc<Cell<bool>>,
    pending_close: Rc<Cell<bool>>,
    lights: Rc<RefCell<Vec<Highlight>>>,
}

impl Panel {
    fn rows(&self, now: Instant) -> Element {
        let theme = self.theme;
        let radius = crate::ui::chrome::account::menu_radius() - MENU_PADDING;
        let mut rows =
            UiBox::new().style(Style::new().axis(Axis::Column).min_width(Length::Px(160.0)));
        for (index, entry) in self.menu.entries.iter().enumerate() {
            let kind = entry.kind;
            if index > 0 && self.menu.entries[index - 1].kind != kind {
                rows = rows.child(crate::ui::chrome::card::double_rule(&theme, MENU_PADDING));
            }
            let mut icon = UiBox::new()
                .width(16.0)
                .height(16.0)
                .style(Style::new().flex_shrink(0.0));
            let glyph = |asset| {
                source_asset_glyph(asset, 15.0, theme.colors.text_muted.to_reference_paint())
            };
            icon = match (kind, &self.search_icon) {
                (CopyKind::Primary, _) => icon.child(glyph(AssetId::TABLER_COPY)),
                (CopyKind::Search, Some(image)) => icon.child(
                    aqua_ui::element::Media::new(image.clone())
                        .width(16.0)
                        .height(16.0)
                        .fit(aqua_core::image::ImageFit::Contain)
                        .build(),
                ),
                (CopyKind::Search, None) => icon.child(glyph(AssetId::TABLER_SEARCH)),
                (CopyKind::Alternative, _) => icon,
            };
            let label = Text::new(entry.label.clone())
                .text_style(TextStyle {
                    families: vec![FontFamily::Named(theme.typography.sans.family.to_owned())],
                    size_px: 14.0,
                    line_height_px: ROW_LINE,
                    ..TextStyle::default()
                })
                .gpui_source_color(theme.colors.text.to_reference_text_color())
                .build();
            let text = entry.text.clone();
            let select_alive = self.alive.clone();
            let select_pending = self.pending_close.clone();
            let select_effect = self.effect.clone();
            let hover_alive = self.alive.clone();
            let hover_effect = self.effect.clone();
            let lights = self.lights.clone();
            let reduced = self.reduced_motion;
            let lit = self.lights.borrow()[index];
            let row = Button::new()
                .style(
                    Style::new()
                        .align(AlignItems::CENTER)
                        .gap(8.0)
                        .padding_insets(aqua_core::Insets {
                            left: 12.0,
                            right: 12.0,
                            top: ROW_PAD,
                            bottom: ROW_PAD,
                        })
                        .radius(Length::Px(radius)),
                )
                .child(crate::ui::chrome::account::row_highlight(
                    &theme,
                    lit,
                    now,
                    ROW_LINE + 2.0 * ROW_PAD,
                    radius,
                ))
                .child(icon.build())
                .child(label)
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
                .build()
                .with_key(format!("copy-value-{index}"))
                .role(Role::MenuItem)
                .aria_label(entry.label.clone())
                .focusable(true)
                .cursor(CursorIcon::Pointer)
                .on_hover_change(move |hovered| {
                    if hover_alive.get() {
                        let mut lights = lights.borrow_mut();
                        lights[index] = lights[index].light(hovered, Instant::now(), reduced);
                        drop(lights);
                        hover_effect(ColorCopyEffect::RequestFrame);
                    }
                });
            rows = rows.child(row);
        }
        rows.build().with_key("copy-menu-rows")
    }

    fn popover(&self, viewport: aqua_core::Size, now: Instant) -> Popover {
        let dismiss_pending = self.pending_close.clone();
        let dismiss_alive = self.alive.clone();
        let dismiss_effect = self.effect.clone();
        Popover::new(
            "copy-menu",
            "Copy value",
            self.menu.position,
            self.theme,
            self.rows(now),
            viewport,
            move || {
                if dismiss_alive.get() {
                    dismiss_pending.set(true);
                    dismiss_effect(ColorCopyEffect::RequestFrame);
                }
            },
        )
        .menu_padding(MENU_PADDING)
        .strong_glass_menu()
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
            lights: Rc::new(RefCell::new(vec![Highlight::default(); menu.entries.len()])),
            menu,
            search_icon,
            theme,
            reduced_motion,
            effect: Rc::new(effect),
            alive: Rc::new(Cell::new(true)),
            pending_close: Rc::new(Cell::new(false)),
        };
        let (element, mut options) = panel.popover(app.viewport(), Instant::now()).into_parts();
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
        if self.root(app).is_none() {
            return Ok(false);
        }
        let instant = Instant::now();
        let running = self
            .panel
            .lights
            .borrow()
            .iter()
            .any(Highlight::running);
        if running {
            let (element, _) = self.panel.popover(app.viewport(), instant).into_parts();
            self.popup.refresh_content(app, element)?;
            for light in self.panel.lights.borrow_mut().iter_mut() {
                *light = light.settle(instant);
            }
        }
        Ok(running || self.popup.is_animating(app))
    }
}

const MENU_PADDING: f32 = 6.0;
const ROW_PAD: f32 = 8.0;
const ROW_LINE: f32 = 20.0;
