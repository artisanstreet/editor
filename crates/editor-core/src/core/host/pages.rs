use super::{HostCompletion, HostData, HostEvent, HostMessage};
use crate::core::{client::CallError, hosts::HostId};
use artisan_api::{Thread, ThreadCursor, ThreadId, ThreadWindow, ThreadsQuery};
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;
use tokio_util::task::AbortOnDropHandle;

const PAGE: u32 = 200;
const CHANGES: usize = 600;

#[derive(Clone, Copy)]
pub enum ThreadPageDirection {
    Latest,
    Older,
    Newer,
}

#[derive(Default)]
enum PageLoad {
    #[default]
    Unloaded,
    Loading {
        generation: u64,
        first: bool,
        changed: HashSet<ThreadId>,
        _task: AbortOnDropHandle<()>,
    },
    Ready,
    Failed(String),
}

#[derive(Default)]
pub struct ThreadPage {
    ids: Vec<ThreadId>,
    next: Option<ThreadCursor>,
    at_latest: bool,
    load: PageLoad,
}

impl ThreadPage {
    #[must_use]
    pub fn ids(&self) -> &[ThreadId] {
        &self.ids
    }
    #[must_use]
    pub const fn has_older(&self) -> bool {
        self.next.is_some()
    }
    #[must_use]
    pub const fn at_latest(&self) -> bool {
        self.at_latest
    }
    #[must_use]
    pub const fn is_loading(&self) -> bool {
        matches!(self.load, PageLoad::Loading { .. })
    }
    #[must_use]
    pub const fn is_loaded(&self) -> bool {
        match self.load {
            PageLoad::Unloaded => false,
            PageLoad::Loading { first, .. } => !first,
            PageLoad::Ready | PageLoad::Failed(_) => true,
        }
    }
    #[must_use]
    pub fn error(&self) -> Option<&str> {
        match &self.load {
            PageLoad::Failed(error) => Some(error),
            PageLoad::Unloaded | PageLoad::Loading { .. } | PageLoad::Ready => None,
        }
    }
}

#[derive(Clone, Debug)]
pub enum ThreadUnavailable {
    Failed(Arc<LoadThreadError>),
    Deleted,
}

impl std::fmt::Display for ThreadUnavailable {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Failed(error) => {
                write!(
                    formatter,
                    "Couldn't load this thread: {}",
                    ::diagnostics::chain(&**error)
                )
            }
            Self::Deleted => {
                formatter.write_str("This thread was deleted after a long time without activity.")
            }
        }
    }
}

#[derive(Debug, thiserror::Error)]
#[error("load thread {thread} from {host}")]
pub struct LoadThreadError {
    pub host: HostId,
    pub thread: ThreadId,
    #[source]
    pub source: CallError,
}

#[derive(Default)]
pub(super) struct ThreadPages {
    pages: [ThreadPage; 2],
    direct: HashMap<ThreadId, (u64, AbortOnDropHandle<()>)>,
    queued: VecDeque<ThreadId>,
    recent: VecDeque<ThreadId>,
    unavailable: HashMap<ThreadId, ThreadUnavailable>,
    removed: HashSet<ThreadId>,
    generation: u64,
}

impl HostData {
    pub fn thread_page(&self, archived: bool) -> &ThreadPage {
        &self.thread_pages.pages[usize::from(archived)]
    }
    #[cfg(feature = "test-support")]
    pub fn test_thread_page_generation(&self, archived: bool) -> u64 {
        match &self.thread_pages.pages[usize::from(archived)].load {
            PageLoad::Loading { generation, .. } => *generation,
            PageLoad::Unloaded | PageLoad::Ready | PageLoad::Failed(_) => {
                unreachable!(
                    "invariant: the test asked for the generation of a page that is not loading"
                )
            }
        }
    }
    pub fn thread_unavailable(&self, id: &ThreadId) -> Option<&ThreadUnavailable> {
        self.thread_pages.unavailable.get(id)
    }
    pub fn fill_threads(&mut self) {
        let page = self.thread_page(false);
        if matches!(page.load, PageLoad::Ready) && page.next.is_some() {
            self.load_threads(false, ThreadPageDirection::Older);
        }
    }
    pub(super) fn reset_thread_pages(&mut self, next: Option<ThreadCursor>) {
        let generation = self.thread_pages.generation.wrapping_add(1);
        self.thread_pages = ThreadPages {
            generation,
            ..ThreadPages::default()
        };
        let mut ids: Vec<_> = self
            .threads
            .values()
            .filter(|thread| !thread.archived && thread.parent_thread_id.is_none())
            .map(|thread| thread.id.clone())
            .collect();
        ids.sort_by(|a, b| {
            self.threads[b]
                .activity_seq
                .cmp(&self.threads[a].activity_seq)
                .then_with(|| b.cmp(a))
        });
        ids.truncate(PAGE as usize);
        let page = ThreadPage {
            ids,
            next,
            at_latest: true,
            load: PageLoad::Ready,
        };
        self.thread_pages.pages[0] = page;
        self.fill_threads();
    }
    pub fn load_threads(&mut self, archived: bool, direction: ThreadPageDirection) {
        let page = &mut self.thread_pages.pages[usize::from(archived)];
        if page.is_loading() {
            return;
        }
        let first_load = !page.is_loaded();
        let window = match direction {
            ThreadPageDirection::Latest => ThreadWindow::Latest,
            ThreadPageDirection::Older => {
                let Some(cursor) = page.next.clone() else {
                    return;
                };
                ThreadWindow::Before(cursor)
            }
            ThreadPageDirection::Newer => {
                let Some(first) = page.ids.first().filter(|_| !page.at_latest) else {
                    return;
                };
                ThreadWindow::After(ThreadCursor {
                    activity_seq: self.threads[first].activity_seq,
                    id: first.clone(),
                })
            }
        };
        let started_seq = self
            .threads
            .values()
            .map(|thread| thread.seq)
            .max()
            .unwrap_or(0);
        self.thread_pages.generation = self.thread_pages.generation.wrapping_add(1);
        let generation = self.thread_pages.generation;
        let query = ThreadsQuery {
            archived,
            window,
            limit: Some(PAGE),
            query: None,
        };
        let client = self.client.clone();
        let sender = self.delivery.sender().map(HostMessage::Completion);
        let task = crate::core::runtime::runtime().spawn(async move {
            let result = client.threads(&query).await;
            let _ = sender
                .send(HostCompletion::ThreadPage {
                    archived,
                    direction,
                    started_seq,
                    generation,
                    result,
                })
                .await;
        });
        self.thread_pages.pages[usize::from(archived)].load = PageLoad::Loading {
            generation,
            first: first_load,
            changed: HashSet::new(),
            _task: AbortOnDropHandle::new(task),
        };
        self.pending_events.push(HostEvent::Changed);
    }
    pub(super) fn finish_thread_page(
        &mut self,
        archived: bool,
        direction: ThreadPageDirection,
        started_seq: artisan_api::Seq,
        generation: u64,
        result: Result<artisan_api::ThreadsPage, crate::core::client::CallError>,
    ) -> bool {
        let page = &mut self.thread_pages.pages[usize::from(archived)];
        let changed = match &mut page.load {
            PageLoad::Loading {
                generation: current,
                changed,
                ..
            } if *current == generation => std::mem::take(changed),
            PageLoad::Loading { .. }
            | PageLoad::Unloaded
            | PageLoad::Ready
            | PageLoad::Failed(_) => {
                return false;
            }
        };
        match result {
            Err(error) => {
                page.load = PageLoad::Failed(format!(
                    "Couldn't load threads: {}",
                    ::diagnostics::chain(&error)
                ));
            }
            Ok(result) => {
                page.load = PageLoad::Ready;
                self.merge_thread_page(archived, direction, started_seq, result, &changed);
            }
        }
        self.prune_threads();
        self.fill_threads();
        true
    }
    fn merge_thread_page(
        &mut self,
        archived: bool,
        direction: ThreadPageDirection,
        started_seq: artisan_api::Seq,
        result: artisan_api::ThreadsPage,
        changed: &HashSet<ThreadId>,
    ) {
        let older = matches!(direction, ThreadPageDirection::Older);
        let newer = matches!(direction, ThreadPageDirection::Newer);
        let page = &mut self.thread_pages.pages[usize::from(archived)];
        if newer {
            page.at_latest = !result.newer;
        } else {
            if !older {
                page.ids.retain(|id| {
                    self.threads
                        .get(id)
                        .is_some_and(|thread| thread.seq > started_seq)
                });
                page.at_latest = true;
            }
            page.next = result.next;
        }
        for thread in result.threads {
            let id = thread.id.clone();
            if self.thread_pages.removed.contains(&id) {
                continue;
            }
            self.accept_thread(thread);
            let page = &mut self.thread_pages.pages[usize::from(archived)];
            if self.threads.get(&id).is_some_and(|thread| {
                thread.archived == archived && thread.parent_thread_id.is_none()
            }) && !page.ids.contains(&id)
            {
                page.ids.push(id);
            }
        }
        for run in result.runs {
            self.remember_run(run);
        }
        let page = &mut self.thread_pages.pages[usize::from(archived)];
        if page.at_latest {
            for id in changed {
                if self.threads.get(id).is_some_and(|thread| {
                    thread.archived == archived && thread.parent_thread_id.is_none()
                }) && !page.ids.contains(id)
                {
                    page.ids.push(id.clone());
                }
            }
        }
        page.ids.sort_by(|a, b| {
            self.threads[b]
                .activity_seq
                .cmp(&self.threads[a].activity_seq)
                .then_with(|| b.cmp(a))
        });
    }

    pub fn ensure_thread(&mut self, id: &ThreadId) {
        if self.thread_pages.removed.contains(id) {
            return;
        }
        self.thread_pages.recent.retain(|known| known != id);
        self.thread_pages.recent.push_back(id.clone());
        while self.thread_pages.recent.len() > 32 {
            self.thread_pages.recent.pop_front();
        }
        if self.threads.contains_key(id) || self.thread_pages.direct.contains_key(id) {
            return;
        }
        self.thread_pages.unavailable.remove(id);
        self.thread_pages.queued.retain(|known| known != id);
        self.thread_pages.queued.push_back(id.clone());
        while self.thread_pages.queued.len() > 32 {
            self.thread_pages.queued.pop_front();
        }
        self.pump_thread_lookups();
    }
    fn pump_thread_lookups(&mut self) {
        while self.thread_pages.direct.len() < 4 {
            let Some(id) = self.thread_pages.queued.pop_back() else {
                break;
            };
            if self.threads.contains_key(&id) {
                continue;
            }
            self.thread_pages.generation = self.thread_pages.generation.wrapping_add(1);
            let generation = self.thread_pages.generation;
            let client = self.client.clone();
            let sender = self.delivery.sender().map(HostMessage::Completion);
            let task_id = id.clone();
            let task = crate::core::runtime::runtime().spawn(async move {
                let result = client.thread(&task_id).await;
                let _ = sender
                    .send(HostCompletion::ThreadLookup(task_id, generation, result))
                    .await;
            });
            self.thread_pages
                .direct
                .insert(id, (generation, AbortOnDropHandle::new(task)));
        }
    }
    pub(super) fn finish_thread_lookup(
        &mut self,
        id: ThreadId,
        generation: u64,
        result: Result<Thread, crate::core::client::CallError>,
    ) -> bool {
        if !self
            .thread_pages
            .direct
            .get(&id)
            .is_some_and(|(current, _)| *current == generation)
        {
            return false;
        }
        self.thread_pages.direct.remove(&id);
        match result {
            Ok(thread) => {
                self.accept_thread(thread);
            }
            Err(source) => {
                let error = LoadThreadError {
                    host: self.id.clone(),
                    thread: id.clone(),
                    source,
                };
                tracing::warn!(
                    host = %self.id,
                    thread = %id,
                    error = %::diagnostics::chain(&error),
                    "couldn't load a thread"
                );
                self.thread_pages
                    .unavailable
                    .insert(id.clone(), ThreadUnavailable::Failed(Arc::new(error)));
            }
        }
        self.prune_threads();
        self.pump_thread_lookups();
        self.pending_events
            .push(HostEvent::ThreadChanged { thread_id: id });
        true
    }
    pub(super) fn is_deleted(&self, id: &ThreadId) -> bool {
        self.thread_pages.removed.contains(id)
    }
    fn accept_thread(&mut self, thread: Thread) -> bool {
        if self.thread_pages.removed.contains(&thread.id)
            || self
                .threads
                .get(&thread.id)
                .is_some_and(|old| old.seq > thread.seq)
        {
            return false;
        }
        self.threads.insert(thread.id.clone(), thread);
        true
    }
    pub(super) fn remember_thread(&mut self, thread: Thread) -> bool {
        let id = thread.id.clone();
        let index = usize::from(thread.archived);
        let top_level = thread.parent_thread_id.is_none();
        if !self.accept_thread(thread) {
            return false;
        }
        for page in &mut self.thread_pages.pages {
            if let PageLoad::Loading { changed, .. } = &mut page.load {
                changed.insert(id.clone());
                if changed.len() > CHANGES {
                    page.load = PageLoad::Failed("The thread list changed while loading.".into());
                }
            }
        }
        self.thread_pages.pages[1 - index]
            .ids
            .retain(|known| known != &id);
        let page = &mut self.thread_pages.pages[index];
        if top_level && page.is_loaded() && (page.at_latest || page.ids.contains(&id)) {
            if !page.ids.contains(&id) {
                page.ids.push(id);
            }
            page.ids.sort_by(|a, b| {
                self.threads[b]
                    .activity_seq
                    .cmp(&self.threads[a].activity_seq)
                    .then_with(|| b.cmp(a))
            });
        }
        self.prune_threads();
        true
    }
    pub(super) fn forget_thread(&mut self, id: &ThreadId) {
        let pages = &mut self.thread_pages;
        for page in &mut pages.pages {
            page.ids.retain(|known| known != id);
            if let PageLoad::Loading { changed, .. } = &mut page.load {
                changed.remove(id);
            }
        }
        pages.direct.remove(id);
        pages.queued.retain(|known| known != id);
        pages
            .unavailable
            .insert(id.clone(), ThreadUnavailable::Deleted);
        pages.removed.insert(id.clone());
        self.threads.remove(id);
        self.runs.remove(id);
        self.stopping.remove(id);
        self.cancel_read_ack(id);
    }
    pub(super) fn prune_threads(&mut self) {
        let mut keep: HashSet<ThreadId> = self
            .thread_pages
            .pages
            .iter()
            .flat_map(|page| {
                let changed = match &page.load {
                    PageLoad::Loading { changed, .. } => Some(changed.iter()),
                    PageLoad::Unloaded | PageLoad::Ready | PageLoad::Failed(_) => None,
                };
                page.ids
                    .iter()
                    .chain(changed.into_iter().flatten())
                    .cloned()
            })
            .chain(self.thread_pages.recent.iter().cloned())
            .collect();
        keep.extend(self.observed.borrow().iter().cloned());
        keep.extend(self.read_acks.keys().cloned());
        keep.extend(
            self.runs
                .iter()
                .filter(|(_, run)| run.status().is_live())
                .map(|(id, _)| id.clone()),
        );
        keep.extend(
            self.waiting
                .iter()
                .filter(|(_, waiting)| !waiting.is_empty())
                .map(|(id, _)| id.clone()),
        );

        self.threads.retain(|id, _| keep.contains(id));
        self.runs
            .retain(|id, run| self.threads.contains_key(id) || run.status().is_live());
        self.waiting
            .retain(|id, items| self.threads.contains_key(id) || !items.is_empty());
        self.thread_pages
            .unavailable
            .retain(|id, _| self.thread_pages.recent.contains(id));
    }
}
