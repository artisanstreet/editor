//! A bounded window over one immutable Forge source: the page around the
//! reader's offset and a few recent pages for revisits. Raw text and record
//! collections share it. The read in flight is owned by its task: changing
//! the source, moving elsewhere or reconnecting drops it, so a stale page
//! can never arrive.
use crate::{ForgeClient, HostData, HostEvent};
use artisan_api::{CollectionRange, CollectionRef, ContentRange, ContentRef};
use futures::future::LocalBoxFuture;
use gpui::{Context, Entity, EventEmitter, Subscription, Task, WeakEntity};
use std::{collections::VecDeque, sync::Arc};

/// One page of an immutable source.
pub trait RangePage: Sized + 'static {
    type Source: Clone + PartialEq + 'static;
    /// Recent pages kept for revisits.
    const KEPT: usize;
    /// Offsets one read covers: a request inside a read in flight (or one
    /// that failed) waits for it, or for an explicit retry.
    const SPAN: u64;
    const FAILURE: &'static str;
    /// The source's length in the page's unit (bytes or records).
    fn length(source: &Self::Source) -> u64;
    fn start(&self) -> u64;
    fn end(&self) -> u64;
    fn total(&self) -> u64;
    fn fetch(
        client: &ForgeClient,
        source: &Self::Source,
        offset: u64,
    ) -> LocalBoxFuture<'static, anyhow::Result<Self>>;
}

impl RangePage for ContentRange {
    type Source = ContentRef;
    const KEPT: usize = 3;
    const SPAN: u64 = 1;
    const FAILURE: &'static str = "Couldn't load source text";
    fn length(source: &ContentRef) -> u64 {
        source.bytes
    }
    fn start(&self) -> u64 {
        self.start
    }
    fn end(&self) -> u64 {
        self.end
    }
    fn total(&self) -> u64 {
        self.total
    }
    fn fetch(
        client: &ForgeClient,
        source: &ContentRef,
        offset: u64,
    ) -> LocalBoxFuture<'static, anyhow::Result<Self>> {
        let (client, source) = (client.clone(), source.clone());
        Box::pin(async move {
            client
                .content_range(&source, offset, artisan_api::CONTENT_RANGE_BYTES)
                .await
        })
    }
}

impl<T: serde::de::DeserializeOwned + Send + 'static> RangePage for CollectionRange<T> {
    type Source = CollectionRef;
    const KEPT: usize = 8;
    const SPAN: u64 = artisan_api::COLLECTION_RANGE_ENTRIES as u64;
    const FAILURE: &'static str = "Couldn't load collection records";
    fn length(source: &CollectionRef) -> u64 {
        source.count
    }
    fn start(&self) -> u64 {
        self.start
    }
    fn end(&self) -> u64 {
        self.end
    }
    fn total(&self) -> u64 {
        self.total
    }
    fn fetch(
        client: &ForgeClient,
        source: &CollectionRef,
        offset: u64,
    ) -> LocalBoxFuture<'static, anyhow::Result<Self>> {
        let (client, source) = (client.clone(), source.clone());
        Box::pin(async move {
            client
                .collection_range(&source, offset, artisan_api::COLLECTION_RANGE_ENTRIES)
                .await
        })
    }
}

pub enum RangeState<R> {
    Loading,
    Ready(Arc<R>),
    Failed(String),
}

/// The window's page or state changed.
pub struct RangeWindowEvent;

pub struct RangeWindow<R: RangePage> {
    host: WeakEntity<HostData>,
    source: R::Source,
    state: RangeState<R>,
    pages: VecDeque<Arc<R>>,
    requested: u64,
    read: Option<Task<()>>,
    _reconnects: Subscription,
}

pub type ContentWindow = RangeWindow<ContentRange>;
pub type CollectionWindow<T> = RangeWindow<CollectionRange<T>>;

impl<R: RangePage> EventEmitter<RangeWindowEvent> for RangeWindow<R> {}

impl<R: RangePage> RangeWindow<R> {
    pub fn new(host: &Entity<HostData>, source: R::Source, cx: &mut Context<Self>) -> Self {
        // A reconnect replaces everything the connection knew; read again.
        let reconnects = cx.subscribe(host, |this, _, event, cx| {
            if matches!(event, HostEvent::Reset) {
                this.pages.clear();
                this.load(this.requested, cx);
            }
        });
        let mut this = Self {
            host: host.downgrade(),
            source,
            state: RangeState::Loading,
            pages: VecDeque::new(),
            requested: 0,
            read: None,
            _reconnects: reconnects,
        };
        this.request(0, cx);
        this
    }

    pub fn source(&self) -> &R::Source {
        &self.source
    }

    pub fn state(&self) -> &RangeState<R> {
        &self.state
    }

    /// A kept page holding `index`. A page may end early at the byte
    /// limit, so its actual bounds count.
    pub fn page_for(&self, index: u64) -> Option<&Arc<R>> {
        self.pages
            .iter()
            .rev()
            .find(|page| page.start() <= index && index < page.end())
    }

    /// Another source: its pages and any read of the old one go.
    pub fn set_source(&mut self, source: R::Source, cx: &mut Context<Self>) {
        if self.source == source {
            return;
        }
        self.source = source;
        self.pages.clear();
        self.load(0, cx);
    }

    /// Shows the page holding `offset`, reading it unless it is kept or
    /// already being read.
    pub fn request(&mut self, offset: u64, cx: &mut Context<Self>) {
        let offset = offset.min(R::length(&self.source));
        let covered = self.requested <= offset && offset < self.requested.saturating_add(R::SPAN);
        if covered && (self.read.is_some() || matches!(self.state, RangeState::Failed(_))) {
            return;
        }
        if let Some(index) = self.pages.iter().position(|page| {
            page.start() <= offset && (offset < page.end() || offset == page.total())
        }) {
            let page = self.pages.remove(index).expect("position is in range");
            self.pages.push_back(page.clone());
            self.state = RangeState::Ready(page);
            self.requested = offset;
            self.read = None;
            self.changed(cx);
            return;
        }
        self.load(offset, cx);
    }

    /// Reads the last requested page again.
    pub fn retry(&mut self, cx: &mut Context<Self>) {
        self.load(self.requested, cx);
    }

    fn load(&mut self, offset: u64, cx: &mut Context<Self>) {
        self.requested = offset;
        let Some(host) = self.host.upgrade() else {
            self.read = None;
            self.state = RangeState::Failed("This Forge is no longer connected.".into());
            self.changed(cx);
            return;
        };
        let read = R::fetch(&host.read(cx).client, &self.source, offset);
        self.state = RangeState::Loading;
        self.read = Some(cx.spawn(async move |this, cx| {
            let result = read.await;
            let _ = this.update(cx, |this, cx| this.finish(result, cx));
        }));
        self.changed(cx);
    }

    fn finish(&mut self, result: anyhow::Result<R>, cx: &mut Context<Self>) {
        self.read = None;
        self.state = match result {
            Ok(page) => {
                let page = Arc::new(page);
                self.pages.retain(|old| old.start() != page.start());
                self.pages.push_back(page.clone());
                while self.pages.len() > R::KEPT {
                    self.pages.pop_front();
                }
                RangeState::Ready(page)
            }
            Err(error) => RangeState::Failed(format!("{}: {error:#}", R::FAILURE)),
        };
        self.changed(cx);
    }

    fn changed(&self, cx: &mut Context<Self>) {
        cx.emit(RangeWindowEvent);
        cx.notify();
    }
}
