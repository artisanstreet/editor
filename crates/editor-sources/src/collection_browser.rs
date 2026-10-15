//! A shared native viewport for canonical collections. Descriptor counts never
//! allocate a Vec of all row descriptors. Source and field identity fence actions.
use crate::ContentText;
use artisan_api::{CollectionRange, CollectionRef, TextContent};
use editor_core::{CollectionWindow, HostData, RangeState, RangeWindowEvent};
use editor_ui::{
    action_row::ActionRow,
    button::Button,
    code_card::CodeCard,
    selectable_text::{SelectableText, SelectionSource, register_selection_source},
    theme::ArtisanTheme,
};
use gpui::{
    App, AppContext as _, Context, Entity, IntoElement, ParentElement as _, Render, SharedString,
    Styled as _, Subscription, UniformListScrollHandle, Window, div, uniform_list,
};
use std::{rc::Rc, sync::Arc};

pub struct CollectionPreview {
    pub label: SharedString,
    pub detail: Option<TextContent>,
    pub selected: bool,
    pub disabled: bool,
}
pub type CollectionPresenter<T> = Rc<dyn Fn(&T, u64, &App) -> CollectionPreview>;
pub type CollectionActivation<T> = Rc<dyn Fn(&T, u64, &mut App)>;
pub struct CollectionBrowser<T: serde::de::DeserializeOwned + Send + 'static> {
    host: gpui::WeakEntity<HostData>,
    window: Entity<CollectionWindow<T>>,
    theme: ArtisanTheme,
    title: SharedString,
    present: CollectionPresenter<T>,
    activate: Option<CollectionActivation<T>>,
    selected: Option<(u64, Arc<CollectionRange<T>>)>,
    detail: Option<Entity<ContentText>>,
    inline_text: SharedString,
    inline_rows: Vec<std::ops::Range<usize>>,
    inline_selection: Option<Rc<SelectionSource>>,
    inline_scope: SharedString,
    inline_scroll: UniformListScrollHandle,
    scroll: UniformListScrollHandle,
    max_list_height: Option<gpui::Pixels>,
    _events: Subscription,
}
impl<T: serde::de::DeserializeOwned + Send + 'static> CollectionBrowser<T> {
    pub fn new(
        host: &Entity<HostData>,
        source: CollectionRef,
        title: impl Into<SharedString>,
        theme: ArtisanTheme,
        present: CollectionPresenter<T>,
        activate: Option<CollectionActivation<T>>,
        cx: &mut Context<Self>,
    ) -> Self {
        let window = cx.new(|cx| CollectionWindow::new(host, source, cx));
        let events = cx.subscribe(&window, |_, _, _: &RangeWindowEvent, cx| cx.notify());
        Self {
            host: host.downgrade(),
            window,
            theme,
            title: title.into(),
            present,
            activate,
            selected: None,
            detail: None,
            inline_text: "".into(),
            inline_rows: Vec::new(),
            inline_selection: None,
            inline_scope: format!("collection-detail-{}", cx.entity_id()).into(),
            inline_scroll: UniformListScrollHandle::new(),
            scroll: UniformListScrollHandle::new(),
            max_list_height: None,
            _events: events,
        }
    }
    pub fn source<'a>(&self, cx: &'a App) -> &'a CollectionRef {
        self.window.read(cx).source()
    }
    fn clear_detail(&mut self) {
        self.selected = None;
        self.detail = None;
        self.inline_text = "".into();
        self.inline_rows.clear();
        self.inline_selection = None;
    }
    pub fn set_source(&mut self, source: CollectionRef, cx: &mut Context<Self>) {
        if self.source(cx) == &source {
            return;
        }
        self.clear_detail();
        self.window
            .update(cx, |window, cx| window.set_source(source, cx));
        cx.notify();
    }
    /// Constrain the native viewport while preserving its existing scroll owner.
    pub fn set_max_list_height(&mut self, height: gpui::Pixels, cx: &mut Context<Self>) {
        let height = height.max(gpui::px(32.0));
        if self.max_list_height == Some(height) {
            return;
        }
        self.max_list_height = Some(height);
        cx.notify();
    }
    fn list_height(&self) -> gpui::Pixels {
        let default = ActionRow::row_height(&self.theme) * 12.0;
        self.max_list_height
            .map_or(default, |height| height.min(default))
    }
    pub fn set_theme(&mut self, theme: ArtisanTheme, cx: &mut Context<Self>) {
        if self.theme == theme {
            return;
        }
        self.theme = theme;
        if let Some(detail) = &self.detail {
            detail.update(cx, |detail, cx| detail.set_theme(theme, cx));
        }
        cx.notify();
    }
    fn valid_page(&self, page: &CollectionRange<T>, index: u64, cx: &App) -> bool {
        let source = self.source(cx);
        page.source == source.source
            && page.total == source.count
            && index >= page.start
            && index < page.end
    }
    fn choose(&mut self, page: Arc<CollectionRange<T>>, index: u64, cx: &mut Context<Self>) {
        if !self.valid_page(&page, index, cx) {
            return;
        }
        let record = &page.records[(index - page.start) as usize];
        if (self.present)(record, index, cx).disabled {
            return;
        }
        if let Some(activate) = &self.activate {
            (activate)(record, index, cx);
            cx.notify();
        } else {
            self.open_detail(page, index, cx);
        }
    }
    fn open_detail(&mut self, page: Arc<CollectionRange<T>>, index: u64, cx: &mut Context<Self>) {
        if !self.valid_page(&page, index, cx) {
            return;
        }
        let preview = (self.present)(&page.records[(index - page.start) as usize], index, cx);
        if preview.disabled {
            return;
        }
        self.clear_detail();
        match preview.detail {
            Some(TextContent::Stored(source)) => {
                self.detail = self
                    .host
                    .upgrade()
                    .map(|host| cx.new(|cx| ContentText::new(&host, source, self.theme, cx)));
            }
            Some(TextContent::Inline(text)) => {
                self.inline_text = text.into();
                self.inline_rows = crate::content_text::raw_rows(&self.inline_text);
                self.inline_selection = Some(SelectionSource::single(
                    self.inline_scope.clone(),
                    vec![0],
                    self.inline_text.clone(),
                    "",
                ));
                self.inline_scroll = UniformListScrollHandle::new();
            }
            None => {}
        }
        self.selected = Some((index, page));
        cx.notify();
    }
    fn rows(
        &mut self,
        range: std::ops::Range<usize>,
        cx: &mut Context<Self>,
    ) -> Vec<gpui::AnyElement> {
        range
            .map(|index| {
                let index = index as u64;
                let Some(page) = self.window.read(cx).page_for(index).cloned() else {
                    let window = self.window.clone();
                    // Native uniform lists also build an offscreen row to
                    // measure height. Only a row entering prepaint requests
                    // data, so measuring row zero cannot cancel a tail fetch.
                    return div()
                        .relative()
                        .h(ActionRow::row_height(&self.theme))
                        .child("Loading entry…")
                        .child(
                            gpui::canvas(
                                move |_, _, cx| {
                                    let window = window.clone();
                                    cx.defer(move |cx| {
                                        window.update(cx, |window, cx| window.request(index, cx))
                                    });
                                },
                                |_, _, _, _| {},
                            )
                            .absolute()
                            .inset_0(),
                        )
                        .into_any_element();
                };
                let preview =
                    (self.present)(&page.records[(index - page.start) as usize], index, cx);
                let owner = cx.entity().downgrade();
                let click_page = page.clone();
                let mut row = ActionRow::new(
                    SharedString::from(format!("collection-entry-{index}")),
                    self.theme,
                    preview.label,
                )
                .selected(preview.selected)
                .disabled(preview.disabled)
                .on_activate(move |_, _, cx| {
                    let _ = owner.update(cx, |this, cx| this.choose(click_page.clone(), index, cx));
                });
                if self.activate.is_some() && preview.detail.is_some() {
                    let owner = cx.entity().downgrade();
                    row = row.trailing(
                        Button::text(
                            SharedString::from(format!("collection-read-{index}")),
                            self.theme,
                            "Read",
                        )
                        .disabled(preview.disabled)
                        .on_activate(move |_, _, cx| {
                            cx.stop_propagation();
                            let _ = owner
                                .update(cx, |this, cx| this.open_detail(page.clone(), index, cx));
                        }),
                    );
                }
                div()
                    .h(ActionRow::row_height(&self.theme))
                    .overflow_hidden()
                    .child(row)
                    .into_any_element()
            })
            .collect()
    }
    fn detail_view(&mut self, window: &mut Window, cx: &mut Context<Self>) -> gpui::AnyElement {
        let mut body = div()
            .flex()
            .flex_col()
            .gap(self.theme.spacing.steps(1.0))
            .child(
                Button::text("collection-back", self.theme, "Back to entries").on_activate(
                    cx.listener(|this, _, _, cx| {
                        this.clear_detail();
                        cx.notify();
                    }),
                ),
            );
        if let Some(detail) = &self.detail {
            body = body.child(detail.clone());
        } else if let Some(source) = &self.inline_selection {
            register_selection_source(source, window, cx);
            body = body.child(
                uniform_list(
                    "collection-detail-lines",
                    self.inline_rows.len(),
                    cx.processor(|this, range: std::ops::Range<usize>, _, _| {
                        range
                            .map(|index| {
                                let slice = this.inline_rows[index].clone();
                                div()
                                    .child(
                                        SelectableText::retained(
                                            SharedString::from(format!(
                                                "collection-detail-line-{index}"
                                            )),
                                            this.inline_text[slice.clone()].to_owned(),
                                            this.theme,
                                            vec![],
                                        )
                                        .selection_scope(this.inline_scope.clone())
                                        .selection_order(vec![0], "")
                                        .source_slice(slice),
                                    )
                                    .into_any_element()
                            })
                            .collect()
                    }),
                )
                .track_scroll(&self.inline_scroll)
                .h(self.list_height())
                .w_full(),
            );
        }
        CodeCard::new(self.theme, body).into_any_element()
    }
}
impl<T: serde::de::DeserializeOwned + Send + 'static> Render for CollectionBrowser<T> {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.selected.is_some() {
            return self.detail_view(window, cx);
        }
        let count = self.source(cx).count;
        let Ok(count_native) = usize::try_from(count) else {
            return div()
                .child("This collection exceeds this device's native list capacity.")
                .into_any_element();
        };
        let mut body = div()
            .flex()
            .flex_col()
            .gap(self.theme.spacing.steps(1.0))
            .child(format!("{} · {count} entries", self.title));
        if count != 0 {
            body = body.child(
                uniform_list(
                    "canonical-collection",
                    count_native,
                    cx.processor(|this, range, _, cx| this.rows(range, cx)),
                )
                .track_scroll(&self.scroll)
                .h(self.list_height())
                .w_full(),
            );
        }
        if let RangeState::Failed(error) = self.window.read(cx).state() {
            body = body.child(error.clone()).child(
                Button::text("collection-retry", self.theme, "Retry").on_activate(cx.listener(
                    |this, _, _, cx| this.window.update(cx, |window, cx| window.retry(cx)),
                )),
            );
        }
        CodeCard::new(self.theme, body).into_any_element()
    }
}
