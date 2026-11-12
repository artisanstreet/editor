use std::{cell::RefCell, collections::BTreeMap, rc::Rc, time::Instant};

use aqua_core::{NodeId, Transform2D};
use aqua_ui::{
    element::{Box as UiBox, Element, Key},
    style::Style,
    tree::hash_key,
};

use super::{
    hover::HoverTone,
    picker_motion::HoverFrame,
    theme::ArtisanTheme,
};
use crate::ui::chrome::menu::Highlight;
use crate::ui::kit::Hover;

#[derive(Clone, Copy)]
enum Layer {
    Background,
    FadeIn,
}

#[derive(Clone)]
pub struct PopInHover {
    prefix: String,
    highlights: Rc<RefCell<BTreeMap<String, Highlight>>>,
}

impl PopInHover {
    pub fn new(prefix: impl Into<String>) -> Self {
        Self {
            prefix: prefix.into(),
            highlights: Rc::default(),
        }
    }

    pub fn hovered(&self, id: &str, hovered: bool, _: HoverTone) {
        self.hovered_at(id, hovered, Instant::now(), false);
    }

    pub fn hovered_at(&self, id: &str, hovered: bool, now: Instant, reduced: bool) {
        let mut highlights = self.highlights.borrow_mut();
        if hovered {
            for (key, highlight) in highlights.iter_mut() {
                if key != id {
                    *highlight = highlight.light(false, now, reduced);
                }
            }
        }
        let highlight = highlights.entry(id.to_owned()).or_default();
        *highlight = highlight.light(hovered, now, reduced);
    }

    pub fn hide(&self) {
        let now = Instant::now();
        for highlight in self.highlights.borrow_mut().values_mut() {
            *highlight = highlight.light(false, now, false);
        }
    }

    pub fn clear(&self) {
        let now = Instant::now();
        for highlight in self.highlights.borrow_mut().values_mut() {
            *highlight = highlight.light(false, now, true);
        }
    }

    pub fn dispose(&mut self, _: &mut aqua::UiApp) {
        self.clear();
    }

    pub fn is_animating(&self) -> bool {
        self.highlights.borrow().values().any(Highlight::running)
    }

    pub fn background(&self, id: &str, theme: &ArtisanTheme, radius: f32) -> Element {
        self.layer(id, theme, Hover::Neutral, radius)
    }

    pub fn layer(&self, id: &str, theme: &ArtisanTheme, hover: Hover, radius: f32) -> Element {
        UiBox::new()
            .style(hover.style(theme))
            .style(
                Style::new()
                    .absolute_at(Some(0.0), Some(0.0), Some(0.0), Some(0.0))
                    .radius(aqua_ui::style::Length::Px(radius))
                    .opacity(0.0),
            )
            .build()
            .with_key(self.key(id))
            .exclude_from_hit_testing()
    }

    pub fn fade_in(&self, id: &str, child: Element) -> Element {
        UiBox::new()
            .style(
                Style::new()
                    .absolute_at(Some(0.0), Some(0.0), Some(0.0), Some(0.0))
                    .opacity(0.0),
            )
            .child(child)
            .build()
            .with_key(self.fade_in_key(id))
            .exclude_from_hit_testing()
    }

    pub fn advance_frame(
        &mut self,
        app: &mut aqua::UiApp,
        surface: NodeId,
        now: Instant,
        reduced: bool,
    ) -> HoverFrame {
        let mut highlights = self.highlights.borrow_mut();
        for highlight in highlights.values_mut() {
            *highlight = match reduced {
                true => Highlight::default().light(highlight.lit(), now, true),
                false => highlight.settle(now),
            };
        }
        let keys: BTreeMap<_, _> = highlights
            .keys()
            .flat_map(|id| {
                [
                    (hash_key(Key::from(self.key(id))), (id.clone(), Layer::Background)),
                    (hash_key(Key::from(self.fade_in_key(id))), (id.clone(), Layer::FadeIn)),
                ]
            })
            .collect();
        let mut frame = HoverFrame::default();
        let mut stack = vec![surface];
        while let Some(node) = stack.pop() {
            let Some(record) = app.tree().get(node) else {
                continue;
            };
            stack.extend(record.children.iter().copied());
            let Some((id, layer)) = record.key.and_then(|key| keys.get(&key)) else {
                continue;
            };
            let Some(highlight) = highlights.get_mut(id) else {
                continue;
            };
            let Some(mut paint) = app.tree().paint(node) else {
                continue;
            };
            paint.opacity = Some(highlight.opacity(now));
            if matches!(layer, Layer::Background) {
                let height = app.tree().rect(node).map_or(0.0, |rect| rect.h);
                let scale = highlight.scale(now);
                paint.transform = Some(
                    Transform2D::scale(1.0, scale)
                        .then(Transform2D::translate(0.0, height * (1.0 - scale))),
                );
            }
            frame.changed |= app.tree_mut().set_paint(node, paint);
            frame.animating |= highlight.running();
        }
        highlights.retain(|_, highlight| highlight.lit() || highlight.running());
        frame
    }

    fn key(&self, id: &str) -> String {
        format!("{}-{id}", self.prefix)
    }

    fn fade_in_key(&self, id: &str) -> String {
        format!("{}-fade-in-{id}", self.prefix)
    }
}
