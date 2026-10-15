//! A bounded view of one immutable source, independent of transcript Item pages.
use crate::{HostData, HostEvent};
use artisan_api::{ContentRange, ContentRef};
use gpui::{Context, Entity, EventEmitter, Subscription, Task, WeakEntity};
use std::{collections::VecDeque, sync::Arc};

#[derive(Clone)]
pub enum ContentWindowState {
    Loading,
    Ready(Arc<ContentRange>),
    Failed(String),
}
pub struct ContentWindowEvent;
impl EventEmitter<ContentWindowEvent> for ContentWindow {}

pub struct ContentWindow {
    host: WeakEntity<HostData>,
    source: ContentRef,
    state: ContentWindowState,
    pages: VecDeque<Arc<ContentRange>>,
    requested: u64,
    generation: u64,
    task: Option<Task<()>>,
    _events: Subscription,
}
impl ContentWindow {
    pub fn new(host: &Entity<HostData>, source: ContentRef, cx: &mut Context<Self>) -> Self {
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
            state: ContentWindowState::Loading,
            pages: VecDeque::new(),
            requested: 0,
            generation: 0,
            task: None,
            _events: events,
        };
        this.request(0, cx);
        this
    }
    pub fn source(&self) -> &ContentRef {
        &self.source
    }
    pub fn state(&self) -> &ContentWindowState {
        &self.state
    }
    /// Changing a payload identity cancels the old task and releases its bytes.
    pub fn set_source(&mut self, source: ContentRef, cx: &mut Context<Self>) {
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
        cx.emit(ContentWindowEvent);
        cx.notify();
    }
    fn request_inner(&mut self, offset: u64, retry: bool, cx: &mut Context<Self>) {
        let offset = offset.min(self.source.bytes);
        if !retry {
            if self.task.is_some() && self.requested == offset {
                return;
            }
            if let Some(index) = self.pages.iter().position(|page| {
                page.start <= offset && (offset < page.end || offset == page.total)
            }) {
                let page = self.pages.remove(index).unwrap();
                self.state = ContentWindowState::Ready(page.clone());
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
            self.state = ContentWindowState::Failed("This Forge is no longer connected.".into());
            self.changed(cx);
            return;
        };
        let client = host.read(cx).client.clone();
        let source = self.source.clone();
        let generation = self.generation;
        self.state = ContentWindowState::Loading;
        self.task = Some(cx.spawn(async move |this, cx| {
            let result = client
                .content_range(&source, offset, artisan_api::CONTENT_RANGE_BYTES)
                .await;
            let _ = this.update(cx, |this, cx| this.finish(&source, generation, result, cx));
        }));
        self.changed(cx);
    }
    fn finish(
        &mut self,
        source: &ContentRef,
        generation: u64,
        result: anyhow::Result<ContentRange>,
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
                while self.pages.len() > 3 {
                    self.pages.pop_front();
                }
                self.state = ContentWindowState::Ready(page);
            }
            Err(error) => {
                self.state =
                    ContentWindowState::Failed(format!("Couldn't load source text: {error:#}"))
            }
        }
        self.changed(cx);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{AppContext, TestAppContext};

    fn window(cx: &mut TestAppContext) -> Entity<ContentWindow> {
        let identity = artisan_api::tls::Identity::generate("content-window-test").unwrap();
        let pin = artisan_api::tls::fingerprint(identity.certificate_der().unwrap().as_ref());
        let client = crate::ForgeClient::new("127.0.0.1:1", &pin, &identity).unwrap();
        let host =
            cx.new(|cx| HostData::new(crate::HostId("test".into()), "test".into(), client, cx));
        let source = ContentRef {
            id: artisan_api::ContentId("a".repeat(64)),
            bytes: 1_000_000,
            lines: 1,
        };
        let window = cx.new(|cx| ContentWindow::new(&host, source, cx));
        window.update(cx, |window, _| window.task = None);
        window
    }
    fn page(source: &ContentRef, offset: u64) -> ContentRange {
        ContentRange {
            id: source.id.clone(),
            start: offset,
            end: offset + 65536,
            total: source.bytes,
            text: "a".repeat(65536),
        }
    }

    #[gpui::test]
    fn stale_source_and_request_completions_do_not_replace_current_text(cx: &mut TestAppContext) {
        let window = window(cx);
        window.update(cx, |window, cx| {
            let old = window.source.clone();
            let old_generation = window.generation;
            window.generation += 1;
            window.finish(&old, old_generation, Ok(page(&old, 0)), cx);
            assert!(matches!(window.state(), ContentWindowState::Loading));
            assert!(window.pages.is_empty());
            window.source.id = artisan_api::ContentId("b".repeat(64));
            window.finish(
                &old,
                window.generation,
                Err(anyhow::anyhow!("late old error")),
                cx,
            );
            assert!(matches!(window.state(), ContentWindowState::Loading));
            let current = window.source.clone();
            window.finish(&current, window.generation, Ok(page(&current, 0)), cx);
            let ContentWindowState::Ready(actual) = window.state() else {
                panic!("current completion missing")
            };
            assert_eq!(actual.id, current.id);
        });
    }

    #[gpui::test]
    fn range_cache_retains_three_pages_and_reuses_loaded_offsets(cx: &mut TestAppContext) {
        let window = window(cx);
        window.update(cx, |window, cx| {
            let source = window.source.clone();
            for offset in [0, 65536, 131072, 196608] {
                window.finish(&source, window.generation, Ok(page(&source, offset)), cx);
            }
            assert_eq!(window.pages.len(), 3);
            assert_eq!(
                window
                    .pages
                    .iter()
                    .map(|page| page.text.len())
                    .sum::<usize>(),
                3 * 65536
            );
            assert_eq!(window.pages.front().unwrap().start, 65536);
            window.request(65540, cx);
            assert!(window.task.is_none());
            let ContentWindowState::Ready(actual) = window.state() else {
                panic!("cached range was not reused")
            };
            assert_eq!(actual.start, 65536);
            assert_eq!(window.pages.back().unwrap().start, 65536);
        });
    }
}
