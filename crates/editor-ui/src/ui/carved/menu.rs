use std::rc::Rc;

use aqua_core::Role;
use aqua_ui::element::{Box as UiBox, Button, Element};
use aqua_ui::style::{Length, Style};
use aqua_ui::{AlignItems, JustifyContent};

use super::face::bevel;
use super::glyph::{Icon, glyph};
use super::{Carve, RADIUS_MD, Type};
use crate::ui::kit::{Hover, Surface, live};
use crate::ui::theme::ArtisanTheme;

const ITEM_RADIUS: f32 = RADIUS_MD - 4.0;
const WIDTH: f32 = 196.0;
const LABEL: Type = Type::sans(14.0, 21.0, 400);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MenuItem<V> {
    pub value: V,
    pub label: String,
    pub icon: Icon,
    pub danger: bool,
}

impl<V> MenuItem<V> {
    pub fn new(value: V, label: impl Into<String>, icon: Icon) -> Self {
        Self {
            value,
            label: label.into(),
            icon,
            danger: false,
        }
    }

    #[must_use]
    pub fn danger(mut self) -> Self {
        self.danger = true;
        self
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Entry<V> {
    Item(MenuItem<V>),
    Separator,
}

pub struct ActionMenu<V: Clone + 'static> {
    theme: ArtisanTheme,
    entries: Vec<Entry<V>>,
    pick: Rc<dyn Fn(V)>,
    close: Rc<dyn Fn()>,
}

impl<V: Clone + 'static> std::fmt::Debug for ActionMenu<V> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ActionMenu").finish_non_exhaustive()
    }
}

impl<V: Clone + 'static> ActionMenu<V> {
    #[expect(
        clippy::large_types_passed_by_value,
        reason = "the menu stores this theme copy and paints from it when built"
    )]
    pub fn new(
        theme: ArtisanTheme,
        entries: Vec<Entry<V>>,
        pick: impl Fn(V) + 'static,
        close: impl Fn() + 'static,
    ) -> Self {
        Self {
            theme,
            entries,
            pick: Rc::new(pick),
            close: Rc::new(close),
        }
    }

    #[must_use]
    pub fn into_parts(self) -> (Element, aqua_ui::popup::PopupOptions) {
        let close = Rc::clone(&self.close);
        let mut options = aqua_ui::popup::PopupOptions::menu();
        options.kind = aqua_ui::overlay::OverlayKind::Popover;
        options.focus = aqua_ui::popup::PopupFocus::Preserve;
        options.placement.align = aqua_ui::overlay::Align::End;
        options.placement.gap = 6.0;
        options.placement.flip = false;
        options.on_close = Some(Rc::new(move |_| close()));
        (self.build(), options)
    }

    fn item(&self, index: usize, item: MenuItem<V>) -> Element {
        let theme = self.theme;
        let carve = Carve::of(&theme);
        let (ink, mark) = if item.danger {
            (carve.err, carve.err.with_alpha(0.68))
        } else {
            (carve.ink, carve.ink_dim)
        };
        let pick = Rc::clone(&self.pick);
        let close = Rc::clone(&self.close);
        let value = item.value;
        let button = Button::new()
            .width(Length::Pct(100.0))
            .radius(ITEM_RADIUS)
            .gap(14.0)
            .style(
                Style::new()
                    .align(AlignItems::CENTER)
                    .justify(JustifyContent::SPACE_BETWEEN)
                    .padding_insets(aqua_core::Insets {
                        left: 10.0,
                        right: 10.0,
                        top: 7.0,
                        bottom: 7.0,
                    }),
            )
            .child(live::layer(
                if item.danger {
                    Hover::Destructive
                } else {
                    Hover::Neutral
                },
                &theme,
                ITEM_RADIUS,
                false,
            ))
            .child(
                LABEL
                    .text(&theme, ink, item.label.clone())
                    .wrap(false)
                    .build(),
            )
            .child(glyph(item.icon, 15.0, mark))
            .on_activate(move || {
                close();
                pick(value.clone());
            });
        live::track(
            button
                .build()
                .with_key(index)
                .role(Role::MenuItem)
                .aria_label(item.label)
                .focusable(false)
                .cursor(aqua_core::CursorIcon::Pointer),
        )
    }

    fn build(self) -> Element {
        let carve = Carve::of(&self.theme);
        let mut column = UiBox::new()
            .layout("column")
            .width(Length::Pct(100.0))
            .style(Style::new().gap(2.0));
        for (index, entry) in self.entries.clone().into_iter().enumerate() {
            column = column.child(match entry {
                Entry::Item(item) => self.item(index, item),
                Entry::Separator => UiBox::new()
                    .height(2.0)
                    .style(Style::new().margin_insets(aqua_core::Insets {
                        left: -6.0,
                        right: -6.0,
                        top: 5.0,
                        bottom: 3.0,
                    }))
                    .child(bevel(&carve))
                    .build(),
            });
        }
        Surface::Menu
            .build(&self.theme, vec![column.build()])
            .style(
                Style::new()
                    .min_width(Length::Px(WIDTH))
                    .padding(Length::Px(6.0)),
            )
            .role(Role::Menu)
    }
}
