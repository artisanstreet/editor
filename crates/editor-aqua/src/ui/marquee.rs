use aqua_core::Transform2D;
use aqua_ui::{
    element::{Box as UiBox, Element, Key, Text},
    style::{Length, Style},
    tree::hash_key,
};

pub struct Marquee {
    id: String,
    text: String,
    started: Option<f64>,
    animating: bool,
}

impl Marquee {
    pub fn new(id: impl Into<String>, text: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            text: text.into(),
            started: None,
            animating: false,
        }
    }

    pub fn set_text(&mut self, text: impl Into<String>) {
        let text = text.into();
        if self.text != text {
            self.text = text;
            self.started = None;
        }
    }

    pub fn build(&self, line_height: f32) -> Element {
        let first = Text::new(self.text.clone())
            .style(
                Style::new()
                    .absolute_at(Some(0.0), Some(0.0), None, None)
                    .flex_shrink(0.0),
            )
            .build()
            .with_key(format!("{}-first", self.id));
        let second = Text::new(self.text.clone())
            .style(
                Style::new()
                    .absolute_at(Some(0.0), Some(0.0), None, None)
                    .flex_shrink(0.0)
                    .opacity(0.0),
            )
            .build()
            .with_key(format!("{}-second", self.id));
        UiBox::new()
            .width(Length::Pct(100.0))
            .height(line_height)
            .style(
                Style::new()
                    .flex_grow(1.0)
                    .flex_shrink(1.0)
                    .min_width(Length::Px(0.0)),
            )
            .clip(true)
            .child(first)
            .child(second)
            .build()
            .with_key(self.id.clone())
    }

    pub fn advance(
        &mut self,
        app: &mut aqua::UiApp,
        owner: aqua_core::NodeId,
        now: f64,
        reduced_motion: bool,
    ) -> bool {
        let [first, second] = ["first", "second"].map(|suffix| {
            app.tree()
                .find_by_key(hash_key(Key::from(format!("{}-{}", self.id, suffix))))
        });
        let (Some(first), Some(second)) = (first, second) else {
            self.animating = false;
            return false;
        };
        let (Some(viewport), Some(text)) = (app.tree().rect(owner), app.tree().rect(first)) else {
            self.animating = false;
            return false;
        };
        self.animating = !reduced_motion && text.w > viewport.w;
        let offset = if self.animating {
            let started = *self.started.get_or_insert(now);
            (std::time::Duration::from_secs_f64((now - started - 1.6).max(0.0)).as_secs_f32()
                * 36.0)
                .rem_euclid(text.w + 48.0)
        } else {
            0.0
        };
        for (node, x, opacity) in [
            (first, -offset, 1.0),
            (
                second,
                text.w + 48.0 - offset,
                if self.animating { 1.0 } else { 0.0 },
            ),
        ] {
            if let Some(record) = app.tree().get(node) {
                let mut paint = record.paint().clone();
                paint.opacity = Some(opacity);
                paint.transform = Some(Transform2D::translate(x, 0.0));
                app.tree_mut().set_paint(node, paint);
            }
        }
        self.animating
    }

    pub fn is_animating(&self) -> bool {
        self.animating
    }
}
