use aqua_text::document::SelectionSourceId;
use aqua_ui::element::Element;
use aqua_ui::style::{Length, Style};
use artisan_api::Item;
use editor_conversation::adapter::RowModel;
use editor_conversation::disclosure::RowDisclosure;
use editor_conversation::fold::{Row, RowKey};
use editor_conversation::presentation::{MarkdownOwners, PresentationScope};
use editor_conversation::view::factory::{RealizedRow, RowFactory};
use editor_conversation::view::status::StatusShimmers;
use editor_ui::theme::{ArtisanTheme, ThemeMode};
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

pub fn row_model(row: Row, items: impl IntoIterator<Item = Arc<Item>>) -> RowModel {
    RowModel {
        row,
        version: 1,
        items: items
            .into_iter()
            .map(|item| (item.id.clone(), item))
            .collect(),
        live: HashMap::new(),
        ui: HashMap::new(),
        run: None,
        author: None,
        gap: 0.0,
        end: 0.0,
    }
}

pub fn realized(model: RowModel) -> RealizedRow {
    RealizedRow {
        model,
        disclosure: RowDisclosure::default(),
        held: false,
    }
}

pub fn source_ids(after: u64) -> (Rc<Cell<u64>>, Rc<dyn Fn() -> SelectionSourceId>) {
    let last = Rc::new(Cell::new(after));
    let allocated = Rc::clone(&last);
    let allocate = Rc::new(move || {
        allocated.set(allocated.get() + 1);
        SelectionSourceId(allocated.get())
    });
    (last, allocate)
}

pub fn fixed_source(id: u64) -> Rc<dyn Fn() -> SelectionSourceId> {
    Rc::new(move || SelectionSourceId(id))
}

pub fn row_factory(
    rows: Vec<RealizedRow>,
    scope: PresentationScope,
    markdown: Rc<RefCell<MarkdownOwners>>,
    reduced_motion: bool,
    allocate: Rc<dyn Fn() -> SelectionSourceId>,
) -> RowFactory {
    row_factory_at(
        rows,
        scope,
        markdown,
        reduced_motion,
        allocate,
        std::time::Instant::now(),
    )
}

pub fn row_factory_at(
    rows: Vec<RealizedRow>,
    scope: PresentationScope,
    markdown: Rc<RefCell<MarkdownOwners>>,
    reduced_motion: bool,
    allocate: Rc<dyn Fn() -> SelectionSourceId>,
    now: std::time::Instant,
) -> RowFactory {
    RowFactory::new(
        rows,
        scope,
        markdown,
        Rc::new(RefCell::new(StatusShimmers::default())),
        *ArtisanTheme::for_mode(ThemeMode::Dark),
        reduced_motion,
        now,
        allocate,
        Rc::new(|| {}),
        Rc::new(|_| {}),
        Rc::new(|_| {}),
        Rc::new(|_| {}),
    )
}

pub fn row_list(
    factory: &Rc<RowFactory>,
    rows: Vec<(usize, RowKey)>,
    row_height: f32,
    list_height: f32,
) -> Element {
    let callback = factory.callback();
    let names: Vec<String> = rows.iter().map(|(_, key)| format!("{key:?}")).collect();
    aqua_ui::collection::List::new()
        .items(0..rows.len())
        .key(move |entry: &usize| names[*entry].clone())
        .try_batched_row(move |entry: &usize| {
            let (index, key) = &rows[*entry];
            callback(*index, key)
        })
        .virtualize()
        .estimated_row_height(Length::Px(row_height))
        .style(
            Style::new()
                .width(Length::Px(600.0))
                .height(Length::Px(list_height)),
        )
        .build()
}

pub fn listed(keys: impl IntoIterator<Item = RowKey>) -> Vec<(usize, RowKey)> {
    keys.into_iter().enumerate().collect()
}
