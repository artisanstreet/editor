mod collection;
mod collection_view;
mod content;
mod controller;
mod selection;
mod view;

pub use collection::{page_record, valid_page};
pub use content::raw_rows;

pub use controller::{
    CollectionBrowser, CollectionPreview, CollectionViewState, ContentText, ContentViewState,
    CopiedSource, WindowViewState,
};

pub use selection::{selection_source, source_slice};

pub use view::{
    ContentAction, ContentEffect, ContentView, content_view, dispatch_content,
    dispatch_content_changed, drain_content, drain_content_changed,
};

pub use collection_view::{
    CollectionAction, collection_detail, collection_list, dispatch_collection,
    dispatch_collection_changed, drain_collection_changed,
};
