use super::session::{CookieChange, Session};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};
use tokio::sync::watch;

pub enum SessionChange {
    Cookies(Vec<CookieChange>),
    NeedsRenewal,
    Expired(String),
}

impl SessionChange {
    fn apply(self, session: &mut Session) {
        match self {
            Self::Cookies(changes) => session.apply(&changes),
            Self::NeedsRenewal => session.needs_renewal = true,
            Self::Expired(reason) => session.expired = Some(reason),
        }
    }
}

enum Queue {
    Idle,
    Writing(Vec<SessionChange>),
}

pub struct SessionWriter {
    home: PathBuf,
    queue: Arc<Mutex<Queue>>,
    writing: Arc<watch::Sender<bool>>,
}

impl SessionWriter {
    pub fn new(home: &Path) -> Self {
        Self {
            home: home.to_owned(),
            queue: Arc::new(Mutex::new(Queue::Idle)),
            writing: Arc::new(watch::Sender::new(false)),
        }
    }

    pub fn submit(&self, change: SessionChange) {
        let mut queue = self.queue.lock().unwrap_or_else(PoisonError::into_inner);
        match &mut *queue {
            Queue::Writing(pending) => pending.push(change),
            Queue::Idle => {
                *queue = Queue::Writing(vec![change]);
                self.writing.send_replace(true);
                let (home, queue, writing) = (
                    self.home.clone(),
                    Arc::clone(&self.queue),
                    Arc::clone(&self.writing),
                );
                drop(tokio::task::spawn_blocking(move || {
                    write_queued(&home, &queue, &writing);
                }));
            }
        }
    }

    #[cfg(test)]
    pub async fn flushed(&self) {
        let mut writing = self.writing.subscribe();
        writing
            .wait_for(|writing| !*writing)
            .await
            .expect("invariant: the session writer outlives its own watch");
    }
}

fn write_queued(home: &Path, queue: &Mutex<Queue>, writing: &watch::Sender<bool>) {
    loop {
        let batch = {
            let mut queue = queue.lock().unwrap_or_else(PoisonError::into_inner);
            match &mut *queue {
                Queue::Writing(pending) if pending.is_empty() => {
                    *queue = Queue::Idle;
                    writing.send_replace(false);
                    return;
                }
                Queue::Writing(pending) => std::mem::take(pending),
                Queue::Idle => unreachable!("invariant: only the session writer idles its queue"),
            }
        };
        let changes = batch.len();
        if let Err(error) = Session::update(home, |session| {
            for change in batch {
                change.apply(session);
            }
        }) {
            tracing::warn!(
                changes,
                "keeping claude.ai's session changes failed: {}",
                diagnostics::chain(&error)
            );
        }
    }
}
