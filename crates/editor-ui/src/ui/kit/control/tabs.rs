use super::link::{Link, LinkTone};
use crate::ui::kit::row::{Density, Row, RowItem};
use crate::ui::kit::surface::Surface;
use crate::ui::kit::tokens::Space;
use crate::ui::theme::ArtisanTheme;
use aqua_core::Role;
use aqua_ui::element::{Box as UiBox, Element, Key};
use std::rc::Rc;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum TabsStyle {
    Text,
    Panel(Surface),
}

pub struct Tabs {
    key: Key,
    theme: ArtisanTheme,
    style: TabsStyle,
    items: Vec<String>,
    selected: usize,
    on_select: Option<Rc<dyn Fn(usize)>>,
}

impl Tabs {
    pub fn new(
        key: impl Into<Key>,
        theme: ArtisanTheme,
        style: TabsStyle,
        items: Vec<String>,
        selected: usize,
    ) -> Self {
        Self {
            key: key.into(),
            theme,
            style,
            items,
            selected,
            on_select: None,
        }
    }

    #[must_use]
    pub fn on_select(mut self, handler: impl Fn(usize) + 'static) -> Self {
        self.on_select = Some(Rc::new(handler));
        self
    }

    #[must_use]
    pub fn build(self) -> Element {
        let select = |index: usize| {
            let handler = self.on_select.clone();
            move || {
                if let Some(handler) = &handler {
                    handler(index);
                }
            }
        };
        let tabs = self.items.iter().enumerate().map(|(index, item)| {
            let key = format!("tab-{index}");
            let chosen = index == self.selected;
            match self.style {
                TabsStyle::Text => Link::new(key, self.theme, LinkTone::Crumb, item.clone())
                    .current(chosen)
                    .on_activate(select(index))
                    .build()
                    .role(Role::Tab)
                    .aria_selected(chosen),
                TabsStyle::Panel(surface) => RowItem::new(
                    key,
                    self.theme,
                    Row::ListItem(Density::Regular),
                    surface,
                    item.clone(),
                )
                .selected(chosen)
                .role(Role::Tab)
                .on_activate(select(index))
                .build(),
            }
        });
        let (layout, gap) = match self.style {
            TabsStyle::Text => ("row", Space::S16),
            TabsStyle::Panel(_) => ("column", Space::S2),
        };
        UiBox::new()
            .layout(layout)
            .gap(gap.px())
            .children(tabs)
            .build()
            .with_key(self.key)
            .role(Role::TabList)
    }
}
