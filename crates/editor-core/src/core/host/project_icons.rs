use super::{BlobImage, HostCompletion, HostData, HostEvent, HostMessage};
use aqua::images::ImageHandle;
use artisan_api::{BlobHash, ProjectId};
use std::{
    collections::{HashMap, VecDeque},
    sync::Arc,
    time::{Duration, Instant},
};
use tokio_util::task::AbortOnDropHandle;

struct Entry {
    hash: Option<BlobHash>,
    checked: Instant,
    failed: bool,
}
impl Entry {
    fn expire(&mut self) {
        self.checked = Instant::now()
            .checked_sub(Duration::from_secs(300))
            .unwrap();
    }
    fn fresh(&self, now: Instant) -> bool {
        let ttl = if self.failed || self.hash.is_none() {
            Duration::from_secs(5)
        } else {
            Duration::from_secs(300)
        };
        now.saturating_duration_since(self.checked) < ttl
    }
}
#[derive(Default)]
pub(super) struct ProjectIcons {
    entries: HashMap<ProjectId, Entry>,
    reading: HashMap<ProjectId, (u64, AbortOnDropHandle<()>)>,
    order: VecDeque<ProjectId>,
    generation: u64,
}
impl ProjectIcons {
    pub(super) fn forget(&mut self, project: &ProjectId) {
        self.reading.remove(project);
        self.entries.remove(project);
        self.order.retain(|id| id != project);
    }
    pub(super) fn refresh(&mut self, project: &ProjectId) {
        self.reading.remove(project);
        if let Some(entry) = self.entries.get_mut(project) {
            entry.expire();
        }
    }
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
    #[cfg(feature = "test-support")]
    pub fn test_project_icon(&mut self, project: ProjectId, hash: BlobHash) -> bool {
        self.project_icons.generation = self.project_icons.generation.wrapping_add(1);
        let generation = self.project_icons.generation;
        let task = crate::core::runtime::runtime().spawn(std::future::pending());
        self.project_icons
            .reading
            .insert(project.clone(), (generation, AbortOnDropHandle::new(task)));
        self.finish_project_icon(project, generation, Ok(Some(hash)))
    }
    pub fn project_icon(&self, project: &ProjectId) -> Option<Arc<ImageHandle>> {
        let hash = self.project_icons.entries.get(project)?.hash.as_ref()?;
        match self.image(hash)? {
            BlobImage::Ready(image) => Some(image),
            BlobImage::Loading | BlobImage::Failed(_) => None,
        }
    }
    #[must_use]
    pub fn project_icon_checked(&self, project: &ProjectId) -> bool {
        self.project_icons.entries.contains_key(project)
    }
    pub fn project_icons_failed(&self) -> bool {
        self.project_icons.entries.values().any(|entry| {
            entry.failed
                || entry
                    .hash
                    .as_ref()
                    .is_some_and(|hash| matches!(self.image(hash), Some(BlobImage::Failed(_))))
        })
    }
    pub fn load_project_icon(&mut self, project: &ProjectId) {
        if !self.projects.contains_key(project) || !self.is_live() {
            return;
        }
        let now = Instant::now();
        let hash = self
            .project_icons
            .entries
            .get(project)
            .and_then(|entry| entry.hash.clone());
        if let Some(hash) = hash {
            self.load_image(hash, false);
        }
        if let Some(entry) = self.project_icons.entries.get(project) {
            let fresh = entry.fresh(now);
            self.project_icons.order.retain(|id| id != project);
            self.project_icons.order.push_back(project.clone());
            if fresh {
                return;
            }
        }
        if self.project_icons.reading.contains_key(project) || self.project_icons.reading.len() >= 4
        {
            return;
        }
        self.project_icons.generation = self.project_icons.generation.wrapping_add(1);
        let generation = self.project_icons.generation;
        let client = self.client.clone();
        let sender = self.delivery.sender().map(HostMessage::Completion);
        let project = project.clone();
        let key = project.clone();
        let task_project = project.clone();
        let task = crate::core::runtime::runtime().spawn(async move {
            let result = client
                .project_icon(&task_project)
                .await
                .map(|blob| blob.map(|blob| blob.hash));
            let _ = sender
                .send(HostCompletion::ProjectIcon(key, generation, result))
                .await;
        });
        self.project_icons
            .reading
            .insert(project, (generation, AbortOnDropHandle::new(task)));
    }
    pub fn retry_project_icons(&mut self) {
        let mut failed_images = Vec::new();
        for entry in self.project_icons.entries.values_mut() {
            if entry.failed {
                entry.checked = Instant::now().checked_sub(Duration::from_secs(5)).unwrap();
            }
            failed_images.extend(entry.hash.clone());
        }
        for hash in failed_images {
            if matches!(self.image(&hash), Some(BlobImage::Failed(_))) {
                self.load_image(hash, true);
            }
        }
        self.pending_events.push(HostEvent::Changed);
    }
    pub(super) fn finish_project_icon(
        &mut self,
        project: ProjectId,
        generation: u64,
        result: Result<Option<BlobHash>, crate::core::client::CallError>,
    ) -> bool {
        if !self
            .project_icons
            .reading
            .get(&project)
            .is_some_and(|(current, _)| *current == generation)
        {
            return false;
        }
        self.project_icons.reading.remove(&project);
        let failed = result.is_err();
        let last = self.project_icons.entries.remove(&project);
        let hash = match result {
            Ok(hash) => hash,
            Err(error) => {
                tracing::debug!(%project, "project icon read failed: {}", ::diagnostics::chain(&error));
                last.as_ref().and_then(|entry| entry.hash.clone())
            }
        };
        let changed = last
            .as_ref()
            .is_none_or(|entry| entry.hash != hash || entry.failed != failed);
        self.project_icons.entries.insert(
            project.clone(),
            Entry {
                hash: hash.clone(),
                checked: Instant::now(),
                failed,
            },
        );
        self.project_icons.order.retain(|id| id != &project);
        self.project_icons.order.push_back(project);
        while self.project_icons.entries.len() > 128 {
            if let Some(oldest) = self.project_icons.order.pop_front() {
                self.project_icons.entries.remove(&oldest);
            }
        }
        if let Some(hash) = hash {
            self.load_image(hash, false);
        }
        changed
    }
}
