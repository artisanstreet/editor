use super::{HostData, HostEvent};
use artisan_api::{
    BlobHash, EngineConfig, Item, ItemId, ItemKind, ProjectId, Seq, Status, Thread, ThreadId,
};
use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

const LOCAL_POSITIONS: Seq = 1 << 62;

static NEXT_ATTEMPT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct SendAttempt(u64);

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum HandBack {
    #[error("the draft gave the message back: {}", .0.headline())]
    Failed(crate::core::Problem),
    #[error("the composer that sent the message closed before Forge answered")]
    Closed,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SendSettlement {
    Accepted,
    HandedBack(HandBack),
}

#[must_use = "a send the Editor shows settles through its receipt"]
pub struct SendReceipt {
    sender: crate::core::delivery::MappedSender<super::HostCompletion, super::HostMessage>,
    attempt: SendAttempt,
    bound: bool,
}

impl SendReceipt {
    #[must_use]
    pub const fn attempt(&self) -> SendAttempt {
        self.attempt
    }

    pub const fn bind(&mut self) {
        self.bound = true;
    }

    pub fn settle(mut self, settlement: SendSettlement) {
        self.deliver(settlement);
    }

    fn deliver(&mut self, settlement: SendSettlement) {
        if std::mem::take(&mut self.bound) {
            let sender = self.sender.clone();
            let attempt = self.attempt;
            crate::core::runtime::runtime().spawn(async move {
                sender
                    .offer(super::HostCompletion::SendSettled(attempt, settlement))
                    .await;
            });
        }
    }
}

impl Drop for SendReceipt {
    fn drop(&mut self) {
        self.deliver(SendSettlement::HandedBack(HandBack::Closed));
    }
}

impl std::fmt::Debug for SendReceipt {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SendReceipt")
            .field("attempt", &self.attempt)
            .field("bound", &self.bound)
            .finish_non_exhaustive()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LocalSend {
    pub attempt: SendAttempt,
    pub thread: ThreadId,
    pub message: ItemId,
    pub text: String,
    pub images: Vec<BlobHash>,
    pub create: Option<NewThread>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NewThread {
    pub project: ProjectId,
    pub config: EngineConfig,
}

enum Creation {
    Sending {
        thread: Thread,
        attempt: SendAttempt,
    },
    Failed {
        thread: Thread,
        cause: HandBack,
    },
}

impl Creation {
    const fn thread(&self) -> &Thread {
        match self {
            Self::Sending { thread, .. } | Self::Failed { thread, .. } => thread,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Posting {
    Sending,
    Accepted,
}

struct LocalMessage {
    item: Arc<Item>,
    attempt: SendAttempt,
    posting: Posting,
}

#[derive(Default)]
pub(super) struct Sends {
    creations: BTreeMap<ThreadId, Creation>,
    messages: HashMap<ThreadId, Vec<LocalMessage>>,
    positions: Seq,
}

impl Sends {
    pub(super) fn confirm_thread(&mut self, id: &ThreadId) {
        self.creations.remove(id);
    }

    pub(super) fn confirm_item(&mut self, thread: &ThreadId, item: &ItemId) -> bool {
        let Some(messages) = self.messages.get_mut(thread) else {
            return false;
        };
        let before = messages.len();
        messages.retain(|message| &message.item.id != item);
        let confirmed = messages.len() != before;
        if messages.is_empty() {
            self.messages.remove(thread);
        }
        confirmed
    }

    pub(super) fn forget(&mut self, id: &ThreadId) {
        self.creations.remove(id);
        self.messages.remove(id);
    }

    pub(super) fn reset(&mut self) {
        self.messages.retain(|_, messages| {
            messages.retain(|message| message.posting == Posting::Sending);
            !messages.is_empty()
        });
    }

    pub(super) fn provisional(&self) -> impl Iterator<Item = &Thread> {
        self.creations.values().map(Creation::thread)
    }

    pub(super) fn kept(&self) -> impl Iterator<Item = &ThreadId> {
        self.creations.keys()
    }

    fn failed(&self, id: &ThreadId) -> Option<&HandBack> {
        match self.creations.get(id) {
            Some(Creation::Failed { cause, .. }) => Some(cause),
            Some(Creation::Sending { .. }) | None => None,
        }
    }

    pub(super) fn creating(&self, id: &ThreadId) -> bool {
        matches!(self.creations.get(id), Some(Creation::Sending { .. }))
    }

    pub(super) fn posting(&self, id: &ThreadId, attempt: SendAttempt) -> bool {
        self.messages.get(id).is_some_and(|messages| {
            messages
                .iter()
                .any(|message| message.attempt == attempt && message.posting == Posting::Sending)
        })
    }

    pub(super) fn posting_attempts(&self, id: &ThreadId) -> Vec<SendAttempt> {
        self.messages
            .get(id)
            .into_iter()
            .flatten()
            .filter(|message| message.posting == Posting::Sending)
            .map(|message| message.attempt)
            .collect()
    }

    fn attempted(&self, attempt: SendAttempt) -> Option<(ThreadId, ItemId)> {
        self.messages.iter().find_map(|(thread, messages)| {
            messages
                .iter()
                .find(|message| message.attempt == attempt)
                .map(|message| (thread.clone(), message.item.id.clone()))
        })
    }

    fn local_item(&mut self, send: &LocalSend) -> Arc<Item> {
        self.positions = self.positions.wrapping_add(1);
        let now = artisan_api::now();
        Arc::new(Item {
            id: send.message.clone(),
            thread_id: send.thread.clone(),
            position: LOCAL_POSITIONS + self.positions,
            seq: 0,
            run_id: None,
            parent_id: None,
            status: Status::Done,
            created_at: now,
            updated_at: now,
            kind: ItemKind::UserMessage {
                text: send.text.clone().into(),
                images: send.images.clone(),
                steer: false,
                from_agent: None,
                sender: None,
            },
        })
    }
}

fn provisional_thread(id: ThreadId, create: NewThread, activity_seq: Seq) -> Thread {
    let now = artisan_api::now();
    Thread {
        id,
        project_id: create.project,
        title: None,
        config: create.config,
        activity_seq,
        activity_at: now,
        read_seq: activity_seq,
        marked_unread: false,
        settled_seq: 0,
        seq: 0,
        created_at: now,
        updated_at: now,
        archived: false,
        parent_thread_id: None,
        spawned_by: None,
        parked_until: None,
    }
}

impl HostData {
    pub fn send_receipt(&self) -> SendReceipt {
        SendReceipt {
            sender: self.delivery.sender().map(super::HostMessage::completion),
            attempt: SendAttempt(NEXT_ATTEMPT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)),
            bound: false,
        }
    }

    pub fn settle_send(&mut self, attempt: SendAttempt, settlement: SendSettlement) {
        let Some((thread, message)) = self.sends.attempted(attempt) else {
            tracing::debug!(
                host = %self.id,
                ?attempt,
                "a send settled after a newer attempt replaced it"
            );
            return;
        };
        match settlement {
            SendSettlement::Accepted => self.send_accepted(&thread, &message),
            SendSettlement::HandedBack(cause) => {
                self.send_failed(&thread, &message, attempt, &cause);
            }
        }
        self.release_creation_edits(&thread);
    }

    pub fn begin_send(&mut self, send: LocalSend) -> Vec<ThreadId> {
        let mut replaced = Vec::new();
        let listed = self.threads.contains_key(&send.thread)
            && !self.sends.creations.contains_key(&send.thread);
        if let Some(create) = send.create.clone().filter(|_| !listed) {
            let newest = self
                .threads
                .values()
                .map(|thread| thread.activity_seq)
                .max()
                .unwrap_or(0);
            let shown = self.sends.creations.remove(&send.thread).map_or_else(
                || provisional_thread(send.thread.clone(), create.clone(), newest + 1),
                |creation| Thread {
                    project_id: create.project.clone(),
                    config: create.config.clone(),
                    ..creation.thread().clone()
                },
            );
            let failed: Vec<ThreadId> = self
                .sends
                .creations
                .iter()
                .filter(|(_, creation)| matches!(creation, Creation::Failed { .. }))
                .map(|(id, _)| id.clone())
                .collect();
            for id in &failed {
                self.discard_creation(id);
            }
            replaced = failed;
            self.sends.creations.insert(
                send.thread.clone(),
                Creation::Sending {
                    thread: shown.clone(),
                    attempt: send.attempt,
                },
            );
            self.expected.expect(send.thread.clone());
            self.show_thread(shown);
        }
        let item = self.sends.local_item(&send);
        let messages = self.sends.messages.entry(send.thread.clone()).or_default();
        messages.retain(|message| message.item.id != send.message);
        messages.push(LocalMessage {
            item,
            attempt: send.attempt,
            posting: Posting::Sending,
        });
        self.pending_events.push(HostEvent::ThreadChanged {
            thread_id: send.thread,
        });
        replaced
    }

    fn send_accepted(&mut self, thread: &ThreadId, message: &ItemId) {
        if let Some(local) = self
            .sends
            .messages
            .get_mut(thread)
            .and_then(|messages| messages.iter_mut().find(|local| &local.item.id == message))
        {
            local.posting = Posting::Accepted;
        }
    }

    fn send_failed(
        &mut self,
        thread: &ThreadId,
        message: &ItemId,
        attempt: SendAttempt,
        cause: &HandBack,
    ) {
        self.sends.confirm_item(thread, message);
        let failed = matches!(
            self.sends.creations.get(thread),
            Some(Creation::Sending { attempt: creating, .. }) if *creating == attempt
        );
        if failed
            && let Some(Creation::Sending {
                thread: provisional,
                ..
            }) = self.sends.creations.remove(thread)
        {
            self.sends.creations.insert(
                thread.clone(),
                Creation::Failed {
                    thread: provisional.clone(),
                    cause: cause.clone(),
                },
            );
            self.drop_edits_because(thread, |host, thread, edit| {
                super::EditThreadError::CreationFailed {
                    host,
                    thread,
                    edit,
                    cause: cause.clone(),
                }
            });
            self.show_thread(provisional);
            if self.observed.borrow().as_ref() != Some(thread) {
                self.discard_creation(thread);
            }
        }
        self.pending_events.push(HostEvent::ThreadChanged {
            thread_id: thread.clone(),
        });
        self.pending_events.push(HostEvent::Changed);
    }

    pub fn retire_echoes<'a>(
        &mut self,
        thread: &ThreadId,
        loaded: impl IntoIterator<Item = &'a ItemId>,
    ) {
        let mut retired = false;
        for item in loaded {
            retired |= self.sends.confirm_item(thread, item);
        }
        if retired {
            self.pending_events.push(HostEvent::ThreadChanged {
                thread_id: thread.clone(),
            });
        }
    }

    pub fn local_messages(&self, thread: &ThreadId) -> impl Iterator<Item = &Arc<Item>> {
        self.sends
            .messages
            .get(thread)
            .into_iter()
            .flatten()
            .map(|local| &local.item)
    }

    #[must_use]
    pub fn is_provisional(&self, thread: &ThreadId) -> bool {
        self.sends.creations.contains_key(thread)
    }

    pub(super) fn creation_failed(&self, thread: &ThreadId) -> Option<&HandBack> {
        self.sends.failed(thread)
    }

    pub(super) fn leave_failed_creation(&mut self, left: &ThreadId) {
        if self.sends.failed(left).is_some() {
            self.discard_creation(left);
        }
    }

    fn discard_creation(&mut self, id: &ThreadId) {
        self.sends.forget(id);
        self.drop_provisional(id);
        self.pending_events.push(HostEvent::ThreadChanged {
            thread_id: id.clone(),
        });
        self.pending_events.push(HostEvent::Changed);
    }
}
