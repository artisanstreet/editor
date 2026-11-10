use super::{page_record, raw_rows};
use editor_core::{
    CollectionWindow, ContentWindow, Core, ForgeClient, HostData, HostId, HostInstance, RangeState,
    delivery::Delivery, runtime::runtime,
};
use artisan_api::{CollectionRange, CollectionRef, ContentRange, ContentRef, TextContent};
use std::{num::NonZeroUsize, ops::Range, path::PathBuf, sync::Arc};
use tokio_util::task::AbortOnDropHandle;

static COPIES: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(1);

enum Completion {
    Copy(
        ContentRef,
        anyhow::Result<String>,
        tokio::sync::SemaphorePermit<'static>,
    ),
    Export(ContentRef, anyhow::Result<()>),
}

pub struct CopiedSource {
    text: String,
    host: HostId,
    instance: HostInstance,
    _permit: tokio::sync::SemaphorePermit<'static>,
}

impl CopiedSource {
    pub fn write(self, core: &Core, write: impl FnOnce(String)) -> bool {
        if core.host_if_instance(&self.host, self.instance).is_none() {
            return false;
        }
        write(self.text);
        true
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
    client: ForgeClient,
    host: HostId,
    instance: HostInstance,
    action: Option<AbortOnDropHandle<()>>,
    picking: Option<u64>,
    picker_generation: u64,
    delivery: Delivery<Completion>,
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
            client,
            host,
            instance,
            action: None,
            picking: None,
            picker_generation: 0,
            delivery: Delivery::new(NonZeroUsize::MIN),
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
            error: self.error.clone(),
        }
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
        self.delivery
            .wake_mailbox()
            .expect("source action wake mailbox")
    }

    pub fn reconcile(&mut self, core: &Core) -> bool {
        if core.host_if_instance(&self.host, self.instance).is_some() {
            return true;
        }
        self.action = None;
        self.picking = None;
        self.window.disconnect();
        self.sync();
        false
    }

    #[cfg(feature = "test-support")]
    pub fn action_finished(&self) -> bool {
        self.action
            .as_ref()
            .is_some_and(|action| action.is_finished())
    }

    pub fn busy(&self) -> bool {
        self.action.is_some() || self.picking.is_some()
    }

    pub fn is_export_pending(&self, generation: u64) -> bool {
        self.picking == Some(generation)
    }

    pub fn set_source(&mut self, source: ContentRef) {
        self.action = None;
        self.picking = None;
        self.error = None;
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
        self.error = None;
        let client = self.client.clone();
        let source = self.window.source().clone();
        let sender = self.delivery.sender();
        self.action = Some(AbortOnDropHandle::new(runtime().spawn(async move {
            let permit = COPIES.acquire().await.expect("copy semaphore stays open");
            let result = client.copy_content(&source, 32 * 1024 * 1024).await;
            let _ = sender.send(Completion::Copy(source, result, permit)).await;
        })));
    }

    pub fn begin_export(&mut self, core: &Core) -> Option<u64> {
        if !self.reconcile(core) || self.busy() {
            return None;
        }
        self.error = None;
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
        self.error = None;
        let client = self.client.clone();
        let source = self.window.source().clone();
        let sender = self.delivery.sender();
        self.action = Some(AbortOnDropHandle::new(runtime().spawn(async move {
            let result = client.export_content(&source, path).await;
            let _ = sender.send(Completion::Export(source, result)).await;
        })));
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
                Completion::Copy(source, result, permit) if &source == self.window.source() => {
                    self.action = None;
                    match result {
                        Ok(text) => {
                            copied = Some(CopiedSource {
                                text,
                                host: self.host.clone(),
                                instance: self.instance,
                                _permit: permit,
                            })
                        }
                        Err(error) => self.error = Some(format!("Couldn't copy source: {error:#}")),
                    }
                }
                Completion::Export(source, result) if &source == self.window.source() => {
                    self.action = None;
                    if let Err(error) = result {
                        self.error = Some(format!("Couldn't export source: {error:#}"));
                    }
                }
                _ => {}
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
