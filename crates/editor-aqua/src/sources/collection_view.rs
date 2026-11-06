use super::{CollectionBrowser, CollectionPreview, page_record, view::code_card};
use crate::{
    core::RangeState,
    ui::{action_row::ActionRow, button::Button, theme::ArtisanTheme},
};
use aqua_ui::{
    collection::{
        CollectionDescriptor, CollectionError, CollectionLayout, CollectionRole, CollectionSource,
        CollectionViewport,
    },
    element::{Box as UiBox, Element, Key, Text},
    style::{Length, Style},
};
use artisan_api::CollectionRange;
use std::{cell::RefCell, rc::Rc, sync::Arc};

type Presenter<T> = Rc<dyn Fn(&T, u64) -> CollectionPreview>;
type Handler<T> = Rc<dyn Fn(CollectionAction<T>)>;

pub enum CollectionAction<T> {
    Request(u64),
    Retry,
    Choose {
        page: Arc<CollectionRange<T>>,
        index: u64,
    },
    Read {
        page: Arc<CollectionRange<T>>,
        index: u64,
    },
    Back,
}

struct Rows<T: serde::de::DeserializeOwned + Send + 'static> {
    browser: Rc<RefCell<CollectionBrowser<T>>>,
    count: usize,
    theme: ArtisanTheme,
    present: Presenter<T>,
    action: Handler<T>,
    has_activation: bool,
}

impl<T: serde::de::DeserializeOwned + Send + 'static> CollectionSource for Rows<T> {
    fn len(&self) -> usize {
        self.count
    }
    fn stable_indexed(&self) -> bool {
        true
    }
    fn key(&self, index: usize) -> Key {
        Key::Usize(index)
    }
    fn layout(&self, available_width: f32) -> CollectionLayout {
        CollectionLayout {
            content_width: available_width,
            column_widths: Vec::new(),
        }
    }
    fn role(&self) -> CollectionRole {
        CollectionRole::List
    }
    fn header(&self, _: &CollectionLayout) -> Option<Element> {
        None
    }
    fn validate(&self) -> Result<(), CollectionError> {
        Ok(())
    }
    fn row(&self, index: usize, _: &CollectionLayout) -> Element {
        let index = index as u64;
        let page = self.browser.borrow().window().page_for(index).cloned();
        let Some(page) = page else {
            return UiBox::new()
                .height(ActionRow::row_height(&self.theme))
                .child(
                    Text::new("Loading entry…")
                        .gpui_source_color(self.theme.colors.text.to_reference_text_color())
                        .build(),
                )
                .build();
        };
        let browser = self.browser.borrow();
        let Some(record) = page_record(browser.window().source(), &page, index) else {
            return UiBox::new()
                .height(ActionRow::row_height(&self.theme))
                .child(
                    Text::new("Loading entry…")
                        .gpui_source_color(self.theme.colors.text.to_reference_text_color())
                        .build(),
                )
                .build();
        };
        let preview = (self.present)(record, index);
        drop(browser);
        let callback = Rc::clone(&self.action);
        let chosen = Arc::clone(&page);
        let mut row = ActionRow::new(
            format!("collection-entry-{index}"),
            self.theme,
            preview.label,
        )
        .selected(preview.selected)
        .disabled(preview.disabled)
        .on_activate(move |_| {
            callback(CollectionAction::Choose {
                page: Arc::clone(&chosen),
                index,
            })
        });
        if self.has_activation && preview.detail.is_some() {
            let callback = Rc::clone(&self.action);
            row = row.trailing(
                Button::text(format!("collection-read-{index}"), self.theme, "Read")
                    .disabled(preview.disabled)
                    .on_activate(move |_| {
                        callback(CollectionAction::Read {
                            page: Arc::clone(&page),
                            index,
                        })
                    })
                    .build(),
            );
        }
        UiBox::new()
            .height(ActionRow::row_height(&self.theme))
            .clip(true)
            .child(row.build())
            .build()
    }
}

pub fn collection_list<T: serde::de::DeserializeOwned + Send + 'static>(
    browser: Rc<RefCell<CollectionBrowser<T>>>,
    title: &str,
    theme: ArtisanTheme,
    max_height: Option<f32>,
    present: Presenter<T>,
    has_activation: bool,
    action: Handler<T>,
) -> Element {
    let count = browser.borrow().window().source().count;
    let Ok(count_native) = usize::try_from(count) else {
        return Text::new("This collection exceeds this device's native list capacity.")
            .gpui_source_color(theme.colors.text.to_reference_text_color())
            .build();
    };
    let height = max_height.map_or(ActionRow::row_height(&theme) * 12.0, |height| {
        height.max(32.0).min(ActionRow::row_height(&theme) * 12.0)
    });
    let mut body = UiBox::new()
        .layout("column")
        .gap(theme.spacing.steps(1.0))
        .child(
            Text::new(format!("{title} · {count} entries"))
                .gpui_source_color(theme.colors.text.to_reference_text_color())
                .build(),
        );
    if count_native != 0 {
        let source = Rc::new(Rows {
            browser: Rc::clone(&browser),
            count: count_native,
            theme,
            present,
            action: Rc::clone(&action),
            has_activation,
        });
        let mut descriptor = CollectionDescriptor::new(
            source,
            vec![
                Style::new()
                    .height(Length::Px(height))
                    .width(Length::Pct(100.0)),
            ],
            true,
            ActionRow::row_height(&theme),
            0,
        );
        let watched_browser = Rc::clone(&browser);
        let request = Rc::clone(&action);
        descriptor.fixed_row_height = true;
        descriptor.on_viewport = Some(Rc::new(move |viewport: CollectionViewport| {
            let missing = {
                let browser = watched_browser.borrow();
                (viewport.visible.start..viewport.visible.end)
                    .find(|index| browser.window().page_for(*index as u64).is_none())
            };
            if let Some(index) = missing {
                request(CollectionAction::Request(index as u64));
            }
        }));
        body = body.child(Element::collection(descriptor));
    }
    let state = browser_state(&browser);
    if let Some(error) = state {
        let action = Rc::clone(&action);
        body = body
            .child(
                Text::new(error)
                    .gpui_source_color(theme.colors.text.to_reference_text_color())
                    .build(),
            )
            .child(
                Button::text("collection-retry", theme, "Retry")
                    .on_activate(move |_| action(CollectionAction::Retry))
                    .build(),
            );
    }
    code_card(theme, body.build())
}

fn browser_state<T: serde::de::DeserializeOwned + Send + 'static>(
    browser: &Rc<RefCell<CollectionBrowser<T>>>,
) -> Option<String> {
    match browser.borrow().window().state() {
        RangeState::Failed(error) => Some(error.clone()),
        _ => None,
    }
}

pub fn collection_detail<T: serde::de::DeserializeOwned + Send + 'static>(
    browser: &CollectionBrowser<T>,
    theme: ArtisanTheme,
    max_height: Option<f32>,
    document: aqua_text::document::SelectionDocumentId,
    source_id: aqua_text::document::SelectionSourceId,
    stored_detail: Option<Element>,
    action: Handler<T>,
) -> Result<super::ContentView, &'static str> {
    if browser.detail.is_some() && stored_detail.is_none() {
        return Err("stored collection detail requires its source view");
    }
    let callback = Rc::clone(&action);
    let mut body = UiBox::new()
        .layout("column")
        .gap(theme.spacing.steps(1.0))
        .child(
            Button::text("collection-back", theme, "Back to entries")
                .on_activate(move |_| callback(CollectionAction::Back))
                .build(),
        );
    let mut selection = None;
    if let Some(detail) = stored_detail {
        body = body.child(detail);
    } else if let Some(text) = &browser.inline_text {
        let text: Arc<str> = Arc::from(text.as_str());
        let ranges = super::raw_rows(&text);
        selection = Some(super::selection_source(source_id, 0, Arc::clone(&text)));
        let height = max_height.map_or(ActionRow::row_height(&theme) * 12.0, |height| {
            height.max(32.0).min(ActionRow::row_height(&theme) * 12.0)
        });
        let list = aqua_ui::collection::List::new()
            .items(ranges.into_iter().enumerate())
            .key(|(index, _)| format!("collection-detail-line-{index}"))
            .row(move |(_, range)| {
                Text::new(text[range.clone()].to_owned())
                    .gpui_source_color(theme.colors.text.to_reference_text_color())
                    .selection_source_in(document, super::source_slice(source_id, range.clone()))
                    .build()
            })
            .virtualize()
            .measured_uniform_rows()
            .style(
                Style::new()
                    .height(Length::Px(height))
                    .width(Length::Pct(100.0)),
            )
            .build();
        body = body.child(list);
    }
    Ok(super::ContentView {
        element: code_card(theme, body.build()),
        selection,
    })
}

pub fn dispatch_collection<T: serde::de::DeserializeOwned + Send + 'static>(
    browser: &mut CollectionBrowser<T>,
    core: &crate::core::Core,
    action: CollectionAction<T>,
    present: impl Fn(&T, u64) -> CollectionPreview,
    activate: Option<&dyn Fn(&T, u64)>,
) -> bool {
    match action {
        CollectionAction::Request(index) => {
            browser.request(core, index);
            true
        }
        CollectionAction::Retry => {
            browser.retry(core);
            true
        }
        CollectionAction::Choose { page, index } => match activate {
            Some(activate) => browser.activate(core, &page, index, present, activate),
            None => browser.open_detail(core, page, index, present),
        },
        CollectionAction::Read { page, index } => browser.open_detail(core, page, index, present),
        CollectionAction::Back => {
            if !browser.reconcile(core) {
                return false;
            }
            browser.clear_detail();
            true
        }
    }
}

pub fn dispatch_collection_changed<T: serde::de::DeserializeOwned + Send + 'static>(
    browser: &mut CollectionBrowser<T>,
    core: &crate::core::Core,
    action: CollectionAction<T>,
    present: impl Fn(&T, u64) -> CollectionPreview,
    activate: Option<&dyn Fn(&T, u64)>,
) -> bool {
    let before = browser.view_state();
    dispatch_collection(browser, core, action, present, activate);
    browser.view_state() != before
}

pub fn drain_collection_changed<T: serde::de::DeserializeOwned + Send + 'static>(
    browser: &mut CollectionBrowser<T>,
    core: &crate::core::Core,
) -> bool {
    let before = browser.view_state();
    browser.drain(core);
    browser.view_state() != before
}
