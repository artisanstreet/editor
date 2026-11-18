use super::{page_record, raw_rows};
use artisan_api::{CollectionRange, CollectionRef, ContentRange, ContentRef, TextContent};
use editor_core::{
    CollectionWindow, CompletionKey, CompletionSlot, ContentWindow, Core, ForgeClient, HostData,
    HostId, HostInstance, RangeState,
    client::{CopyContentError, ExportContentError},
    delivery::OwnedDelivery,
};
use std::{num::NonZeroUsize, ops::Range, path::PathBuf, sync::Arc};

static COPIES: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(1);

enum Completion {
    Copy(
        CompletionKey,
        Result<String, CopyContentError>,
        tokio::sync::SemaphorePermit<'static>,
    ),
    Export(CompletionKey, Result<(), ExportContentError>),
}

pub struct CopiedSource {
    text: String,
    host: HostId,
    instance: HostInstance,
    failure: std::rc::Weak<std::cell::RefCell<Option<String>>>,
    _permit: tokio::sync::SemaphorePermit<'static>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Copied {
    Written,
    Failed,
    Abandoned,
}

impl CopiedSource {
    pub fn write<E: std::error::Error>(
        self,
        core: &Core,
        write: impl FnOnce(String) -> Result<(), E>,
    ) -> Copied {
        if core.host_if_instance(&self.host, self.instance).is_none() {
            return Copied::Abandoned;
        }
        match write(self.text) {
            Ok(()) => Copied::Written,
            Err(error) => {
                tracing::warn!(
                    host = %self.host,
                    error = %diagnostics::chain(&error),
                    "could not put the copied source on the clipboard"
                );
                match self.failure.upgrade() {
                    Some(failure) => {
                        *failure.borrow_mut() = Some(format!(
                            "Couldn't copy source to the clipboard: {}",
                            diagnostics::chain(&error)
                        ));
                        Copied::Failed
                    }
                    None => Copied::Abandoned,
                }
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WindowViewState {
    Loading,
    Failed(String),
    Ready(usize),
}

fn window_view_state<R: editor_core::RangePage + Send>(
    window: &editor_core::RangeWindow<R>,
) -> WindowViewState {
    match window.state() {
        RangeState::Loading => WindowViewState::Loading,
        RangeState::Failed(error) => WindowViewState::Failed(error.clone()),
        RangeState::Ready(page) => WindowViewState::Ready(Arc::as_ptr(page) as usize),
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ContentViewState {
    identity: u64,
    source: ContentRef,
    window: WindowViewState,
    busy: bool,
    error: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CollectionViewState {
    source: CollectionRef,
    window: WindowViewState,
    selected: Option<(u64, usize)>,
    detail: Option<ContentViewState>,
    inline: bool,
}

pub struct ContentText {
    identity: u64,
    window: ContentWindow,
    pub rows: Vec<Range<usize>>,
    pub page: Option<Arc<ContentRange>>,
    pub error: Option<String>,
    clipboard_failure: std::rc::Rc<std::cell::RefCell<Option<String>>>,
    client: ForgeClient,
    host: HostId,
    instance: HostInstance,
    action: CompletionSlot,
    picking: Option<u64>,
    picker_generation: u64,
    delivery: OwnedDelivery<Completion>,
}

impl ContentText {
    #[cfg(feature = "test-support")]
    pub fn completion_for_tests(
        &mut self,
    ) -> editor_core::RangeCompletion<artisan_api::ContentRange> {
        self.window.completion_for_tests()
    }
    pub fn new(host: &HostData, source: ContentRef) -> Self {
        Self::from_parts(
            host.client.clone(),
            host.id.clone(),
            host.instance(),
            source,
        )
    }

    fn from_parts(
        client: ForgeClient,
        host: HostId,
        instance: HostInstance,
        source: ContentRef,
    ) -> Self {
        static NEXT_IDENTITY: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        Self {
            identity: NEXT_IDENTITY.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
            window: ContentWindow::new(client.clone(), source),
            rows: Vec::new(),
            page: None,
            error: None,
            clipboard_failure: std::rc::Rc::default(),
            client,
            host,
            instance,
            action: CompletionSlot::default(),
            picking: None,
            picker_generation: 0,
            delivery: OwnedDelivery::new(NonZeroUsize::MIN),
        }
    }

    pub fn identity(&self) -> u64 {
        self.identity
    }

    pub fn view_state(&self) -> ContentViewState {
        ContentViewState {
            identity: self.identity,
            source: self.window.source().clone(),
            window: window_view_state(&self.window),
            busy: self.busy(),
            error: self.shown_error(),
        }
    }

    pub fn shown_error(&self) -> Option<String> {
        self.error
            .clone()
            .or_else(|| self.clipboard_failure.borrow().clone())
    }

    fn clear_error(&mut self) {
        self.error = None;
        self.clipboard_failure.borrow_mut().take();
    }

    pub fn window(&self) -> &ContentWindow {
        &self.window
    }

    pub fn retry(&mut self, core: &Core) {
        if self.reconcile(core) {
            self.window.retry();
        }
    }

    pub fn action_wake(&self) -> &aqua::mailbox::Mailbox<()> {
        self.delivery.mailbox()
    }

    pub fn reconcile(&mut self, core: &Core) -> bool {
        if core.host_if_instance(&self.host, self.instance).is_some() {
            return true;
        }
        self.action.cancel();
        self.picking = None;
        self.window.disconnect();
        self.sync();
        false
    }

    #[cfg(feature = "test-support")]
    pub fn action_finished(&self) -> bool {
        self.action.is_finished()
    }

    pub fn busy(&self) -> bool {
        self.action.is_running() || self.picking.is_some()
    }

    pub fn is_export_pending(&self, generation: u64) -> bool {
        self.picking == Some(generation)
    }

    pub fn set_source(&mut self, source: ContentRef) {
        self.action.cancel();
        self.picking = None;
        self.clear_error();
        self.window.set_source(source);
        self.sync();
    }

    pub fn sync(&mut self) {
        let RangeState::Ready(page) = self.window.state() else {
            self.page = None;
            self.rows.clear();
            return;
        };
        if self.page.as_ref().is_some_and(|old| Arc::ptr_eq(old, page)) {
            return;
        }
        self.rows = raw_rows(&page.text);
        self.page = Some(Arc::clone(page));
    }

    pub fn previous(&mut self, core: &Core) {
        if !self.reconcile(core) {
            return;
        }
        if let Some(page) = &self.page {
            self.window.request(
                page.start
                    .saturating_sub(u64::from(artisan_api::CONTENT_RANGE_BYTES)),
            );
        }
    }

    pub fn next(&mut self, core: &Core) {
        if !self.reconcile(core) {
            return;
        }
        if let Some(page) = &self.page {
            self.window.request(page.end);
        }
    }

    pub fn copy(&mut self, core: &Core) {
        if !self.reconcile(core) || self.busy() {
            return;
        }
        self.clear_error();
        let client = self.client.clone();
        let source = self.window.source().clone();
        let sender = self.delivery.sender();
        self.action
            .spawn(&self.host, self.instance, move |key| async move {
                let permit = COPIES.acquire().await.expect("copy semaphore stays open");
                let result = client.copy_content(&source, 32 * 1024 * 1024).await;
                let _ = sender.send(Completion::Copy(key, result, permit)).await;
            });
    }

    pub fn begin_export(&mut self, core: &Core) -> Option<u64> {
        if !self.reconcile(core) || self.busy() {
            return None;
        }
        self.clear_error();
        self.picker_generation = self.picker_generation.wrapping_add(1);
        self.picking = Some(self.picker_generation);
        self.picking
    }

    pub fn finish_export(&mut self, core: &Core, generation: u64, path: Option<PathBuf>) {
        self.finish_export_result(core, generation, Ok(path));
    }

    pub fn finish_export_result(
        &mut self,
        core: &Core,
        generation: u64,
        result: Result<Option<PathBuf>, String>,
    ) {
        if self.picking != Some(generation) || !self.reconcile(core) {
            return;
        }
        self.picking = None;
        match result {
            Ok(Some(path)) => self.export(core, path),
            Ok(None) => {}
            Err(error) => self.error = Some(format!("Couldn't export source: {error}")),
        }
    }

    pub fn export(&mut self, core: &Core, path: PathBuf) {
        if !self.reconcile(core) || self.busy() {
            return;
        }
        self.clear_error();
        let client = self.client.clone();
        let source = self.window.source().clone();
        let sender = self.delivery.sender();
        self.action
            .spawn(&self.host, self.instance, move |key| async move {
                let result = client.export_content(&source, path).await;
                let _ = sender.send(Completion::Export(key, result)).await;
            });
    }

    pub fn drain(&mut self, core: &Core) -> Option<CopiedSource> {
        if !self.reconcile(core) {
            self.delivery.drain();
            return None;
        }
        self.window.drain();
        self.sync();
        let mut copied = None;
        for completion in self.delivery.drain() {
            match completion {
                Completion::Copy(key, result, permit) if self.action.finish(core, &key) => {
                    match result {
                        Ok(text) => {
                            copied = Some(CopiedSource {
                                text,
                                host: self.host.clone(),
                                instance: self.instance,
                                failure: std::rc::Rc::downgrade(&self.clipboard_failure),
                                _permit: permit,
                            })
                        }
                        Err(error) => {
                            self.error = Some(format!(
                                "Couldn't copy source: {}",
                                diagnostics::chain(&error)
                            ))
                        }
                    }
                }
                Completion::Export(key, result) if self.action.finish(core, &key) => {
                    if let Err(error) = result {
                        self.error = Some(format!(
                            "Couldn't export source: {}",
                            diagnostics::chain(&error)
                        ));
                    }
                }
                Completion::Copy(..) | Completion::Export(..) => {}
            }
        }
        copied
    }
}

pub struct CollectionPreview {
    pub label: String,
    pub detail: Option<TextContent>,
    pub selected: bool,
    pub disabled: bool,
}

pub struct CollectionBrowser<T: serde::de::DeserializeOwned + Send + 'static> {
    identity: u64,
    window: CollectionWindow<T>,
    pub selected: Option<(u64, Arc<CollectionRange<T>>)>,
    pub detail: Option<ContentText>,
    pub inline_text: Option<String>,
    host: HostId,
    instance: HostInstance,
}

impl<T: serde::de::DeserializeOwned + Send + 'static> CollectionBrowser<T> {
    #[cfg(feature = "test-support")]
    pub fn completion_for_tests(
        &mut self,
    ) -> editor_core::RangeCompletion<artisan_api::CollectionRange<T>> {
        self.window.completion_for_tests()
    }
    pub fn new(host: &HostData, source: CollectionRef) -> Self {
        Self::from_parts(
            host.client.clone(),
            host.id.clone(),
            host.instance(),
            source,
        )
    }

    fn from_parts(
        client: ForgeClient,
        host: HostId,
        instance: HostInstance,
        source: CollectionRef,
    ) -> Self {
        static NEXT_IDENTITY: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        Self {
            identity: NEXT_IDENTITY.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
            window: CollectionWindow::new(client, source),
            selected: None,
            detail: None,
            inline_text: None,
            host,
            instance,
        }
    }

    pub fn identity(&self) -> u64 {
        self.identity
    }

    pub fn reconcile(&mut self, core: &Core) -> bool {
        if core.host_if_instance(&self.host, self.instance).is_some() {
            return true;
        }
        self.window.disconnect();
        self.clear_detail();
        false
    }

    pub fn view_state(&self) -> CollectionViewState {
        CollectionViewState {
            source: self.window.source().clone(),
            window: window_view_state(&self.window),
            selected: self
                .selected
                .as_ref()
                .map(|(index, page)| (*index, Arc::as_ptr(page) as usize)),
            detail: self.detail.as_ref().map(ContentText::view_state),
            inline: self.inline_text.is_some(),
        }
    }

    pub fn window(&self) -> &CollectionWindow<T> {
        &self.window
    }

    pub fn request(&mut self, core: &Core, index: u64) {
        if self.reconcile(core) {
            self.window.request(index);
        }
    }

    pub fn retry(&mut self, core: &Core) {
        if self.reconcile(core) {
            self.window.retry();
        }
    }

    pub fn drain(&mut self, core: &Core) -> bool {
        if !self.reconcile(core) {
            return false;
        }
        self.window.drain()
    }

    pub fn set_source(&mut self, source: CollectionRef) {
        if self.window.source() == &source {
            return;
        }
        self.clear_detail();
        self.window.set_source(source);
    }

    pub fn clear_detail(&mut self) {
        self.selected = None;
        self.detail = None;
        self.inline_text = None;
    }

    pub fn open_detail(
        &mut self,
        core: &Core,
        page: Arc<CollectionRange<T>>,
        index: u64,
        present: impl FnOnce(&T, u64) -> CollectionPreview,
    ) -> bool {
        if !self.reconcile(core) {
            return false;
        }
        let Some(record) = page_record(self.window.source(), &page, index) else {
            return false;
        };
        let preview = present(record, index);
        if preview.disabled {
            return false;
        }
        self.clear_detail();
        match preview.detail {
            Some(TextContent::Stored(source)) => {
                if let Some(host) = core.host_if_instance(&self.host, self.instance) {
                    self.detail = Some(ContentText::new(host, source));
                }
            }
            Some(TextContent::Inline(text)) => self.inline_text = Some(text),
            None => {}
        }
        self.selected = Some((index, page));
        true
    }

    pub fn activate(
        &self,
        core: &Core,
        page: &CollectionRange<T>,
        index: u64,
        present: impl FnOnce(&T, u64) -> CollectionPreview,
        activate: impl FnOnce(&T, u64),
    ) -> bool {
        if core.host_if_instance(&self.host, self.instance).is_none() {
            return false;
        }
        let Some(record) = page_record(self.window.source(), page, index) else {
            return false;
        };
        if present(record, index).disabled {
            return false;
        }
        activate(record, index);
        true
    }
}

pub trait SourceProviderExt {
    fn open_content(&self, source: ContentRef) -> Option<ContentText>;
    fn open_collection<T: serde::de::DeserializeOwned + Send + 'static>(
        &self,
        source: CollectionRef,
    ) -> Option<CollectionBrowser<T>>;
}

impl SourceProviderExt for editor_core::host::ScopedSourceProvider {
    fn open_content(&self, source: ContentRef) -> Option<ContentText> {
        Some(ContentText::from_parts(
            self.client()?,
            self.host().clone(),
            self.instance(),
            source,
        ))
    }
    fn open_collection<T: serde::de::DeserializeOwned + Send + 'static>(
        &self,
        source: CollectionRef,
    ) -> Option<CollectionBrowser<T>> {
        Some(CollectionBrowser::from_parts(
            self.client()?,
            self.host().clone(),
            self.instance(),
            source,
        ))
    }
}
