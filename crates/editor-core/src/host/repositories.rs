//! What git said about each project, read on demand by visible views.
//! Reads coalesce for two seconds; a failure keeps the last answer and
//! backs off for five. Each read is owned by its task: a project change or
//! a reconnect drops it, so a late answer about an old path can't land.
use super::{HostData, HostEvent};
use crate::connection::ConnectionStatus;
use artisan_api::{ProjectId, ProjectRepository};
use gpui::{Context, Task};
use std::collections::{HashMap, VecDeque};
use std::time::{Duration, Instant};

/// Projects whose answer is kept.
const KEPT: usize = 128;
/// Reads at once.
const READS: usize = 4;

pub(super) struct Entry {
    value: Option<ProjectRepository>,
    pub(super) checked: Instant,
    failed: bool,
}
impl Entry {
    fn fresh(&self) -> bool {
        let seconds = if self.failed { 5 } else { 2 };
        self.checked.elapsed() < Duration::from_secs(seconds)
    }
}

#[derive(Default)]
pub(super) struct RepositoryCache {
    pub(super) entries: HashMap<ProjectId, Entry>,
    pub(super) reading: HashMap<ProjectId, Task<()>>,
    /// Least recently used first.
    pub(super) order: VecDeque<ProjectId>,
}

impl RepositoryCache {
    pub(super) fn get(&self, project: &ProjectId) -> Option<&ProjectRepository> {
        self.entries.get(project)?.value.as_ref()
    }

    /// Forgets everything, cancelling reads in flight.
    pub(super) fn clear(&mut self) {
        *self = Self::default();
    }

    /// Forgets one project, cancelling its read.
    pub(super) fn forget(&mut self, project: &ProjectId) {
        self.reading.remove(project);
        self.entries.remove(project);
        self.order.retain(|id| id != project);
    }
}

impl HostData {
    /// What git said about a project; `None` until read or outside a
    /// repository.
    #[must_use]
    pub fn repository(&self, project: &ProjectId) -> Option<&ProjectRepository> {
        self.repositories.get(project)
    }

    /// Reads a project's repository unless a fresh answer is known.
    pub fn load_repository(&mut self, project: &ProjectId, refresh: bool, cx: &mut Context<Self>) {
        if !self.projects.contains_key(project) {
            return;
        }
        let cache = &mut self.repositories;
        if self.status != ConnectionStatus::Live
            || cache.reading.contains_key(project)
            || cache.reading.len() >= READS
        {
            return;
        }
        if cache.entries.contains_key(project) {
            cache.order.retain(|id| id != project);
            cache.order.push_back(project.clone());
        }
        if !refresh && cache.entries.get(project).is_some_and(Entry::fresh) {
            return;
        }
        let client = self.client.clone();
        let project = project.clone();
        let key = project.clone();
        let task = cx.spawn(async move |this, cx| {
            let result = client.project_repository(&project).await;
            this.update(cx, |host, cx| host.finish_repository(project, result, cx))
                .ok();
        });
        cache.reading.insert(key, task);
    }

    pub(super) fn finish_repository(
        &mut self,
        project: ProjectId,
        result: anyhow::Result<Option<ProjectRepository>>,
        cx: &mut Context<Self>,
    ) {
        let cache = &mut self.repositories;
        cache.reading.remove(&project);
        let failed = result.is_err();
        let value = match result {
            Ok(repository) => repository,
            Err(error) => {
                tracing::debug!(%project, "repository read failed: {error:#}");
                cache
                    .entries
                    .get(&project)
                    .and_then(|entry| entry.value.clone())
            }
        };
        cache.entries.insert(
            project.clone(),
            Entry {
                value,
                checked: Instant::now(),
                failed,
            },
        );
        cache.order.retain(|id| id != &project);
        cache.order.push_back(project);
        while cache.entries.len() > KEPT {
            if let Some(oldest) = cache.order.pop_front() {
                cache.entries.remove(&oldest);
            }
        }
        cx.emit(HostEvent::Changed);
        cx.notify();
    }
}

#[cfg(test)]
mod tests {
    use crate::{ConnectionStatus, ForgeClient, HostData, HostId};
    use artisan_api::{Event, Project, ProjectId, ProjectRepository};
    use gpui::{AppContext, Entity, TestAppContext};
    use std::time::{Duration, Instant};

    fn host(cx: &mut TestAppContext) -> Entity<HostData> {
        let identity = artisan_api::tls::Identity::generate("repository-test").unwrap();
        let fingerprint =
            artisan_api::tls::fingerprint(identity.certificate_der().unwrap().as_ref());
        let client = ForgeClient::new("127.0.0.1:1", &fingerprint, &identity).unwrap();
        cx.new(|cx| {
            let mut host =
                HostData::new(HostId("repository-test".into()), "Test".into(), client, cx);
            host.stop_following();
            host.status = ConnectionStatus::Live;
            host
        })
    }
    fn project(seq: artisan_api::Seq) -> Project {
        Project {
            id: ProjectId::from("test-project"),
            path: "/project".into(),
            name: "Project".into(),
            seq,
            created_at: 0,
            removed: false,
        }
    }
    fn repository(branch: &str) -> ProjectRepository {
        ProjectRepository {
            branch: Some(branch.into()),
            host: None,
            web_url: None,
            worktree: None,
        }
    }

    #[gpui::test]
    fn a_project_change_cancels_its_repository_read(cx: &mut TestAppContext) {
        let host = host(cx);
        host.update(cx, |host, cx| {
            let old = project(1);
            let id = old.id.clone();
            host.apply_event(Event::Project { project: old }, Some(1), cx);
            host.finish_repository(id.clone(), Ok(Some(repository("old"))), cx);
            host.load_repository(&id, true, cx);
            assert!(host.repositories.reading.contains_key(&id));
            host.apply_event(
                Event::Project {
                    project: project(2),
                },
                Some(2),
                cx,
            );
            assert!(!host.repositories.reading.contains_key(&id));
            assert!(host.repository(&id).is_none());
        });
    }

    #[gpui::test]
    fn expired_repository_reads_coalesce_and_failure_preserves_last_observation(
        cx: &mut TestAppContext,
    ) {
        let host = host(cx);
        host.update(cx, |host, cx| {
            let project = project(1);
            let id = project.id.clone();
            host.apply_event(Event::Project { project }, Some(1), cx);
            host.finish_repository(id.clone(), Ok(Some(repository("main"))), cx);
            host.load_repository(&id, false, cx);
            assert!(
                host.repositories.reading.is_empty(),
                "fresh observation avoids another read"
            );
            host.repositories.entries.get_mut(&id).unwrap().checked =
                Instant::now().checked_sub(Duration::from_secs(3)).unwrap();
            host.load_repository(&id, false, cx);
            host.load_repository(&id, false, cx);
            assert_eq!(host.repositories.reading.len(), 1);
            host.finish_repository(id.clone(), Err(anyhow::anyhow!("refused")), cx);
            assert_eq!(
                host.repository(&id).unwrap().branch.as_deref(),
                Some("main")
            );
            host.load_repository(&id, false, cx);
            assert!(
                host.repositories.reading.is_empty(),
                "failed refresh has a backoff"
            );
            assert_eq!(
                host.repositories.order.len(),
                1,
                "refresh does not accumulate LRU entries"
            );
        });
    }
}
