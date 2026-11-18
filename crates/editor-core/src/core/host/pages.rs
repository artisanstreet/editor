use super::{HostCompletion, HostData, HostEvent, HostMessage};
use crate::core::{client::CallError, hosts::HostId};
use artisan_api::{Thread, ThreadCursor, ThreadId, ThreadWindow, ThreadsQuery};
use std::cmp::Ordering;
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;
use tokio_util::task::AbortOnDropHandle;

const PAGE: u32 = 200;
const RETAINED: usize = 5 * PAGE as usize;
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
    members: HashSet<ThreadId>,
    next: Option<ThreadCursor>,
    at_latest: bool,
    load: PageLoad,
}

fn newest_first(a: &Thread, b: &Thread) -> Ordering {
    b.activity_seq
        .cmp(&a.activity_seq)
        .then_with(|| b.id.cmp(&a.id))
}

impl ThreadPage {
    fn place(&self, thread: &Thread, threads: &HashMap<ThreadId, Thread>) -> usize {
        self.ids
            .partition_point(|id| newest_first(&threads[id], thread) == Ordering::Less)
    }

    fn insert(&mut self, id: &ThreadId, threads: &HashMap<ThreadId, Thread>) {
        if self.members.insert(id.clone()) {
            let at = self.place(&threads[id], threads);
            self.ids.insert(at, id.clone());
        }
    }

    fn remove(
        &mut self,
        id: &ThreadId,
        stored: Option<&Thread>,
        threads: &HashMap<ThreadId, Thread>,
    ) {
        if !self.members.remove(id) {
            return;
        }
        let at = stored
            .map(|thread| self.place(thread, threads))
            .filter(|at| self.ids.get(*at) == Some(id))
            .or_else(|| self.ids.iter().position(|known| known == id))
            .expect("invariant: every page member is listed in the page");
        self.ids.remove(at);
    }

    fn retain(&mut self, mut keep: impl FnMut(&ThreadId) -> bool) {
        self.ids.retain(|id| keep(id));
        self.members = self.ids.iter().cloned().collect();
    }

    fn window(&mut self, older: bool, threads: &HashMap<ThreadId, Thread>) {
        let Some(excess) = self
            .ids
            .len()
            .checked_sub(RETAINED)
            .filter(|excess| *excess > 0)
        else {
            return;
        };
        let dropped: Vec<ThreadId> = if older {
            self.at_latest = false;
            self.ids.drain(..excess).collect()
        } else {
            let dropped = self.ids.split_off(RETAINED);
            self.next = self.ids.last().map(|id| ThreadCursor {
                activity_seq: threads[id].activity_seq,
                id: id.clone(),
            });
            dropped
        };
        for id in &dropped {
            self.members.remove(id);
        }
    }

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
    removed: HashMap<ThreadId, u64>,
    generation: u64,
}

impl ThreadPages {
    fn retire_removed(&mut self) {
        let oldest = self
            .pages
            .iter()
            .filter_map(|page| match page.load {
                PageLoad::Loading { generation, .. } => Some(generation),
                PageLoad::Unloaded | PageLoad::Ready | PageLoad::Failed(_) => None,
            })
            .chain(self.direct.values().map(|(generation, _)| *generation))
            .min();
        match oldest {
            Some(oldest) => self.removed.retain(|_, removed| oldest <= *removed),
            None => self.removed.clear(),
        }
    }

    fn deleted(&self, id: &ThreadId) -> bool {
        self.removed.contains_key(id)
            || matches!(self.unavailable.get(id), Some(ThreadUnavailable::Deleted))
    }
}

enum Accepted {
    Refused,
    New,
    Replaced(Thread),
}

fn listed_in(thread: &Thread, archived: bool) -> bool {
    thread.archived == archived && thread.parent_thread_id.is_none()
}

fn inbox_worthy(thread: &Thread) -> bool {
    let settled = thread.settled_seq > 0 && thread.settled_seq >= thread.activity_seq;
    listed_in(thread, false) && thread.is_unread() && !settled
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
    fn store_thread(&mut self, thread: Thread) -> Option<Thread> {
        let id = thread.id.clone();
        let mut placed = [false; 2];
        for (index, page) in self.thread_pages.pages.iter_mut().enumerate() {
            if page.members.contains(&id) {
                let stored = self.threads.get(&id);
                page.remove(&id, stored, &self.threads);
                placed[index] = true;
            }
        }
        let old = self.threads.insert(id.clone(), thread);
        for (index, page) in self.thread_pages.pages.iter_mut().enumerate() {
            if placed[index] {
                page.insert(&id, &self.threads);
            }
        }
        old
    }
    pub(super) fn reset_thread_pages(&mut self, next: Option<ThreadCursor>) {
        let generation = self.thread_pages.generation.wrapping_add(1);
        self.thread_pages = ThreadPages {
            generation,
            ..ThreadPages::default()
        };
        let mut threads: Vec<&Thread> = self
            .threads
            .values()
            .filter(|thread| listed_in(thread, false))
            .collect();
        threads.sort_by(|a, b| newest_first(a, b));
        threads.truncate(PAGE as usize);
        let ids: Vec<ThreadId> = threads
            .into_iter()
            .map(|thread| thread.id.clone())
            .collect();
        let members = ids.iter().cloned().collect();
        self.thread_pages.pages[0] = ThreadPage {
            ids,
            members,
            next,
            at_latest: true,
            load: PageLoad::Ready,
        };
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
        self.thread_pages.retire_removed();
        self.prune_threads();
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
        let index = usize::from(archived);
        let page = &mut self.thread_pages.pages[index];
        if newer {
            page.at_latest = !result.newer;
        } else {
            if !older {
                let threads = &self.threads;
                page.retain(|id| {
                    threads
                        .get(id)
                        .is_some_and(|thread| thread.seq > started_seq)
                });
                page.at_latest = true;
            }
            page.next = result.next;
        }
        for thread in result.threads {
            let id = thread.id.clone();
            if self.thread_pages.deleted(&id) {
                continue;
            }
            self.accept_thread(thread);
            if self
                .threads
                .get(&id)
                .is_some_and(|thread| listed_in(thread, archived))
            {
                self.thread_pages.pages[index].insert(&id, &self.threads);
            }
        }
        for run in result.runs {
            self.remember_run(run);
        }
        let page = &mut self.thread_pages.pages[index];
        if page.at_latest {
            for id in changed {
                if self
                    .threads
                    .get(id)
                    .is_some_and(|thread| listed_in(thread, archived))
                {
                    page.insert(id, &self.threads);
                }
            }
        }
        page.window(older, &self.threads);
    }

    pub fn ensure_thread(&mut self, id: &ThreadId) {
        if self.thread_pages.deleted(id) {
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
        self.thread_pages.retire_removed();
        self.prune_threads();
        self.pump_thread_lookups();
        self.pending_events
            .push(HostEvent::ThreadChanged { thread_id: id });
        true
    }
    pub(super) fn is_deleted(&self, id: &ThreadId) -> bool {
        self.thread_pages.deleted(id)
    }
    fn accept_thread(&mut self, thread: Thread) -> Accepted {
        if self.thread_pages.deleted(&thread.id)
            || self
                .threads
                .get(&thread.id)
                .is_some_and(|old| old.seq > thread.seq)
        {
            return Accepted::Refused;
        }
        self.store_thread(thread)
            .map_or(Accepted::New, Accepted::Replaced)
    }
    pub(super) fn remember_thread(&mut self, thread: Thread) -> bool {
        let id = thread.id.clone();
        let index = usize::from(thread.archived);
        let top_level = thread.parent_thread_id.is_none();
        let old = match self.accept_thread(thread) {
            Accepted::Refused => return false,
            Accepted::New => None,
            Accepted::Replaced(old) => Some(old),
        };
        for page in &mut self.thread_pages.pages {
            if let PageLoad::Loading { changed, .. } = &mut page.load {
                changed.insert(id.clone());
                if changed.len() > CHANGES {
                    page.load = PageLoad::Failed("The thread list changed while loading.".into());
                }
            }
        }
        let moved = old
            .as_ref()
            .is_some_and(|old| usize::from(old.archived) != index);
        if moved {
            self.thread_pages.pages[1 - index].remove(&id, None, &self.threads);
        }
        let page = &mut self.thread_pages.pages[index];
        let joined = top_level && page.is_loaded() && page.at_latest && !page.members.contains(&id);
        if joined {
            page.insert(&id, &self.threads);
        }
        if old.is_none() || moved || joined {
            self.prune_threads();
        }
        true
    }
    pub(super) fn forget_thread(&mut self, id: &ThreadId) {
        let pages = &mut self.thread_pages;
        for page in &mut pages.pages {
            page.remove(id, None, &self.threads);
            if let PageLoad::Loading { changed, .. } = &mut page.load {
                changed.remove(id);
            }
        }
        pages.direct.remove(id);
        pages.queued.retain(|known| known != id);
        pages
            .unavailable
            .insert(id.clone(), ThreadUnavailable::Deleted);
        pages.removed.insert(id.clone(), pages.generation);
        pages.retire_removed();
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
        keep.extend(
            self.threads
                .values()
                .filter(|thread| inbox_worthy(thread))
                .map(|thread| thread.id.clone()),
        );
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
