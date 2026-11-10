use std::collections::HashSet;

use artisan_api::ThreadId;

use editor_core::HostData;

#[derive(Default)]
pub struct ArchiveForm {
    pub host: Option<String>,
    pub restoring: HashSet<ThreadId>,
    pub error: Option<String>,
    pub ids: Vec<ThreadId>,
    generation: u64,
    dirty: bool,
}

pub struct ArchiveRequest {
    pub host: String,
    pub thread: ThreadId,
    generation: u64,
}

impl ArchiveForm {
    pub fn invalidate(&mut self) {
        self.dirty = true;
    }

    pub fn sync(&mut self, host: Option<&HostData>) {
        let id = host.map(|host| host.id.0.clone());
        if self.host != id {
            self.host = id;
            self.dirty = true;
            self.generation += 1;
            self.restoring.clear();
            self.error = None;
        }
        if !self.dirty {
            return;
        }
        self.dirty = false;
        self.ids = host
            .map(|host| {
                host.thread_page(true)
                    .ids
                    .iter()
                    .filter(|id| host.threads.get(*id).is_some_and(|thread| thread.archived))
                    .cloned()
                    .collect()
            })
            .unwrap_or_default();
    }

    pub fn begin(&mut self, thread: ThreadId) -> Option<ArchiveRequest> {
        let host = self.host.clone()?;
        if !self.restoring.insert(thread.clone()) {
            return None;
        }
        self.error = None;
        Some(ArchiveRequest {
            host,
            thread,
            generation: self.generation,
        })
    }

    pub fn complete(&mut self, request: &ArchiveRequest, result: Result<(), String>) {
        if self.host.as_ref() != Some(&request.host) || self.generation != request.generation {
            return;
        }
        self.restoring.remove(&request.thread);
        if let Err(error) = result {
            self.error = Some(format!("Couldn't unarchive the thread: {error}"));
        }
    }
}
