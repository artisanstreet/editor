use super::{
    CollectionBrowser, CollectionPreview, page_record,
    view::{button, code_card, source_lines, stack, viewport},
};
use editor_core::RangeState;
use editor_ui::kit::{ButtonSize, ButtonTone, Density, Row, RowItem, Surface, TextRole};
use editor_ui::theme::ArtisanTheme;
use aqua_ui::{
    collection::{
        CollectionDescriptor, CollectionError, CollectionLayout, CollectionRole, CollectionSource,
        CollectionViewport,
    },
    element::{Box as UiBox, Element, Key},
    style::{Length, Style},
};
use artisan_api::CollectionRange;
use std::{cell::RefCell, rc::Rc, sync::Arc};

type Presenter<T> = Rc<dyn Fn(&T, u64) -> CollectionPreview>;
type Handler<T> = Rc<dyn Fn(CollectionAction<T>)>;

const ENTRY: Row = Row::ListItem(Density::Regular);
const READ: ButtonSize = ButtonSize::Sm;

fn entry_height() -> f32 {
    ENTRY.height().unwrap_or_default()
}

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

impl<T: serde::de::DeserializeOwned + Send + 'static> Rows<T> {
    fn loading(&self, index: u64) -> Element {
        RowItem::new(
            format!("collection-loading-{index}"),
            self.theme,
            ENTRY,
            Surface::Inset,
            "Loading entry…",
        )
        .disabled(true)
        .build()
    }
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
            return self.loading(index);
        };
        let browser = self.browser.borrow();
        let Some(record) = page_record(browser.window().source(), &page, index) else {
            return self.loading(index);
        };
        let preview = (self.present)(record, index);
        drop(browser);
        let callback = Rc::clone(&self.action);
        let chosen = Arc::clone(&page);
        let mut row = RowItem::new(
            format!("collection-entry-{index}"),
            self.theme,
            ENTRY,
            Surface::Inset,
            preview.label,
        )
        .selected(preview.selected)
        .disabled(preview.disabled)
        .on_activate(move || {
            callback(CollectionAction::Choose {
                page: Arc::clone(&chosen),
                index,
            })
        });
        if self.has_activation && preview.detail.is_some() {
            let callback = Rc::clone(&self.action);
            row = row.trailing(
                button(
                    &format!("collection-read-{index}"),
                    self.theme,
                    ButtonTone::Quiet,
                    "Read",
                    move || {
                        callback(CollectionAction::Read {
                            page: Arc::clone(&page),
                            index,
                        })
                    },
                )
                .size(READ)
                .nested(
                    ENTRY.radius_in(Surface::Inset),
                    (entry_height() - READ.height()) / 2.0,
                )
                .disabled(preview.disabled)
                .build(),
            );
        }
        UiBox::new()
            .height(entry_height())
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
        return TextRole::Label
            .text(&theme, "This collection exceeds this device's native list capacity.")
            .build();
    };
    let height = viewport(entry_height(), max_height);
    let mut body = stack().child(
        TextRole::Title
            .text(&theme, format!("{title} · {count} entries"))
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
            entry_height(),
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
    if let Some(error) = browser_state(&browser) {
        let action = Rc::clone(&action);
        body = body
            .child(TextRole::Label.text(&theme, error).build())
            .child(
                button("collection-retry", theme, ButtonTone::Neutral, "Retry", move || {
                    action(CollectionAction::Retry)
                })
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
    let mut body = stack().child(
        button(
            "collection-back",
            theme,
            ButtonTone::Quiet,
            "Back to entries",
            move || callback(CollectionAction::Back),
        )
        .build(),
    );
    let mut selection = None;
    if let Some(detail) = stored_detail {
        body = body.child(detail);
    } else if let Some(text) = &browser.inline_text {
        let text: Arc<str> = Arc::from(text.as_str());
        let ranges = super::raw_rows(&text);
        selection = Some(super::selection_source(source_id, 0, Arc::clone(&text)));
        body = body.child(source_lines(
            "collection-detail-line",
            theme,
            text,
            ranges,
            viewport(TextRole::Code.spec().line, max_height),
            document,
            source_id,
        ));
    }
    Ok(super::ContentView {
        element: code_card(theme, body.build()),
        selection,
    })
}

pub fn dispatch_collection<T: serde::de::DeserializeOwned + Send + 'static>(
    browser: &mut CollectionBrowser<T>,
    core: &editor_core::Core,
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
    core: &editor_core::Core,
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
    core: &editor_core::Core,
) -> bool {
    let before = browser.view_state();
    browser.drain(core);
    browser.view_state() != before
}
