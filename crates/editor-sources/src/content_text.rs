//! Shared bounded raw-source viewer. Its selection scope is the loaded raw page,
//! separate from a transcript's formatted semantic selection.
use artisan_api::ContentRef;
use editor_core::{ContentWindow, HostData, RangeState, RangeWindow, RangeWindowEvent};
use editor_ui::selectable_text::{SelectableText, SelectionSource, register_selection_source};
use editor_ui::{button::Button, code_card::CodeCard, theme::ArtisanTheme};
use gpui::{
    AppContext as _, ClipboardItem, Context, Entity, IntoElement, ParentElement as _, Render,
    SharedString, Styled as _, Subscription, Task, UniformListScrollHandle, Window, div, px,
    uniform_list,
};
use std::{ops::Range, rc::Rc, sync::Arc};

pub struct ContentText {
    host: gpui::WeakEntity<HostData>,
    window: Entity<ContentWindow>,
    theme: ArtisanTheme,
    rows: Vec<Range<usize>>,
    text: SharedString,
    selection: Option<Rc<SelectionSource>>,
    scope: SharedString,
    page: Option<Arc<artisan_api::ContentRange>>,
    scroll: UniformListScrollHandle,
    action: Option<Task<()>>,
    error: Option<String>,
    _events: Subscription,
}
impl ContentText {
    pub fn new(
        host: &Entity<HostData>,
        source: ContentRef,
        theme: ArtisanTheme,
        cx: &mut Context<Self>,
    ) -> Self {
        let window = cx.new(|cx| ContentWindow::new(host, source, cx));
        let events = cx.subscribe(&window, |this, _, _: &RangeWindowEvent, cx| {
            this.sync(cx);
            cx.notify();
        });
        Self {
            host: host.downgrade(),
            window,
            theme,
            rows: Vec::new(),
            text: "".into(),
            selection: None,
            scope: format!("raw-source-{}", cx.entity_id()).into(),
            page: None,
            scroll: UniformListScrollHandle::new(),
            action: None,
            error: None,
            _events: events,
        }
    }
    pub fn source<'a>(&self, cx: &'a gpui::App) -> &'a ContentRef {
        self.window.read(cx).source()
    }
    pub fn set_theme(&mut self, theme: ArtisanTheme, cx: &mut Context<Self>) {
        if self.theme == theme {
            return;
        }
        self.theme = theme;
        cx.notify();
    }
    pub fn set_source(&mut self, source: ContentRef, cx: &mut Context<Self>) {
        self.action = None;
        self.error = None;
        self.window
            .update(cx, |window, cx| window.set_source(source, cx));
        self.sync(cx);
    }
    fn sync(&mut self, cx: &Context<Self>) {
        let RangeState::Ready(page) = self.window.read(cx).state() else {
            self.page = None;
            self.rows.clear();
            self.selection = None;
            self.text = "".into();
            return;
        };
        if self.page.as_ref().is_some_and(|old| Arc::ptr_eq(old, page)) {
            return;
        }
        self.page = Some(Arc::clone(page));
        self.text = page.text.clone().into();
        self.rows = raw_rows(&self.text);
        self.selection = Some(SelectionSource::single(
            self.scope.clone(),
            vec![page.start],
            self.text.clone(),
            "",
        ));
        self.scroll = UniformListScrollHandle::new();
    }
    fn raw_copy(&mut self, cx: &mut Context<Self>) {
        let Some(host) = self.host.upgrade() else {
            return;
        };
        let client = host.read(cx).client.clone();
        let source = self.source(cx).clone();
        self.error = None;
        self.action = Some(cx.spawn(async move |this, cx| {
            // Native clipboard writes need one complete allocation. Share a single
            // copy slot across source views, including the final UI transfer.
            static COPIES: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(1);
            #[expect(clippy::expect_used, reason = "COPIES is static and never closed")]
            let _copy = COPIES
                .acquire()
                .await
                .expect("clipboard copy semaphore stays open");
            let result = client.copy_content(&source, 32 * 1024 * 1024).await;
            // A closed viewer has no one to report the result to.
            this.update(cx, |this, cx| {
                if this.source(cx) != &source {
                    return;
                }
                this.action = None;
                match result {
                    Ok(text) => cx.write_to_clipboard(ClipboardItem::new_string(text)),
                    Err(error) => this.error = Some(format!("Couldn't copy source: {error:#}")),
                }
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }
    fn export(&mut self, cx: &mut Context<Self>) {
        let Some(host) = self.host.upgrade() else {
            return;
        };
        let client = host.read(cx).client.clone();
        let source = self.source(cx).clone();
        let picker = cx.prompt_for_new_path(&std::env::temp_dir(), Some("source.txt"));
        self.error = None;
        self.action = Some(cx.spawn(async move |this, cx| {
            let result = async {
                if let Some(path) = picker.await?? {
                    client.export_content(&source, path).await?;
                }
                Ok::<_, anyhow::Error>(())
            }
            .await;
            // A closed viewer has no one to report the result to.
            this.update(cx, |this, cx| {
                if this.source(cx) != &source {
                    return;
                }
                this.action = None;
                if let Err(error) = result {
                    this.error = Some(format!("Couldn't export source: {error:#}"));
                }
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }
}
// A raw page has bounded selectable fragments, including one giant line. This
// viewer makes no claim to preserve Markdown wrapping or formatted leaf geometry.
pub fn raw_rows(text: &str) -> Vec<Range<usize>> {
    let mut rows = Vec::new();
    let mut start = 0;
    while start < text.len() {
        let mut cap = (start + 1024).min(text.len());
        while !text.is_char_boundary(cap) {
            cap -= 1;
        }
        let mut end = text[start..cap]
            .find('\n')
            .map_or(cap, |offset| start + offset + 1);
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        rows.push(start..end);
        start = end;
    }
    rows
}
impl Render for ContentText {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.sync(cx);
        if let Some(source) = &self.selection {
            register_selection_source(source, window, cx);
        }
        let theme = self.theme;
        let mut body = div().flex().flex_col().gap(theme.spacing.steps(1.0));
        match self.window.read(cx).state() {
            RangeState::Loading => body = body.child("Loading source…"),
            RangeState::Failed(error) => {
                body = body.child(error.clone()).child(
                    Button::text("source-retry", theme, "Retry").on_activate(
                        cx.listener(|this, _, _, cx| this.window.update(cx, RangeWindow::retry)),
                    ),
                );
            }
            RangeState::Ready(page) => {
                let start = page.start;
                let end = page.end;
                let total = page.total;
                body = body.child(format!(
                    "Raw source bytes {start}–{end} of {total}. Selection copies this loaded page."
                ));
                body = body.child(
                    uniform_list(
                        "raw-source-rows",
                        self.rows.len(),
                        cx.processor(|this, range: Range<usize>, _, _| {
                            let start = this.page.as_ref().map_or(0, |page| page.start);
                            range
                                .map(|index| {
                                    let slice = this.rows[index].clone();
                                    let label = this.text[slice.clone()].to_owned();
                                    div()
                                        .child(
                                            SelectableText::retained(
                                                format!("raw-source-row-{index}"),
                                                label,
                                                this.theme,
                                                vec![],
                                            )
                                            .selection_scope(this.scope.clone())
                                            .selection_order(vec![start], "")
                                            .source_slice(slice),
                                        )
                                        .into_any_element()
                                })
                                .collect()
                        }),
                    )
                    .track_scroll(&self.scroll)
                    .h(px(320.0))
                    .w_full(),
                );
                body = body.child(
                    div()
                        .flex()
                        .gap(theme.spacing.steps(1.0))
                        .child(
                            Button::text("source-prev", theme, "Previous")
                                .disabled(start == 0)
                                .on_activate(cx.listener(move |this, _, _, cx| {
                                    this.window.update(cx, |window, cx| {
                                        window.request(
                                            start.saturating_sub(u64::from(
                                                artisan_api::CONTENT_RANGE_BYTES,
                                            )),
                                            cx,
                                        );
                                    });
                                })),
                        )
                        .child(
                            Button::text("source-next", theme, "Next")
                                .disabled(end == total)
                                .on_activate(cx.listener(move |this, _, _, cx| {
                                    this.window.update(cx, |window, cx| window.request(end, cx));
                                })),
                        ),
                );
            }
        }
        body = body.child(
            div()
                .flex()
                .gap(theme.spacing.steps(1.0))
                .child(
                    Button::text("source-copy", theme, "Copy full raw source")
                        .disabled(self.action.is_some())
                        .on_activate(cx.listener(|this, _, _, cx| this.raw_copy(cx))),
                )
                .child(
                    Button::text("source-export", theme, "Export source")
                        .disabled(self.action.is_some())
                        .on_activate(cx.listener(|this, _, _, cx| this.export(cx))),
                ),
        );
        if let Some(error) = &self.error {
            body = body.child(error.clone());
        }
        CodeCard::new(theme, body)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn raw_rows_preserve_every_unicode_byte_and_bound_individual_layouts() {
        let text = "🙂β".repeat(20000) + "\n\nend";
        let rows = raw_rows(&text);
        assert!(rows.iter().all(|row| row.len() <= 1024));
        assert_eq!(
            rows.iter()
                .map(|row| &text[row.clone()])
                .collect::<String>(),
            text
        );
    }
}
