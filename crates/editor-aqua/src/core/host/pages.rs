use super::{HostCompletion, HostData, HostEvent, HostMessage};
use artisan_api::{Thread, ThreadCursor, ThreadId, ThreadsQuery};
use std::collections::{HashMap, HashSet, VecDeque};
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
pub struct ThreadPage {
    pub ids: Vec<ThreadId>,
    pub next: Option<ThreadCursor>,
    pub loading: bool,
    pub loaded: bool,
    pub error: Option<String>,
    pub at_latest: bool,
    task: Option<AbortOnDropHandle<()>>,
    generation: u64,
    changed: HashSet<ThreadId>,
}

#[derive(Default)]
pub(super) struct ThreadPages {
    pages: [ThreadPage; 2],
    direct: HashMap<ThreadId, (u64, AbortOnDropHandle<()>)>,
    queued: VecDeque<ThreadId>,
    recent: VecDeque<ThreadId>,
    errors: HashMap<ThreadId, String>,
    removed: HashSet<ThreadId>,
    generation: u64,
}

impl HostData {
    pub fn thread_page(&self, archived: bool) -> &ThreadPage {
        &self.thread_pages.pages[usize::from(archived)]
    }
    pub fn fill_threads(&mut self) {
        let page = self.thread_page(false);
        if page.loaded && !page.loading && page.error.is_none() && page.next.is_some() {
            self.load_threads(false, ThreadPageDirection::Older);
        }
    }
    pub fn thread_load_error(&self, id: &ThreadId) -> Option<&str> {
        self.thread_pages.errors.get(id).map(String::as_str)
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
        let mut page = ThreadPage {
            ids,
            next,
            loaded: true,
            at_latest: true,
            ..ThreadPage::default()
        };
        self.thread_pages.pages[0] = page;
        self.fill_threads();
    }
    pub fn load_threads(&mut self, archived: bool, direction: ThreadPageDirection) {
        let page = &mut self.thread_pages.pages[usize::from(archived)];
        if page.loading {
            return;
        }
        let (cursor, after) = match direction {
            ThreadPageDirection::Latest => (None, None),
            ThreadPageDirection::Older => {
                let Some(cursor) = page.next.clone() else {
                    return;
                };
                (Some(cursor), None)
            }
            ThreadPageDirection::Newer => {
                let Some(first) = page.ids.first().filter(|_| !page.at_latest) else {
                    return;
                };
                (
                    None,
                    Some(ThreadCursor {
                        activity_seq: self.threads[first].activity_seq,
                        id: first.clone(),
                    }),
                )
            }
        };
        page.loading = true;
        page.error = None;
        page.changed.clear();
        let started_seq = self
            .threads
            .values()
            .map(|thread| thread.seq)
            .max()
            .unwrap_or(0);
        self.thread_pages.generation = self.thread_pages.generation.wrapping_add(1);
        let generation = self.thread_pages.generation;
        page.generation = generation;
        let query = ThreadsQuery {
            archived,
            before_activity: cursor.as_ref().map(|cursor| cursor.activity_seq),
            before_id: cursor.map(|cursor| cursor.id),
            after_activity: after.as_ref().map(|cursor| cursor.activity_seq),
            after_id: after.map(|cursor| cursor.id),
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
        page.task = Some(AbortOnDropHandle::new(task));
        self.pending_events.push(HostEvent::Changed);
    }
    pub(super) fn finish_thread_page(
        &mut self,
        archived: bool,
        direction: ThreadPageDirection,
        started_seq: artisan_api::Seq,
        generation: u64,
        result: anyhow::Result<artisan_api::ThreadsPage>,
    ) -> bool {
        let page = &mut self.thread_pages.pages[usize::from(archived)];
        if page.generation != generation || !page.loading {
            return false;
        }
        page.loading = false;
        page.loaded = true;
        page.task = None;
        match result {
            Err(error) => page.error = Some(format!("Couldn't load threads: {error:#}")),
            Ok(result) => self.merge_thread_page(archived, direction, started_seq, result),
        }
        self.thread_pages.pages[usize::from(archived)]
            .changed
            .clear();
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
            if self
                .threads
                .get(&id)
                .is_none_or(|known| known.seq <= thread.seq)
            {
                self.threads.insert(id.clone(), thread);
            }
            if self.threads.get(&id).is_some_and(|thread| {
                thread.archived == archived && thread.parent_thread_id.is_none()
            }) && !page.ids.contains(&id)
            {
                page.ids.push(id);
            }
        }
        for run in result.runs {
            if self
                .runs
                .get(&run.thread_id)
                .is_none_or(|known| known.seq <= run.seq)
            {
                self.runs.insert(run.thread_id.clone(), run);
            }
        }
        if page.at_latest {
            for id in &page.changed {
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
        self.thread_pages.errors.remove(id);
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
        result: anyhow::Result<Thread>,
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
            Ok(thread) if self.thread_pages.removed.contains(&id) => drop(thread),
            Ok(thread) => {
                if self
                    .threads
                    .get(&id)
                    .is_none_or(|old| old.seq <= thread.seq)
                {
                    self.threads.insert(id.clone(), thread);
                }
            }
            Err(error) => {
                self.thread_pages
                    .errors
                    .insert(id.clone(), format!("Couldn't load thread: {error:#}"));
            }
        }
        self.prune_threads();
        self.pump_thread_lookups();
        self.pending_events
            .push(HostEvent::ThreadChanged { thread_id: id });
        true
    }
    pub(super) fn remember_thread(&mut self, thread: Thread) {
        let id = thread.id.clone();
        if self
            .threads
            .get(&id)
            .is_some_and(|old| old.seq > thread.seq)
        {
            return;
        }
        let index = usize::from(thread.archived);
        let top_level = thread.parent_thread_id.is_none();
        self.threads.insert(id.clone(), thread);
        for page in &mut self.thread_pages.pages {
            if page.loading {
                page.changed.insert(id.clone());
                if page.changed.len() > CHANGES {
                    page.task = None;
                    page.loading = false;
                    page.loaded = true;
                    page.changed.clear();
                    page.error = Some("The thread list changed while loading.".into());
                }
            }
        }
        self.thread_pages.pages[1 - index]
            .ids
            .retain(|known| known != &id);
        let page = &mut self.thread_pages.pages[index];
        if top_level && page.loaded && (page.at_latest || page.ids.contains(&id)) {
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
    }
    pub(super) fn forget_thread(&mut self, id: &ThreadId) {
        let pages = &mut self.thread_pages;
        for page in &mut pages.pages {
            page.ids.retain(|known| known != id);
            page.changed.remove(id);
        }
        pages.direct.remove(id);
        pages.queued.retain(|known| known != id);
        pages.errors.insert(
            id.clone(),
            "This thread was deleted after a long time without activity.".into(),
        );
        pages.removed.insert(id.clone());
        self.threads.remove(id);
        self.runs.remove(id);
        self.read_acks.remove(id);
    }
    pub(super) fn prune_threads(&mut self) {
        let mut keep: HashSet<ThreadId> = self
            .thread_pages
            .pages
            .iter()
            .flat_map(|page| page.ids.iter().chain(page.changed.iter()).cloned())
            .chain(self.thread_pages.recent.iter().cloned())
            .collect();
        keep.extend(self.observed.borrow().iter().cloned());
        keep.extend(self.read_acks.keys().cloned());
        keep.extend(
            self.runs
                .iter()
                .filter(|(_, run)| run.status.is_live())
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
            .retain(|id, run| self.threads.contains_key(id) || run.status.is_live());
        self.waiting
            .retain(|id, items| self.threads.contains_key(id) || !items.is_empty());
        self.thread_pages
            .errors
            .retain(|id, _| self.thread_pages.recent.contains(id));
    }
}
