use editor_ui::kit::{Button, ButtonTone};
use editor_ui::theme::ArtisanTheme;
use aqua_ui::element::{Box as UiBox, Element, Text};
use aqua_ui::style::Length;
use std::rc::Rc;
use std::sync::Arc;

const CLIPBOARD_MAX_BYTES: usize = 32 * 1024 * 1024;

#[derive(Clone)]
pub enum CopySource {
    Text(Arc<str>),
    Live(Arc<ropey::Rope>),
}

impl CopySource {
    pub fn from_parts(live: Option<Arc<ropey::Rope>>, text: Arc<str>) -> Self {
        live.map_or_else(|| Self::Text(text), Self::Live)
    }

    pub fn bytes(&self) -> usize {
        match self {
            Self::Text(text) => text.len(),
            Self::Live(text) => text.len_bytes(),
        }
    }
    pub fn into_text(self) -> String {
        match self {
            Self::Text(text) => text.to_string(),
            Self::Live(text) => text.to_string(),
        }
    }
}

pub fn copy_action(
    key: String,
    theme: ArtisanTheme,
    source: CopySource,
    copy: Rc<dyn Fn(CopySource)>,
) -> Element {
    let admitted = source.bytes() <= CLIPBOARD_MAX_BYTES;
    let label = match &source {
        CopySource::Text(_) => "Copy full content",
        CopySource::Live(_) => "Copy live preview",
    };
    let button = Button::text(key, theme, ButtonTone::Quiet, label)
        .disabled(!admitted)
        .on_activate(move |_| {
            if admitted {
                copy(source.clone());
            }
        })
        .build();
    let mut root = UiBox::new()
        .layout("column")
        .width(Length::Pct(100.0))
        .child(button);
    if !admitted {
        root = root.child(Text::new("Full content exceeds the 32 MiB clipboard limit.").build());
    }
    root.build()
}

pub fn failed_body(
    theme: ArtisanTheme,
    error: String,
    key: String,
    source: CopySource,
    copy: Rc<dyn Fn(CopySource)>,
) -> Element {
    UiBox::new()
        .layout("column")
        .width(Length::Pct(100.0))
        .child(Text::new(error).build())
        .child(copy_action(key, theme, source, copy))
        .build()
}
