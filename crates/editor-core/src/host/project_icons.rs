//! Each project's icon, read on demand by the views that show it (the
//! sidebar's thread rows, the composer's project menu). An answer is kept
//! five minutes, a missing icon or a failure five seconds; a failure keeps
//! the last icon. Each read is owned by its task: a project change or a
//! reconnect drops it, so a late answer about an old path can't land.
//! An entry holds its decoded pixels, so screenshots filling the shared
//! image cache never send a menu back to folder glyphs; a project that
//! keeps its path keeps its icon through a reconnect while it is read
//! again.
use super::{BlobImage, HostData, HostEvent};
use crate::connection::ConnectionStatus;
use artisan_api::{BlobHash, ProjectId};
use gpui::{Context, RenderImage, Task};
use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Projects whose answer is kept.
const KEPT: usize = 128;
/// Reads at once.
const READS: usize = 4;

pub(super) struct Entry {
    pub(super) hash: Option<BlobHash>,
    pub(super) checked: Instant,
    pub(super) failed: bool,
    /// The icon's pixels once decoded, held past the image cache's bound.
    pub(super) image: Option<Arc<RenderImage>>,
}
impl Entry {
    /// Due for another read at the next showing.
    #[expect(
        clippy::unwrap_used,
        reason = "the monotonic clock is far more than five seconds past its origin by now"
    )]
    pub(super) fn expire(&mut self) {
        self.checked = Instant::now()
            .checked_sub(Duration::from_secs(300))
            .unwrap();
    }

    pub(super) fn fresh(&self, now: Instant) -> bool {
        let ttl = if self.failed || self.hash.is_none() {
            Duration::from_secs(5)
        } else {
            Duration::from_secs(300)
        };
        now.saturating_duration_since(self.checked) < ttl
    }
}

#[derive(Default)]
pub(super) struct ProjectIconCache {
    pub(super) entries: HashMap<ProjectId, Entry>,
    pub(super) reading: HashMap<ProjectId, Task<()>>,
    /// Least recently used first.
    pub(super) order: VecDeque<ProjectId>,
}

impl ProjectIconCache {
    /// Forgets one project, cancelling its read.
    pub(super) fn forget(&mut self, project: &ProjectId) {
        self.reading.remove(project);
        self.entries.remove(project);
        self.order.retain(|id| id != project);
    }

    /// Cancels a project's read and reads it again at the next showing,
    /// keeping its icon up meanwhile.
    pub(super) fn refresh(&mut self, project: &ProjectId) {
        self.reading.remove(project);
        if let Some(entry) = self.entries.get_mut(project) {
            entry.expire();
        }
    }

    /// Cancels every read and forgets the projects `kept` refuses; the
    /// rest keep showing their icon until it is read again.
    pub(super) fn reset(&mut self, kept: impl Fn(&ProjectId) -> bool) {
        self.reading.clear();
        self.entries.retain(|id, _| kept(id));
        self.order.retain(|id| self.entries.contains_key(id));
        for entry in self.entries.values_mut() {
            entry.expire();
        }
    }
}

impl HostData {
    /// A project's icon, once read and decoded.
    #[must_use]
    pub fn project_icon(&self, project: &ProjectId) -> Option<Arc<RenderImage>> {
        let entry = self.project_icons.entries.get(project)?;
        let hash = entry.hash.as_ref()?;
        match self.image(hash) {
            Some(BlobImage::Ready(image)) => Some(image),
            _ => entry.image.clone(),
        }
    }

    /// Whether any project's last icon read failed.
    #[must_use]
    pub fn project_icons_failed(&self) -> bool {
        self.project_icons
            .entries
            .values()
            .any(|entry| entry.failed)
    }

    /// Reads a project's icon unless a fresh answer is known, and keeps
    /// its pixels loaded.
    pub fn load_project_icon(&mut self, project: &ProjectId, cx: &mut Context<Self>) {
        if !self.projects.contains_key(project) || self.status != ConnectionStatus::Live {
            return;
        }
        let now = Instant::now();
        let cache = &mut self.project_icons;
        if let Some(entry) = cache.entries.get_mut(project) {
            if let Some(hash) = &entry.hash {
                match self.images.get(hash) {
                    Some(BlobImage::Ready(image)) => entry.image = Some(image),
                    // Pixels held here are still reachable to the image
                    // cache, so this never downloads them again.
                    _ => self.images.load(hash.clone(), false, &self.client, cx),
                }
            }
            let fresh = entry.fresh(now);
            cache.order.retain(|id| id != project);
            cache.order.push_back(project.clone());
            if fresh {
                return;
            }
        }
        let cache = &mut self.project_icons;
        if cache.reading.contains_key(project) || cache.reading.len() >= READS {
            return;
        }
        let client = self.client.clone();
        let project = project.clone();
        let key = project.clone();
        let task = cx.spawn(async move |this, cx| {
            let result = client.project_icon(&project).await;
            this.update(cx, |host, cx| {
                host.finish_project_icon(
                    project,
                    result.map(|blob| blob.map(|blob| blob.hash)),
                    cx,
                );
            })
            .ok();
        });
        cache.reading.insert(key, task);
    }

    /// Reads every failed icon again the next time it is shown.
    #[expect(
        clippy::unwrap_used,
        reason = "the monotonic clock is far more than five seconds past its origin by now"
    )]
    pub fn retry_project_icons(&mut self, cx: &mut Context<Self>) {
        for entry in self.project_icons.entries.values_mut() {
            if entry.failed {
                entry.checked = Instant::now().checked_sub(Duration::from_secs(5)).unwrap();
            }
        }
        cx.emit(HostEvent::Changed);
        cx.notify();
    }

    pub(super) fn finish_project_icon(
        &mut self,
        project: ProjectId,
        result: anyhow::Result<Option<BlobHash>>,
        cx: &mut Context<Self>,
    ) {
        let cache = &mut self.project_icons;
        cache.reading.remove(&project);
        let failed = result.is_err();
        let last = cache.entries.remove(&project);
        let hash = match result {
            Ok(hash) => hash,
            Err(error) => {
                tracing::debug!(%project, "project icon read failed: {error:#}");
                last.as_ref().and_then(|entry| entry.hash.clone())
            }
        };
        // The same icon keeps its pixels through the read.
        let image = last
            .filter(|last| last.hash == hash)
            .and_then(|last| last.image);
        cache.entries.insert(
            project.clone(),
            Entry {
                hash: hash.clone(),
                checked: Instant::now(),
                failed,
                image,
            },
        );
        cache.order.retain(|id| id != &project);
        cache.order.push_back(project);
        while cache.entries.len() > KEPT {
            if let Some(oldest) = cache.order.pop_front() {
                cache.entries.remove(&oldest);
            }
        }
        if let Some(hash) = hash {
            self.load_image(hash, false, cx);
        }
        cx.emit(HostEvent::ImagesChanged);
        cx.notify();
    }
}

#[cfg(test)]
mod tests {
    use super::Entry;
    use crate::{ConnectionStatus, ForgeClient, HostData, HostId};
    use artisan_api::{BlobHash, Event, Project, ProjectId};
    use gpui::{AppContext, Entity, TestAppContext};
    use std::time::{Duration, Instant};

    fn host(cx: &mut TestAppContext) -> Entity<HostData> {
        let identity = artisan_api::tls::Identity::generate("project-icon-test").unwrap();
        let fingerprint =
            artisan_api::tls::fingerprint(identity.certificate_der().unwrap().as_ref());
        let client = ForgeClient::new("127.0.0.1:1", &fingerprint, &identity).unwrap();
        cx.new(|cx| {
            let mut host = HostData::new(
                HostId("project-icon-test".into()),
                "Test".into(),
                client,
                cx,
            );
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

    #[test]
    fn icons_expire_after_success_missing_and_failure() {
        let now = Instant::now();
        let mut entry = Entry {
            hash: Some(BlobHash("image".into())),
            failed: false,
            checked: now.checked_sub(Duration::from_secs(299)).unwrap(),
            image: None,
        };
        assert!(entry.fresh(now));
        assert!(!entry.fresh(now + Duration::from_secs(1)));
        entry.hash = None;
        entry.checked = now.checked_sub(Duration::from_secs(4)).unwrap();
        assert!(entry.fresh(now));
        assert!(!entry.fresh(now + Duration::from_secs(1)));
        entry.hash = Some(BlobHash("last-good".into()));
        entry.failed = true;
        assert!(
            !entry.fresh(now + Duration::from_secs(1)),
            "failed refresh uses failure TTL even with retained pixels"
        );
    }

    #[gpui::test]
    fn a_project_change_cancels_its_icon_read_and_reads_coalesce(cx: &mut TestAppContext) {
        let host = host(cx);
        host.update(cx, |host, cx| {
            let old = project(1);
            let id = old.id.clone();
            host.apply_event(Event::Project { project: old }, Some(1), cx);
            host.finish_project_icon(id.clone(), Ok(None), cx);
            host.load_project_icon(&id, cx);
            assert!(
                host.project_icons.reading.is_empty(),
                "fresh answer is kept"
            );
            host.project_icons.entries.get_mut(&id).unwrap().checked =
                Instant::now().checked_sub(Duration::from_secs(6)).unwrap();
            host.load_project_icon(&id, cx);
            host.load_project_icon(&id, cx);
            assert_eq!(host.project_icons.reading.len(), 1);
            assert_eq!(host.project_icons.order.len(), 1);
            host.apply_event(
                Event::Project {
                    project: project(2),
                },
                Some(2),
                cx,
            );
            assert!(host.project_icons.reading.is_empty());
            assert!(
                host.project_icons.entries.contains_key(&id),
                "same path keeps its icon while it is read again"
            );
            host.load_project_icon(&id, cx);
            assert_eq!(host.project_icons.reading.len(), 1);
            host.apply_event(
                Event::Project {
                    project: Project {
                        path: "/moved".into(),
                        ..project(3)
                    },
                },
                Some(3),
                cx,
            );
            assert!(host.project_icons.reading.is_empty());
            assert!(host.project_icons.entries.is_empty());
            assert!(host.project_icons.order.is_empty());
        });
    }
}
