//! One renderer-owned publication and hot-page cache, fenced by immutable input.
use super::projection_worker::{self, NativeInput, NativeWindow};
use gpui::{App, Pixels, SharedString, Task, TextRun};
use std::cell::RefCell;
use std::collections::VecDeque;
use std::ops::Range;
use std::rc::Rc;
use std::sync::Arc;

const MAX_ENTRIES: usize = 16;
const PAGES_PER_SOURCE: usize = 4;
#[derive(Clone)]
pub enum NativePreparation {
    Pending,
    Ready(Arc<NativeWindow>),
    Failed(String),
}
struct Entry {
    key: SharedString,
    source: NativeInput,
    font_size: Pixels,
    width: Pixels,
    runs: Arc<[TextRun]>,
    generation: u64,
    pages: VecDeque<Arc<NativeWindow>>,
    task: Option<Task<()>>,
    error: Option<String>,
}
impl Entry {
    fn matches(
        &self,
        source: &NativeInput,
        font_size: Pixels,
        width: Pixels,
        runs: &Arc<[TextRun]>,
    ) -> bool {
        self.source.same(source)
            && self.font_size == font_size
            && self.width == width
            && Arc::ptr_eq(&self.runs, runs)
    }
}
#[derive(Default)]
struct Cache {
    entries: VecDeque<Entry>,
    generation: u64,
}
#[derive(Default)]
pub(super) struct ProjectionOwner {
    cache: Rc<RefCell<Cache>>,
}
impl Cache {
    fn push(&mut self, entry: Entry) {
        self.entries.push_back(entry);
        while self.entries.len() > MAX_ENTRIES {
            self.entries.pop_front();
        }
    }
}
impl std::fmt::Debug for ProjectionOwner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProjectionOwner")
            .field("entries", &self.cache.borrow().entries.len())
            .finish()
    }
}
impl ProjectionOwner {
    pub fn clear(&self) {
        self.cache.borrow_mut().entries.clear();
    }

    pub fn retry(&self, key: &SharedString) {
        if let Some(entry) = self
            .cache
            .borrow_mut()
            .entries
            .iter_mut()
            .find(|entry| &entry.key == key)
        {
            entry.error = None;
        }
    }

    /// Only the exact currently owned source/style/width can supply topology.
    pub fn row_count(
        &self,
        key: &SharedString,
        source: &NativeInput,
        font_size: Pixels,
        width: Pixels,
        runs: &Arc<[TextRun]>,
    ) -> Option<usize> {
        self.cache
            .borrow()
            .entries
            .iter()
            .find(|entry| &entry.key == key && entry.matches(source, font_size, width, runs))?
            .pages
            .back()
            .map(|page| page.row_count())
    }
    pub fn prepare(
        &self,
        key: SharedString,
        source: NativeInput,
        font_size: Pixels,
        width: Pixels,
        runs: Arc<[TextRun]>,
        range: Range<usize>,
        cx: &mut App,
        notify: impl Fn(&mut App) + 'static,
    ) -> NativePreparation {
        if !projection_worker::admits(&source, &runs) {
            self.cache
                .borrow_mut()
                .entries
                .retain(|entry| entry.key != key);
            return NativePreparation::Failed(
                "This paragraph exceeds the native shaping admission budget; open the full source"
                    .into(),
            );
        }
        let mut cache = self.cache.borrow_mut();
        let old = cache
            .entries
            .iter()
            .position(|entry| entry.key == key)
            .and_then(|index| cache.entries.remove(index));
        let mut entry = match old {
            Some(entry) if entry.matches(&source, font_size, width, &runs) => entry,
            _ => {
                cache.generation = cache.generation.wrapping_add(1);
                Entry {
                    key: key.clone(),
                    source: source.clone(),
                    font_size,
                    width,
                    runs: runs.clone(),
                    generation: cache.generation,
                    pages: VecDeque::new(),
                    task: None,
                    error: None,
                }
            }
        };
        if let Some(page) = entry.pages.iter().find(|page| {
            page.start <= range.start
                && page.start.saturating_add(page.rows.len()) >= range.end.min(page.row_count())
        }) {
            let result = NativePreparation::Ready(page.clone());
            cache.push(entry);
            return result;
        }
        if entry.task.is_some() {
            cache.push(entry);
            return NativePreparation::Pending;
        }
        if let Some(error) = entry.error.clone() {
            cache.push(entry);
            return NativePreparation::Failed(error);
        }
        // A verified ready page owns the exact immutable source. Reuse it on
        // cold scrolling rather than repeat the complete admitted source fetch.
        let shape_source = entry
            .pages
            .back()
            .map(|page| NativeInput::Inline(page.text.clone()))
            .unwrap_or(source);
        let task = match projection_worker::start(cx, shape_source, font_size, runs, width, range) {
            Ok(task) => task,
            Err(error) => {
                // Busy is explicit control state, never inferred from text.
                if error == projection_worker::StartError::Busy {
                    let owner = Rc::downgrade(&self.cache);
                    let generation = entry.generation;
                    entry.task = Some(cx.spawn(async move |cx| {
                        projection_worker::wait_for_owner().await;
                        let _ =
                            cx.update(|cx| {
                                let Some(owner) = owner.upgrade() else { return };
                                let mut cache = owner.borrow_mut();
                                let Some(entry) = cache.entries.iter_mut().find(|entry| {
                                    entry.key == key && entry.generation == generation
                                }) else {
                                    return;
                                };
                                entry.task = None;
                                drop(cache);
                                notify(cx);
                            });
                    }));
                    cache.push(entry);
                    return NativePreparation::Pending;
                }
                let error = error.to_string();
                entry.error = Some(error.clone());
                cache.push(entry);
                return NativePreparation::Failed(error);
            }
        };
        let owner = Rc::downgrade(&self.cache);
        let generation = entry.generation;
        entry.task = Some(cx.spawn(async move |cx| {
            let result = task.await;
            let _ = cx.update(|cx| {
                let Some(owner) = owner.upgrade() else { return };
                let mut cache = owner.borrow_mut();
                let Some(entry) = cache
                    .entries
                    .iter_mut()
                    .find(|entry| entry.key == key && entry.generation == generation)
                else {
                    return;
                };
                match result {
                    Ok(page) => {
                        if entry
                            .pages
                            .back()
                            .is_some_and(|previous| previous.row_count() != page.row_count())
                        {
                            entry.pages.clear();
                        }
                        entry.pages.push_back(Arc::new(page));
                        while entry.pages.len() > PAGES_PER_SOURCE {
                            entry.pages.pop_front();
                        }
                    }
                    Err(error) => entry.error = Some(error),
                }
                entry.task = None;
                drop(cache);
                notify(cx);
            });
        }));
        cache.push(entry);
        NativePreparation::Pending
    }
}
