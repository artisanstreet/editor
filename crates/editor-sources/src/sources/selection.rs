use aqua_text::document::{SelectionSource, SelectionSourceId, SourceSlice};
use std::{ops::Range, sync::Arc};

pub fn selection_source(
    id: SelectionSourceId,
    order: u64,
    text: impl Into<Arc<str>>,
) -> SelectionSource {
    SelectionSource {
        id,
        order: vec![order],
        text: text.into(),
        separator: Arc::from(""),
        copies: Vec::new(),
    }
}

pub fn source_slice(id: SelectionSourceId, range: Range<usize>) -> SourceSlice {
    SourceSlice { source: id, range }
}
