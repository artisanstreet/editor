use aqua::mailbox::{Mailbox, SendError};
use artisan_api::{CatalogSkill, ThreadId};
use artisan_mentions::MessageVocabulary;
use editor_core::HostData;
use std::collections::HashMap;
use std::num::NonZeroUsize;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio_util::task::AbortOnDropHandle;

pub(crate) const TITLE_RECHECK: Duration = Duration::from_secs(1);

#[derive(Debug, Default)]
pub struct MentionSources {
    own: Option<ThreadId>,
    skills: Option<Arc<[CatalogSkill]>>,
    threads: usize,
    titles: HashMap<ThreadId, String>,
    checked: Option<Instant>,
    recheck: Option<Instant>,
    vocabulary: Arc<MessageVocabulary>,
}

fn mentionable_threads<'a>(
    host: &'a HostData,
    own: &'a ThreadId,
) -> impl Iterator<Item = (&'a ThreadId, &'a str)> {
    host.threads
        .values()
        .filter(move |thread| &thread.id != own)
        .filter_map(|thread| {
            let title = thread.title.as_deref()?;
            artisan_mentions::valid_thread_title(title).then_some((&thread.id, title))
        })
}

impl MentionSources {
    pub(crate) const fn vocabulary(&self) -> &Arc<MessageVocabulary> {
        &self.vocabulary
    }

    pub(crate) const fn recheck_at(&self) -> Option<Instant> {
        self.recheck
    }

    pub(crate) fn sync(&mut self, host: &HostData, own: &ThreadId, now: Instant) -> bool {
        let same_skills = match (&self.skills, &host.skill_catalog) {
            (Some(known), Some(current)) => Arc::ptr_eq(known, current),
            (None, None) => true,
            (Some(_), None) | (None, Some(_)) => false,
        };
        let same_scope =
            same_skills && self.own.as_ref() == Some(own) && self.threads == host.threads.len();
        if same_scope && self.titles_match(host, own) {
            self.recheck = None;
            return false;
        }
        let due = self
            .checked
            .map(|checked| checked + TITLE_RECHECK)
            .filter(|due| now < *due);
        if let (true, Some(due)) = (same_scope, due) {
            self.recheck = Some(due);
            return false;
        }
        self.checked = Some(now);
        self.recheck = None;
        self.own = Some(own.clone());
        self.threads = host.threads.len();
        self.skills.clone_from(&host.skill_catalog);
        self.titles = mentionable_threads(host, own)
            .map(|(id, title)| (id.clone(), title.to_owned()))
            .collect();
        let skills: Vec<&str> = self
            .skills
            .iter()
            .flat_map(|catalog| catalog.iter())
            .filter(|skill| skill.enabled)
            .map(|skill| skill.name.as_str())
            .collect();
        let titles: Vec<&str> = self.titles.values().map(String::as_str).collect();
        let vocabulary = MessageVocabulary::new(&skills, &titles);
        if *self.vocabulary == vocabulary {
            return false;
        }
        self.vocabulary = Arc::new(vocabulary);
        true
    }

    fn titles_match(&self, host: &HostData, own: &ThreadId) -> bool {
        let mut seen = 0;
        mentionable_threads(host, own).all(|(id, title)| {
            seen += 1;
            self.titles.get(id).is_some_and(|known| known == title)
        }) && seen == self.titles.len()
    }
}

pub struct MentionRecheck {
    timers: tokio::runtime::Handle,
    wake: Mailbox<()>,
    armed: Option<(Instant, AbortOnDropHandle<()>)>,
}

impl std::fmt::Debug for MentionRecheck {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MentionRecheck")
            .field("due", &self.armed.as_ref().map(|(due, _)| due))
            .finish_non_exhaustive()
    }
}

impl Default for MentionRecheck {
    fn default() -> Self {
        Self::on(editor_core::runtime::runtime().handle().clone())
    }
}

impl MentionRecheck {
    #[must_use]
    pub(crate) fn on(timers: tokio::runtime::Handle) -> Self {
        Self {
            timers,
            wake: Mailbox::new(NonZeroUsize::MIN),
            armed: None,
        }
    }

    pub(crate) fn arm(&mut self, due: Option<Instant>, now: Instant) {
        let Some(due) = due else {
            self.armed = None;
            return;
        };
        if self.armed.as_ref().is_some_and(|(armed, _)| *armed == due) {
            return;
        }
        let sender = self.wake.sender();
        let wait = due.saturating_duration_since(now);
        let task = self.timers.spawn(async move {
            tokio::time::sleep(wait).await;
            match sender.send(()) {
                Ok(()) | Err(SendError::Full(())) => {}
                Err(SendError::Closed(())) => {
                    tracing::debug!("the transcript closed before its mention recheck");
                }
            }
        });
        self.armed = Some((due, AbortOnDropHandle::new(task)));
    }

    pub(crate) fn bind(&self, ctx: &aqua::AppCtx<'_>, window: aqua::prelude::AquaWindowId) {
        ctx.bind_mailbox(window, &self.wake);
    }

    pub(crate) fn fire(&mut self) -> bool {
        let woke = !self.wake.drain().is_empty();
        if woke {
            self.armed = None;
        }
        woke
    }
}

#[cfg(test)]
#[path = "../../tests/conversation/mentions.rs"]
mod tests;
