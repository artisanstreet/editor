use aqua_core::{CursorIcon, Role};
use aqua_ui::element::{Button, Element, Key};
use aqua_ui::style::{Length, Style};

pub struct HitArea {
    key: Key,
    role: Role,
    label: String,
    children: Vec<Element>,
    on_activate: Option<Box<dyn Fn()>>,
}

impl HitArea {
    pub fn new(key: impl Into<Key>, role: Role, label: impl Into<String>) -> Self {
        Self {
            key: key.into(),
            role,
            label: label.into(),
            children: Vec::new(),
            on_activate: None,
        }
    }

    #[must_use]
    pub fn child(mut self, child: Element) -> Self {
        self.children.push(child);
        self
    }

    #[must_use]
    pub fn on_activate(mut self, handler: impl Fn() + 'static) -> Self {
        self.on_activate = Some(Box::new(handler));
        self
    }

    #[must_use]
    pub fn build(self) -> Element {
        let mut button = Button::new().style(
            Style::new()
                .width(Length::Pct(100.0))
                .height(Length::Pct(100.0)),
        );
        for child in self.children {
            button = button.child(child);
        }
        if let Some(handler) = self.on_activate {
            button = button.on_activate(handler);
        }
        button
            .build()
            .with_key(self.key)
            .role(self.role)
            .aria_label(self.label)
            .focusable(true)
            .cursor(CursorIcon::Pointer)
    }
}
