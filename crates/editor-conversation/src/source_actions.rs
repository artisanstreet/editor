//! Explicit raw-source clipboard export, separate from rendered semantic selection.
use editor_ui::button::{Button, ButtonVariant};
use editor_ui::theme::ArtisanTheme;
use gpui::{AnyElement, IntoElement, ParentElement, Styled, div};
use std::sync::Arc;
const CLIPBOARD_MAX_BYTES: usize = 32 * 1024 * 1024;
#[derive(Clone)]
pub(crate) enum CopySource {
    Text(Arc<str>),
    Live(Arc<ropey::Rope>),
}
impl CopySource {
    pub(crate) fn from_parts(live: Option<Arc<ropey::Rope>>, text: Option<Arc<str>>) -> Self {
        match live {
            Some(live) => Self::Live(live),
            None => Self::Text(text.expect("static prose source")),
        }
    }
    fn bytes(&self) -> usize {
        match self {
            Self::Text(text) => text.len(),
            Self::Live(text) => text.len_bytes(),
        }
    }
    fn into_text(self) -> String {
        match self {
            Self::Text(text) => text.to_string(),
            Self::Live(text) => text.to_string(),
        }
    }
}
pub(crate) fn copy_action(key: String, theme: ArtisanTheme, source: CopySource) -> AnyElement {
    let admitted = source.bytes() <= CLIPBOARD_MAX_BYTES;
    let label = match &source {
        CopySource::Text(_) => "Copy full content",
        CopySource::Live(_) => "Copy live preview",
    };
    let button = Button::text(key, theme, label)
        .variant(ButtonVariant::Ghost)
        .disabled(!admitted)
        .on_activate(move |_, _, cx| {
            if !admitted {
                return;
            }
            let source = source.clone();
            cx.spawn(async move |cx| {
                let text = cx
                    .background_executor()
                    .spawn(async move { source.into_text() })
                    .await;
                let _ =
                    cx.update(|cx| cx.write_to_clipboard(gpui::ClipboardItem::new_string(text)));
            })
            .detach();
        });
    let mut root = div().w_full().child(button);
    if !admitted {
        root = root.child("Full content exceeds the 32 MiB clipboard limit.");
    }
    root.into_any_element()
}
