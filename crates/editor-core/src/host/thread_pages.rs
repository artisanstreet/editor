//! Bounded summary windows and direct lookup, separate from transcript Items.
use super::{HostData, HostEvent};
use artisan_api::{Thread, ThreadCursor, ThreadId, ThreadsQuery};
use gpui::{Context, Task};
use std::collections::{HashMap, HashSet, VecDeque};
const PAGE: u32 = 200;
const WINDOW: usize = 600;
const BYTES: usize = 8 * 1024 * 1024;
#[derive(Clone, Copy)]
pub enum ThreadPageDirection {
    Latest,
    Older,
}
#[derive(Default)]
pub struct ThreadPage {
    pub ids: Vec<ThreadId>,
    pub next: Option<ThreadCursor>,
    pub loading: bool,
    pub loaded: bool,
    pub error: Option<String>,
    pub at_latest: bool,
    task: Option<Task<()>>,
    changed: HashSet<ThreadId>,
}
#[derive(Default)]
pub(super) struct ThreadPages {
    pub(super) pages: [ThreadPage; 2],
    direct: HashMap<ThreadId, Task<()>>,
    queued: VecDeque<ThreadId>,
    recent: VecDeque<ThreadId>,
    errors: HashMap<ThreadId, String>,
}
impl HostData {
    pub fn thread_page(&self, archived: bool) -> &ThreadPage {
        &self.thread_pages.pages[usize::from(archived)]
    }
    pub fn thread_load_error(&self, id: &ThreadId) -> Option<&str> {
        self.thread_pages.errors.get(id).map(String::as_str)
    }
    pub(super) fn reset_thread_pages(&mut self, next: Option<ThreadCursor>) {
        self.thread_pages = ThreadPages::default();
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
            ..Default::default()
        };
        trim_page(&mut page, &self.threads, false);
        self.thread_pages.pages[0] = page;
    }
    pub fn load_threads(
        &mut self,
        archived: bool,
        direction: ThreadPageDirection,
        cx: &mut Context<Self>,
    ) {
        let index = usize::from(archived);
        let page = &mut self.thread_pages.pages[index];
        if page.loading {
            return;
        }
        let older = matches!(direction, ThreadPageDirection::Older);
        let cursor = if older {
            let Some(cursor) = page.next.clone() else {
                return;
            };
            Some(cursor)
        } else {
            None
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
        let client = self.client.clone();
        let query = ThreadsQuery {
            archived,
            before_activity: cursor.as_ref().map(|cursor| cursor.activity_seq),
            before_id: cursor.map(|cursor| cursor.id),
            limit: Some(PAGE),
            query: None,
        };
        page.task = Some(cx.spawn(async move |this, cx| {
            let result = client.threads(&query).await;
            let _ = this.update(cx, |this, cx| {
                let page = &mut this.thread_pages.pages[index];
                page.loading = false;
                page.loaded = true;
                page.task = None;
                match result {
                    Err(error) => page.error = Some(format!("Couldn't load threads: {error:#}")),
                    Ok(result) => {
                        if !older {
                            page.ids.retain(|id| {
                                this.threads
                                    .get(id)
                                    .is_some_and(|thread| thread.seq > started_seq)
                            });
                            page.at_latest = true;
                        }
                        page.next = result.next;
                        for thread in result.threads {
                            let id = thread.id.clone();
                            if this
                                .threads
                                .get(&id)
                                .is_none_or(|known| known.seq <= thread.seq)
                            {
                                this.threads.insert(id.clone(), thread);
                            }
                            if this.threads.get(&id).is_some_and(|thread| {
                                thread.archived == archived && thread.parent_thread_id.is_none()
                            }) && !page.ids.contains(&id)
                            {
                                page.ids.push(id);
                            }
                        }
                        for run in result.runs {
                            if this
                                .runs
                                .get(&run.thread_id)
                                .is_none_or(|known| known.seq <= run.seq)
                            {
                                this.runs.insert(run.thread_id.clone(), run);
                            }
                        }
                        if !older {
                            for id in &page.changed {
                                if this.threads.get(id).is_some_and(|thread| {
                                    thread.archived == archived && thread.parent_thread_id.is_none()
                                }) && !page.ids.contains(id)
                                {
                                    page.ids.push(id.clone());
                                }
                            }
                        }
                        page.ids.sort_by(|a, b| {
                            this.threads[b]
                                .activity_seq
                                .cmp(&this.threads[a].activity_seq)
                                .then_with(|| b.cmp(a))
                        });
                        trim_page(page, &this.threads, older);
                    }
                }
                page.changed.clear();
                this.prune_threads();
                cx.emit(HostEvent::Changed);
                cx.notify();
            });
        }));
        cx.emit(HostEvent::Changed);
        cx.notify();
    }
    pub fn ensure_thread(&mut self, id: &ThreadId, cx: &mut Context<Self>) {
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
        self.pump_thread_lookups(cx);
    }
    fn pump_thread_lookups(&mut self, cx: &mut Context<Self>) {
        while self.thread_pages.direct.len() < 4 {
            let Some(id) = self.thread_pages.queued.pop_back() else {
                break;
            };
            if self.threads.contains_key(&id) {
                continue;
            }
            self.start_thread_lookup(id, cx);
        }
    }
    fn start_thread_lookup(&mut self, id: ThreadId, cx: &mut Context<Self>) {
        let key = id.clone();
        let client = self.client.clone();
        let task = cx.spawn(async move |this, cx| {
            let result = client.thread(&id).await;
            let _ = this.update(cx, |this, cx| {
                this.thread_pages.direct.remove(&id);
                match result {
                    Ok(thread) => {
                        if this
                            .threads
                            .get(&id)
                            .is_none_or(|old| old.seq <= thread.seq)
                        {
                            this.threads.insert(id.clone(), thread);
                        }
                    }
                    Err(error) => {
                        this.thread_pages
                            .errors
                            .insert(id.clone(), format!("Couldn't load thread: {error:#}"));
                    }
                }
                this.prune_threads();
                this.pump_thread_lookups(cx);
                cx.emit(HostEvent::ThreadChanged { thread_id: id });
                cx.notify();
            });
        });
        self.thread_pages.direct.insert(key, task);
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
        // Keep updated versions until an overlapping request has merged. Otherwise an
        // archive event could be evicted and an older response could resurrect its row.
        for page in &mut self.thread_pages.pages {
            if page.loading {
                page.changed.insert(id.clone());
                if page.changed.len() > WINDOW {
                    page.task = None;
                    page.loading = false;
                    page.loaded = true;
                    page.changed.clear();
                    page.error = Some("The thread list changed while loading. Try again.".into());
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
            trim_page(page, &self.threads, false);
        }
        self.prune_threads();
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
        keep.extend(self.read_acks.pending.keys().cloned());
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
/// Eviction changes the next cursor only when removing the oldest records.
/// Removing the newest records while browsing backwards preserves the server cursor.
fn trim_page(page: &mut ThreadPage, threads: &HashMap<ThreadId, Thread>, evict_newest: bool) {
    let mut bytes: usize = page.ids.iter().map(|id| thread_bytes(&threads[id])).sum();
    let mut trimmed = false;
    while page.ids.len() > WINDOW || bytes > BYTES && page.ids.len() > 1 {
        let id = if evict_newest {
            page.at_latest = false;
            page.ids.remove(0)
        } else {
            page.ids.pop().unwrap()
        };
        bytes = bytes.saturating_sub(thread_bytes(&threads[&id]));
        trimmed = true;
    }
    if trimmed && !evict_newest {
        page.next = page.ids.last().map(|id| ThreadCursor {
            activity_seq: threads[id].activity_seq,
            id: id.clone(),
        });
    }
}

fn thread_bytes(thread: &Thread) -> usize {
    let config = &thread.config;
    std::mem::size_of::<Thread>()
        + thread.title.as_ref().map_or(0, String::len)
        + [
            &config.model,
            &config.effort,
            &config.speed,
            &config.variant,
        ]
        .into_iter()
        .map(|value| value.as_ref().map_or(0, String::len))
        .sum::<usize>()
        + config
            .options
            .iter()
            .map(|(key, value)| key.len() + json_bytes(value))
            .sum::<usize>()
}

fn json_bytes(value: &serde_json::Value) -> usize {
    std::mem::size_of::<serde_json::Value>()
        + match value {
            serde_json::Value::String(text) => text.len(),
            serde_json::Value::Array(values) => values.iter().map(json_bytes).sum(),
            serde_json::Value::Object(values) => values
                .iter()
                .map(|(key, value)| std::mem::size_of::<String>() + key.len() + json_bytes(value))
                .sum(),
            _ => 0,
        }
}

#[cfg(test)]
mod tests {
    use super::*;
    use artisan_api::{EngineConfig, EngineId, ProjectId};

    fn summaries(count: usize, payload: usize) -> (ThreadPage, HashMap<ThreadId, Thread>) {
        let mut threads = HashMap::new();
        let mut page = ThreadPage {
            at_latest: true,
            ..Default::default()
        };
        for index in (1..=count).rev() {
            let id = ThreadId::from(format!("thread-{index}"));
            let mut config = EngineConfig::new(EngineId::Claude);
            config.options.insert(
                "payload".into(),
                serde_json::Value::String("x".repeat(payload)),
            );
            threads.insert(
                id.clone(),
                Thread {
                    id: id.clone(),
                    project_id: ProjectId::from("project"),
                    title: None,
                    config,
                    activity_seq: index as u64,
                    activity_at: index as i64,
                    read_seq: 0,
                    marked_unread: false,
                    settled_seq: 0,
                    seq: index as u64,
                    created_at: 0,
                    updated_at: 0,
                    archived: false,
                    parent_thread_id: None,
                },
            );
            page.ids.push(id);
        }
        (page, threads)
    }

    #[test]
    fn moving_backwards_discards_newest_summaries_without_skipping_older_ones() {
        let (mut page, threads) = summaries(801, 0);
        page.next = Some(ThreadCursor {
            activity_seq: 1,
            id: ThreadId::from("thread-1"),
        });
        trim_page(&mut page, &threads, true);
        assert_eq!(page.ids.len(), WINDOW);
        assert_eq!(page.ids.first().unwrap(), &ThreadId::from("thread-600"));
        assert_eq!(page.next.unwrap().activity_seq, 1);
        assert!(!page.at_latest);
    }

    #[test]
    fn byte_limited_latest_page_points_before_retained_tail() {
        let (mut page, threads) = summaries(300, 40 * 1024);
        trim_page(&mut page, &threads, false);
        assert!(page.ids.len() < 300);
        assert!(
            page.ids
                .iter()
                .map(|id| thread_bytes(&threads[id]))
                .sum::<usize>()
                <= BYTES
        );
        assert_eq!(page.next.as_ref().unwrap().id, *page.ids.last().unwrap());
        assert!(page.at_latest);
    }
}
