//! A bounded view of indexed canonical records. Native lists use the full count
//! without allocating transcript rows or fetching a complete record array.
use crate::{HostData, HostEvent};
use artisan_api::{CollectionRange, CollectionRef};
use gpui::{Context, Entity, EventEmitter, Subscription, Task, WeakEntity};
use std::{collections::VecDeque, sync::Arc};

pub enum CollectionWindowState<T> {
    Loading,
    Ready(Arc<CollectionRange<T>>),
    Failed(String),
}
pub struct CollectionWindowEvent;
impl<T: 'static> EventEmitter<CollectionWindowEvent> for CollectionWindow<T> {}

pub struct CollectionWindow<T> {
    host: WeakEntity<HostData>,
    source: CollectionRef,
    state: CollectionWindowState<T>,
    pages: VecDeque<Arc<CollectionRange<T>>>,
    requested: u64,
    generation: u64,
    task: Option<Task<()>>,
    _events: Subscription,
}
impl<T: serde::de::DeserializeOwned + Send + 'static> CollectionWindow<T> {
    pub fn new(host: &Entity<HostData>, source: CollectionRef, cx: &mut Context<Self>) -> Self {
        let events = cx.subscribe(host, |this, _, event, cx| {
            if matches!(event, HostEvent::Reset) {
                this.generation = this.generation.wrapping_add(1);
                this.task = None;
                this.pages.clear();
                this.request_inner(this.requested, true, cx);
            }
        });
        let mut this = Self {
            host: host.downgrade(),
            source,
            state: CollectionWindowState::Loading,
            pages: VecDeque::new(),
            requested: 0,
            generation: 0,
            task: None,
            _events: events,
        };
        this.request(0, cx);
        this
    }
    pub fn source(&self) -> &CollectionRef {
        &self.source
    }
    pub fn state(&self) -> &CollectionWindowState<T> {
        &self.state
    }
    /// Native row builders consult cached pages before requesting another
    /// range. A page may end early at the byte limit, so use actual bounds.
    pub fn page_for(&self, index: u64) -> Option<&Arc<CollectionRange<T>>> {
        self.pages
            .iter()
            .rev()
            .find(|page| page.start <= index && index < page.end)
    }
    pub fn cached_pages(&self) -> usize {
        self.pages.len()
    }
    /// Changing a payload identity cancels the old task and releases its bytes.
    pub fn set_source(&mut self, source: CollectionRef, cx: &mut Context<Self>) {
        if self.source == source {
            return;
        }
        self.source = source;
        self.generation = self.generation.wrapping_add(1);
        self.task = None;
        self.pages.clear();
        self.request_inner(0, true, cx);
    }
    pub fn request(&mut self, offset: u64, cx: &mut Context<Self>) {
        self.request_inner(offset, false, cx);
    }
    pub fn retry(&mut self, cx: &mut Context<Self>) {
        self.request_inner(self.requested, true, cx);
    }
    fn changed(&self, cx: &mut Context<Self>) {
        cx.emit(CollectionWindowEvent);
        cx.notify();
    }
    fn request_inner(&mut self, offset: u64, retry: bool, cx: &mut Context<Self>) {
        let offset = offset.min(self.source.count);
        if !retry {
            if matches!(self.state, CollectionWindowState::Failed(_))
                && self.requested <= offset
                && offset
                    < self
                        .requested
                        .saturating_add(u64::from(artisan_api::COLLECTION_RANGE_ENTRIES))
            {
                return;
            }
            if self.task.is_some()
                && self.requested <= offset
                && offset
                    < self
                        .requested
                        .saturating_add(u64::from(artisan_api::COLLECTION_RANGE_ENTRIES))
            {
                return;
            }
            if let Some(index) = self.pages.iter().position(|page| {
                page.start <= offset && (offset < page.end || offset == page.total)
            }) {
                let page = self.pages.remove(index).unwrap();
                self.state = CollectionWindowState::Ready(page.clone());
                self.pages.push_back(page);
                self.requested = offset;
                self.task = None;
                self.generation = self.generation.wrapping_add(1);
                self.changed(cx);
                return;
            }
        }
        self.requested = offset;
        self.task = None;
        self.generation = self.generation.wrapping_add(1);
        let Some(host) = self.host.upgrade() else {
            self.state = CollectionWindowState::Failed("This Forge is no longer connected.".into());
            self.changed(cx);
            return;
        };
        let client = host.read(cx).client.clone();
        let source = self.source.clone();
        let generation = self.generation;
        self.state = CollectionWindowState::Loading;
        self.task = Some(cx.spawn(async move |this, cx| {
            let result = client
                .collection_range(&source, offset, artisan_api::COLLECTION_RANGE_ENTRIES)
                .await;
            let _ = this.update(cx, |this, cx| this.finish(&source, generation, result, cx));
        }));
        self.changed(cx);
    }
    fn finish(
        &mut self,
        source: &CollectionRef,
        generation: u64,
        result: anyhow::Result<CollectionRange<T>>,
        cx: &mut Context<Self>,
    ) {
        if self.source != *source || self.generation != generation {
            return;
        }
        self.task = None;
        match result {
            Ok(page) => {
                let page = Arc::new(page);
                self.pages.retain(|old| old.start != page.start);
                self.pages.push_back(page.clone());
                while self.pages.len() > 8 {
                    self.pages.pop_front();
                }
                self.state = CollectionWindowState::Ready(page);
            }
            Err(error) => {
                self.state = CollectionWindowState::Failed(format!(
                    "Couldn't load collection records: {error:#}"
                ))
            }
        }
        self.changed(cx);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use artisan_api::{ContentId, ContentRef, PlanStep, StepStatus};
    use gpui::AppContext as _;
    use gpui::TestAppContext;
    #[gpui::test]
    fn descriptor_changes_and_byte_limited_pages_keep_exact_record_bounds(cx: &mut TestAppContext) {
        // The HTTP client wakes GPUI from its network runtime.
        cx.executor().allow_parking();
        let identity = artisan_api::tls::Identity::generate("collection-window-test").unwrap();
        let pin = artisan_api::tls::fingerprint(identity.certificate_der().unwrap().as_ref());
        let client = crate::ForgeClient::new("127.0.0.1:1", &pin, &identity).unwrap();
        let host = cx.new(|cx| {
            HostData::new(
                crate::HostId("collection-test".into()),
                "test".into(),
                client,
                cx,
            )
        });
        let source = CollectionRef {
            source: ContentRef {
                id: ContentId("a".repeat(64)),
                bytes: 1_000_000,
                lines: 5001,
            },
            count: 5000,
        };
        let window = cx.new(|cx| CollectionWindow::<PlanStep>::new(&host, source.clone(), cx));
        window.update(cx, |window, cx| {
            window.task = None;
            let generation = window.generation;
            let page = CollectionRange {
                source: source.source.clone(),
                start: 0,
                end: 2,
                total: 5000,
                records: vec![
                    PlanStep {
                        text: "a".into(),
                        status: StepStatus::Pending,
                    },
                    PlanStep {
                        text: "b".into(),
                        status: StepStatus::Done,
                    },
                ],
            };
            window.finish(&source, generation, Ok(page), cx);
            assert!(window.page_for(0).is_some());
            assert!(window.page_for(1).is_some());
            assert!(
                window.page_for(2).is_none(),
                "count limit must not pretend byte-limited records were loaded"
            );
            let mut replaced = source.clone();
            replaced.source.id = ContentId("b".repeat(64));
            window.set_source(replaced, cx);
            assert!(window.page_for(0).is_none());
            window.finish(
                &source,
                generation,
                Err(anyhow::anyhow!("late old response")),
                cx,
            );
            assert!(matches!(window.state(), CollectionWindowState::Loading));
            assert_eq!(window.cached_pages(), 0);
        });
    }
}
