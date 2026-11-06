use crate::core::{ForgeClient, delivery::Delivery};
use artisan_api::{CollectionRange, CollectionRef, ContentRange, ContentRef};
use futures::future::BoxFuture;
use std::{collections::VecDeque, num::NonZeroUsize, sync::Arc};
use tokio_util::task::AbortOnDropHandle;

pub trait RangePage: Sized + 'static {
    type Source: Clone + PartialEq + 'static;
    const KEPT: usize;
    const SPAN: u64;
    const FAILURE: &'static str;
    fn length(source: &Self::Source) -> u64;
    fn start(&self) -> u64;
    fn end(&self) -> u64;
    fn total(&self) -> u64;
    fn fetch(
        client: &ForgeClient,
        source: &Self::Source,
        offset: u64,
    ) -> BoxFuture<'static, anyhow::Result<Self>>;
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
    ) -> BoxFuture<'static, anyhow::Result<Self>> {
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
    ) -> BoxFuture<'static, anyhow::Result<Self>> {
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

pub struct RangeWindow<R: RangePage + Send> {
    client: Option<ForgeClient>,
    source: R::Source,
    state: RangeState<R>,
    pages: VecDeque<Arc<R>>,
    requested: u64,
    generation: u64,
    read: Option<AbortOnDropHandle<()>>,
    delivery: Delivery<(u64, anyhow::Result<R>)>,
}

pub type ContentWindow = RangeWindow<ContentRange>;
pub type CollectionWindow<T> = RangeWindow<CollectionRange<T>>;

#[cfg(feature = "test-support")]
pub struct RangeCompletion<R: RangePage + Send> {
    generation: u64,
    sender: crate::core::delivery::DeliverySender<(u64, anyhow::Result<R>)>,
}

#[cfg(feature = "test-support")]
impl<R: RangePage + Send> RangeCompletion<R> {
    pub async fn complete(&self, result: anyhow::Result<R>) -> bool {
        self.sender.send((self.generation, result)).await.is_ok()
    }
}

impl<R: RangePage + Send> RangeWindow<R> {
    pub fn new(client: ForgeClient, source: R::Source) -> Self {
        let mut window = Self {
            client: Some(client),
            source,
            state: RangeState::Loading,
            pages: VecDeque::new(),
            requested: 0,
            generation: 0,
            read: None,
            delivery: Delivery::new(NonZeroUsize::MIN),
        };
        window.request(0);
        window
    }

    pub fn wake_mailbox(&self) -> &aqua::mailbox::Mailbox<()> {
        self.delivery
            .wake_mailbox()
            .expect("range window owns its wake mailbox")
    }

    #[cfg(feature = "test-support")]
    pub fn completion_for_tests(&mut self) -> RangeCompletion<R> {
        self.read = None;
        self.generation = self.generation.wrapping_add(1);
        self.delivery.drain();
        RangeCompletion {
            generation: self.generation,
            sender: self.delivery.sender(),
        }
    }

    pub fn source(&self) -> &R::Source {
        &self.source
    }
    pub fn state(&self) -> &RangeState<R> {
        &self.state
    }

    pub fn page_for(&self, index: u64) -> Option<&Arc<R>> {
        self.pages
            .iter()
            .rev()
            .find(|page| page.start() <= index && index < page.end())
    }

    pub fn set_source(&mut self, source: R::Source) {
        if self.source == source {
            return;
        }
        self.source = source;
        self.pages.clear();
        self.load(0);
    }

    pub fn request(&mut self, offset: u64) {
        let offset = offset.min(R::length(&self.source));
        let covered = self.requested <= offset && offset < self.requested.saturating_add(R::SPAN);
        if covered && (self.read.is_some() || matches!(self.state, RangeState::Failed(_))) {
            return;
        }
        if let Some(index) = self.pages.iter().position(|page| {
            page.start() <= offset && (offset < page.end() || offset == page.total())
        }) && let Some(page) = self.pages.remove(index)
        {
            self.pages.push_back(Arc::clone(&page));
            self.state = RangeState::Ready(page);
            self.requested = offset;
            self.read = None;
            self.generation = self.generation.wrapping_add(1);
            return;
        }
        self.load(offset);
    }

    pub fn retry(&mut self) {
        self.load(self.requested);
    }

    pub fn reset(&mut self) {
        self.pages.clear();
        self.load(self.requested);
    }

    pub fn disconnect(&mut self) {
        self.client = None;
        self.read = None;
        self.generation = self.generation.wrapping_add(1);
        self.state = RangeState::Failed("This Forge is no longer connected.".into());
    }

    fn load(&mut self, offset: u64) {
        self.requested = offset;
        self.read = None;
        self.generation = self.generation.wrapping_add(1);
        let Some(client) = &self.client else {
            self.state = RangeState::Failed("This Forge is no longer connected.".into());
            return;
        };
        let future = R::fetch(client, &self.source, offset);
        let sender = self.delivery.sender();
        let generation = self.generation;
        self.state = RangeState::Loading;
        self.read = Some(AbortOnDropHandle::new(
            crate::core::runtime::runtime().spawn(async move {
                let result = future.await;
                let _ = sender.send((generation, result)).await;
            }),
        ));
    }

    pub fn drain(&mut self) -> bool {
        let mut changed = false;
        for (generation, result) in self.delivery.drain() {
            if generation != self.generation {
                continue;
            }
            self.read = None;
            self.state = match result {
                Ok(page) => {
                    let page = Arc::new(page);
                    self.pages.retain(|old| old.start() != page.start());
                    self.pages.push_back(Arc::clone(&page));
                    while self.pages.len() > R::KEPT {
                        self.pages.pop_front();
                    }
                    RangeState::Ready(page)
                }
                Err(error) => RangeState::Failed(format!("{}: {error:#}", R::FAILURE)),
            };
            changed = true;
        }
        changed
    }
}
