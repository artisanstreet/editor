use crate::core::{
    config_file::UpdateConfigError,
    delivery::{AdmissionOrder, Delivery, DeliverySender},
    hosts::{EditorFiles, EditorSettings},
    runtime::runtime,
    settings::{SetSettingError, Settings},
};
use std::{
    collections::HashMap,
    num::NonZeroUsize,
    sync::{Arc, Mutex, PoisonError},
};

#[derive(Clone, Debug)]
pub enum SettingsSave {
    Saved,
    Saving,
    Failed(Arc<SetSettingError>),
}

pub(crate) struct Saved {
    first: u64,
    through: u64,
    changes: Vec<(String, u64)>,
    result: Result<EditorSettings, UpdateConfigError>,
}

#[derive(Debug)]
pub(crate) struct Settled {
    pub(crate) changed: bool,
    pub(crate) first: u64,
    pub(crate) through: u64,
    pub(crate) failure: Option<Arc<SetSettingError>>,
}

struct Change {
    revision: u64,
    key: String,
    value: serde_json::Value,
}

enum Queue {
    Idle,
    Writing(Vec<Change>),
}

pub(crate) struct SettingsSaves {
    files: EditorFiles,
    queue: Arc<Mutex<Queue>>,
    saved: Delivery<Saved>,
    confirmed: EditorSettings,
    latest: u64,
    latest_by_key: HashMap<String, u64>,
    settled_through: u64,
    state: SettingsSave,
}

impl SettingsSaves {
    pub(crate) fn new(
        files: EditorFiles,
        wake: aqua::mailbox::Sender<()>,
        admission: AdmissionOrder,
        settings: &Settings,
    ) -> Self {
        Self {
            files,
            queue: Arc::new(Mutex::new(Queue::Idle)),
            saved: Delivery::with_order(
                NonZeroUsize::new(64).expect("invariant: 64 is not zero"),
                wake,
                admission,
            ),
            confirmed: settings.values(),
            latest: 0,
            latest_by_key: HashMap::new(),
            settled_through: 0,
            state: SettingsSave::Saved,
        }
    }

    pub(crate) const fn state(&self) -> &SettingsSave {
        &self.state
    }

    pub(crate) fn reload(&mut self, settings: &Settings) {
        self.confirmed = settings.values();
        self.state = if self.settled_through == self.latest {
            SettingsSave::Saved
        } else {
            SettingsSave::Saving
        };
    }

    pub(crate) fn pending_revision(&self, key: &str) -> Option<u64> {
        self.latest_by_key
            .get(key)
            .copied()
            .filter(|revision| *revision > self.settled_through)
    }

    pub(crate) fn submit(&mut self, key: String, value: serde_json::Value) -> u64 {
        self.latest += 1;
        let revision = self.latest;
        self.latest_by_key.insert(key.clone(), revision);
        self.state = SettingsSave::Saving;
        let change = Change {
            revision,
            key,
            value,
        };
        let mut queue = self.queue.lock().unwrap_or_else(PoisonError::into_inner);
        match &mut *queue {
            Queue::Writing(pending) => pending.push(change),
            Queue::Idle => {
                *queue = Queue::Writing(vec![change]);
                drop(runtime().spawn(write_queued(
                    self.files.clone(),
                    Arc::clone(&self.queue),
                    self.saved.sender(),
                )));
            }
        }
        revision
    }

    pub(crate) fn peek_order(&mut self) -> Option<u64> {
        self.saved.peek_order()
    }

    pub(crate) fn settle(&mut self, settings: &mut Settings) -> Option<Settled> {
        let saved = self.saved.drain_one()?;
        self.settled_through = saved.through;
        let (changed, failure) = match saved.result {
            Ok(stored) => {
                let validated = Settings::validated(stored);
                self.confirmed = validated.values();
                if saved.through == self.latest {
                    let changed = settings.values() != self.confirmed;
                    *settings = validated;
                    if matches!(self.state, SettingsSave::Saving) {
                        self.state = SettingsSave::Saved;
                    }
                    (changed, None)
                } else {
                    (false, None)
                }
            }
            Err(error) => self.fail(settings, saved.changes, error),
        };
        Some(Settled {
            changed,
            first: saved.first,
            through: saved.through,
            failure,
        })
    }

    fn fail(
        &mut self,
        settings: &mut Settings,
        changes: Vec<(String, u64)>,
        error: UpdateConfigError,
    ) -> (bool, Option<Arc<SetSettingError>>) {
        let last_key = changes
            .last()
            .map(|(key, _)| key.clone())
            .expect("invariant: a settings save carries at least one change");
        if let UpdateConfigError::Read(error) = error {
            let error = Arc::new(error);
            *settings = Settings::Unreadable(Arc::clone(&error));
            let failure = Arc::new(SetSettingError::Unreadable {
                key: last_key,
                source: error,
            });
            self.state = SettingsSave::Failed(Arc::clone(&failure));
            return (true, Some(failure));
        }
        let superseded = |key: &str, revision: u64| {
            self.latest_by_key
                .get(key)
                .is_some_and(|latest| *latest > revision)
        };
        let lost: Vec<String> = changes
            .into_iter()
            .filter(|(key, revision)| !superseded(key, *revision))
            .map(|(key, _)| key)
            .collect();
        let failure = Arc::new(SetSettingError::Save {
            key: lost.last().cloned().unwrap_or(last_key),
            source: error,
        });
        tracing::error!(
            error = %::diagnostics::chain(&*failure),
            rolled_back = lost.len(),
            "saving Editor settings failed"
        );
        if lost.is_empty() {
            return (false, Some(failure));
        }
        for key in &lost {
            settings.put(key, self.confirmed.get(key).cloned());
        }
        self.state = SettingsSave::Failed(Arc::clone(&failure));
        (true, Some(failure))
    }
}

async fn write_queued(files: EditorFiles, queue: Arc<Mutex<Queue>>, saved: DeliverySender<Saved>) {
    loop {
        let batch = {
            let mut queue = queue.lock().unwrap_or_else(PoisonError::into_inner);
            match &mut *queue {
                Queue::Writing(pending) if pending.is_empty() => {
                    *queue = Queue::Idle;
                    return;
                }
                Queue::Writing(pending) => std::mem::take(pending),
                Queue::Idle => unreachable!("invariant: only the settings writer idles its queue"),
            }
        };
        let first = batch
            .first()
            .map(|change| change.revision)
            .expect("invariant: a settings batch is never empty");
        let through = batch
            .last()
            .map(|change| change.revision)
            .expect("invariant: a settings batch is never empty");
        let changes = batch
            .iter()
            .map(|change| (change.key.clone(), change.revision))
            .collect();
        let files = files.clone();
        let result = tokio::task::spawn_blocking(move || {
            files.update_settings(|settings| {
                for change in batch {
                    settings.insert(change.key, change.value);
                }
            })
        })
        .await
        .expect("invariant: saving settings.json does not panic");
        if saved
            .send(Saved {
                first,
                through,
                changes,
                result,
            })
            .await
            .is_err()
        {
            return;
        }
    }
}
