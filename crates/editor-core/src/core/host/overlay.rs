use super::{HostCompletion, HostData, HostEvent, HostMessage};
use crate::core::{client::CallError, hosts::HostId};
use artisan_api::{Permissions, Seq, Thread, ThreadId, UpdateThread};
use futures::future::BoxFuture;
use std::collections::{BTreeMap, HashMap, VecDeque};
use tokio_util::sync::CancellationToken;
use tokio_util::task::AbortOnDropHandle;

const CONFIRMED: usize = 128;
const READING: usize = 128;
const READS_AT_ONCE: usize = 4;
const READ_ATTEMPTS: u32 = 5;
const READ_RETRY: std::time::Duration = std::time::Duration::from_secs(1);

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ThreadEdit {
    Rename(String),
    Archive,
    Unarchive,
    Settle { observed_seq: Seq },
    MarkUnread,
    Permissions(Permissions),
}

impl std::fmt::Display for ThreadEdit {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Rename(_) => f.write_str("rename"),
            Self::Archive => f.write_str("archive"),
            Self::Unarchive => f.write_str("restore from the archive"),
            Self::Settle { observed_seq } => write!(f, "settle through seq {observed_seq}"),
            Self::MarkUnread => f.write_str("mark unread"),
            Self::Permissions(Permissions::Auto) => f.write_str("set permissions to auto"),
            Self::Permissions(Permissions::Full) => f.write_str("set permissions to full access"),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum EditThreadError {
    #[error("{edit} for thread {thread} on {host}: this Editor doesn't have the thread")]
    Unknown {
        host: HostId,
        thread: ThreadId,
        edit: ThreadEdit,
    },
    #[error("{edit} for thread {thread} on {host}: creating the thread failed")]
    CreationFailed {
        host: HostId,
        thread: ThreadId,
        edit: ThreadEdit,
        #[source]
        cause: super::sending::HandBack,
    },
    #[error("{edit} for thread {thread} on {host}: the thread was removed")]
    Removed {
        host: HostId,
        thread: ThreadId,
        edit: ThreadEdit,
    },
    #[error("{edit} for thread {thread} on {host}")]
    Refused {
        host: HostId,
        thread: ThreadId,
        edit: ThreadEdit,
        #[source]
        source: CallError,
    },
}

impl EditThreadError {
    #[must_use]
    pub const fn edit(&self) -> &ThreadEdit {
        match self {
            Self::Unknown { edit, .. }
            | Self::CreationFailed { edit, .. }
            | Self::Removed { edit, .. }
            | Self::Refused { edit, .. } => edit,
        }
    }

    #[must_use]
    pub fn problem(&self) -> crate::core::Problem {
        let headline = match self.edit() {
            ThreadEdit::Rename(_) => "Couldn't rename this thread.",
            ThreadEdit::Archive => "Couldn't archive this thread.",
            ThreadEdit::Unarchive => "Couldn't restore the thread.",
            ThreadEdit::Settle { .. } => "Couldn't mark this thread as read.",
            ThreadEdit::MarkUnread => "Couldn't mark this thread as unread.",
            ThreadEdit::Permissions(_) => "Couldn't change this thread's permissions.",
        };
        let reason = match self {
            Self::Unknown { .. } => "This thread isn't on this Forge.",
            Self::Removed { .. } => "The thread was deleted.",
            Self::CreationFailed { .. } => {
                "The thread couldn't be created. Send the message again."
            }
            Self::Refused { source, .. } => source.fault().sentence(),
        };
        crate::core::Problem::caused_by(format!("{headline} {reason}"), self)
    }
}

#[derive(Clone, Debug)]
pub struct PostGate {
    written: tokio::sync::watch::Receiver<BTreeMap<ThreadId, u64>>,
    thread: ThreadId,
    through: u64,
}

impl PostGate {
    pub async fn opened(mut self) {
        let (thread, through) = (self.thread, self.through);
        if let Err(closed) = self
            .written
            .wait_for(|written| written.get(&thread).is_none_or(|done| *done >= through))
            .await
        {
            tracing::debug!(
                %thread,
                error = %closed,
                "the host went away while a message waited for its permission change"
            );
        }
    }
}

pub type EditReport = Box<dyn FnOnce(Result<(), EditThreadError>) -> BoxFuture<'static, ()> + Send>;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Field {
    Title,
    Archived,
    Settled,
    Reading,
    Permissions,
}

enum Pending {
    Edit {
        edit: ThreadEdit,
        report: EditReport,
        after: Vec<super::sending::SendAttempt>,
    },
    Read(Seq),
}

impl ThreadEdit {
    const fn field(&self) -> Field {
        match self {
            Self::Rename(_) => Field::Title,
            Self::Archive | Self::Unarchive => Field::Archived,
            Self::Settle { .. } => Field::Settled,
            Self::MarkUnread => Field::Reading,
            Self::Permissions(_) => Field::Permissions,
        }
    }

    fn apply(&self, thread: &mut Thread) {
        match self {
            Self::Rename(title) => thread.title = Some(title.clone()),
            Self::Archive => {
                thread.archived = true;
                thread.parked_until = None;
            }
            Self::Unarchive => thread.archived = false,
            Self::Settle { observed_seq } => {
                thread.settled_seq = thread
                    .settled_seq
                    .max((*observed_seq).min(thread.activity_seq));
            }
            Self::MarkUnread => thread.marked_unread = true,
            Self::Permissions(permissions) => thread.config.permissions = *permissions,
        }
    }

    fn request(&self, base: &Thread) -> EditRequest {
        let update = |update: UpdateThread| EditRequest::Update(Box::new(update));
        match self {
            Self::Rename(title) => update(UpdateThread {
                title: Some(title.clone()),
                ..UpdateThread::default()
            }),
            Self::Archive => update(UpdateThread {
                archived: Some(true),
                ..UpdateThread::default()
            }),
            Self::Unarchive => update(UpdateThread {
                archived: Some(false),
                ..UpdateThread::default()
            }),
            Self::Settle { observed_seq } => EditRequest::Settle(*observed_seq),
            Self::MarkUnread => EditRequest::MarkUnread,
            Self::Permissions(permissions) => update(UpdateThread {
                config: Some(artisan_api::EngineConfig {
                    permissions: *permissions,
                    ..base.config.clone()
                }),
                ..UpdateThread::default()
            }),
        }
    }
}

impl Pending {
    const fn field(&self) -> Field {
        match self {
            Self::Edit { edit, .. } => edit.field(),
            Self::Read(_) => Field::Reading,
        }
    }

    fn apply(&self, thread: &mut Thread) {
        match self {
            Self::Edit { edit, .. } => edit.apply(thread),
            Self::Read(seq) => {
                let seq = (*seq).min(thread.activity_seq);
                thread.read_seq = thread.read_seq.max(seq);
                if seq >= thread.activity_seq {
                    thread.marked_unread = false;
                }
            }
        }
    }

    fn request(&self, base: &Thread) -> EditRequest {
        match self {
            Self::Edit { edit, .. } => edit.request(base),
            Self::Read(seq) => EditRequest::Read(*seq),
        }
    }

    const fn writes_permissions(&self) -> bool {
        matches!(
            self,
            Self::Edit {
                edit: ThreadEdit::Permissions(_),
                ..
            }
        )
    }

    const fn is_read(&self) -> bool {
        matches!(self, Self::Read(_))
    }
}

enum EditRequest {
    Update(Box<UpdateThread>),
    Settle(Seq),
    MarkUnread,
    Read(Seq),
}

impl EditRequest {
    async fn send(
        self,
        client: &crate::core::ForgeClient,
        host: &HostId,
        id: &ThreadId,
        superseded: CancellationToken,
    ) -> Result<Thread, CallError> {
        match self {
            Self::Update(update) => client.update_thread(id, &update).await,
            Self::Settle(observed) => client.settle_thread(id, observed).await,
            Self::MarkUnread => client.mark_unread(id).await,
            Self::Read(seq) => {
                let update = UpdateThread {
                    read_seq: Some(seq),
                    ..UpdateThread::default()
                };
                let asked = std::time::Instant::now();
                let mut attempt = 1;
                loop {
                    match client.update_thread(id, &update).await {
                        Ok(saved) => {
                            crate::core::diagnostics::open_timing::rpc_done(
                                std::time::Duration::ZERO,
                                asked.elapsed(),
                            );
                            return Ok(saved);
                        }
                        Err(error)
                            if attempt < READ_ATTEMPTS
                                && super::read_ack_failure(&error).is_transient() =>
                        {
                            tracing::debug!(
                                %host,
                                thread = %id,
                                seq,
                                attempt,
                                error = %::diagnostics::chain(&error),
                                "read acknowledgement will retry"
                            );
                            tokio::select! {
                                () = tokio::time::sleep(READ_RETRY) => attempt += 1,
                                () = superseded.cancelled() => return Err(error),
                            }
                        }
                        Err(error) => return Err(error),
                    }
                }
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct EditTicket(u64);

struct Sending {
    ticket: EditTicket,
    pending: Pending,
    superseded: CancellationToken,
    _request: AbortOnDropHandle<()>,
}

struct Lane {
    sending: Option<Sending>,
    waiting: VecDeque<Pending>,
    left_window: Option<super::pages::WindowMark>,
}

impl Lane {
    fn pending(&self) -> impl Iterator<Item = &Pending> {
        self.sending
            .iter()
            .map(|sending| &sending.pending)
            .chain(&self.waiting)
    }

    fn reading(&self) -> bool {
        self.sending
            .as_ref()
            .is_some_and(|sending| sending.pending.is_read())
    }
}

struct Patched {
    base: Thread,
    lanes: BTreeMap<Field, Lane>,
}

impl Patched {
    fn shown(&self) -> Thread {
        let mut thread = self.base.clone();
        for pending in self.lanes.values().flat_map(Lane::pending) {
            pending.apply(&mut thread);
        }
        thread
    }
}

#[derive(Default)]
pub(super) struct ThreadOverlay {
    patched: HashMap<ThreadId, Patched>,
    confirmed: VecDeque<Thread>,
    parked_reads: VecDeque<ThreadId>,
    tickets: u64,
}

impl ThreadOverlay {
    pub(super) fn shown(&mut self, forge: Thread) -> Thread {
        match self.patched.get_mut(&forge.id) {
            Some(patched) => {
                patched.base = forge;
                patched.shown()
            }
            None => forge,
        }
    }

    pub(super) fn stale(&self, forge: &Thread) -> bool {
        self.confirmed
            .iter()
            .any(|confirmed| confirmed.id == forge.id && confirmed.seq > forge.seq)
    }

    pub(super) fn caught_up(&mut self, forge: &Thread) {
        self.confirmed
            .retain(|confirmed| confirmed.id != forge.id || confirmed.seq > forge.seq);
    }

    pub(super) fn freshest(&self, forge: Thread) -> Thread {
        self.confirmed
            .iter()
            .find(|confirmed| confirmed.id == forge.id && confirmed.seq > forge.seq)
            .cloned()
            .unwrap_or(forge)
    }

    fn confirm(&mut self, saved: Thread) {
        if self
            .confirmed
            .iter()
            .any(|confirmed| confirmed.id == saved.id && confirmed.seq >= saved.seq)
        {
            return;
        }
        self.confirmed.retain(|confirmed| confirmed.id != saved.id);
        self.confirmed.push_back(saved);
        while self.confirmed.len() > CONFIRMED {
            self.confirmed.pop_front();
        }
    }

    pub(super) fn unlisted(&self, listed: impl Fn(&ThreadId) -> bool) -> Vec<ThreadId> {
        self.patched
            .keys()
            .filter(|id| !listed(id))
            .cloned()
            .collect()
    }

    fn forget(&mut self, id: &ThreadId) -> Vec<(ThreadEdit, EditReport)> {
        self.confirmed.retain(|confirmed| &confirmed.id != id);
        self.parked_reads.retain(|parked| parked != id);
        let Some(patched) = self.patched.remove(id) else {
            return Vec::new();
        };
        patched
            .lanes
            .into_values()
            .flat_map(|lane| {
                lane.sending
                    .map(|sending| sending.pending)
                    .into_iter()
                    .chain(lane.waiting)
            })
            .filter_map(|pending| match pending {
                Pending::Edit { edit, report, .. } => Some((edit, report)),
                Pending::Read(_) => None,
            })
            .collect()
    }

    pub(super) fn kept(&self) -> impl Iterator<Item = &ThreadId> {
        self.patched.keys()
    }

    pub(super) fn editing(&self) -> bool {
        !self.patched.is_empty()
    }

    fn reading(&self) -> usize {
        self.patched
            .values()
            .filter(|patched| patched.lanes.contains_key(&Field::Reading))
            .count()
    }

    fn reads_in_flight(&self) -> usize {
        self.patched
            .values()
            .filter(|patched| {
                patched
                    .lanes
                    .get(&Field::Reading)
                    .is_some_and(Lane::reading)
            })
            .count()
    }

    fn sending_field(&self, id: &ThreadId, ticket: EditTicket) -> Option<Field> {
        self.patched
            .get(id)?
            .lanes
            .iter()
            .find_map(|(field, lane)| {
                lane.sending
                    .as_ref()
                    .filter(|sending| sending.ticket == ticket)
                    .map(|_| *field)
            })
    }

    fn base(&self, id: &ThreadId) -> Option<&Thread> {
        self.patched.get(id).map(|patched| &patched.base)
    }

    fn next_ticket(&mut self) -> EditTicket {
        self.tickets = self.tickets.wrapping_add(1);
        EditTicket(self.tickets)
    }
}

fn deliver(report: EditReport, outcome: Result<(), EditThreadError>) {
    crate::core::runtime::runtime().spawn(report(outcome));
}

impl HostData {
    pub fn edit_thread(&mut self, id: &ThreadId, edit: ThreadEdit, report: EditReport) {
        let refusal = if !self.threads.contains_key(id) {
            Some(EditThreadError::Unknown {
                host: self.id.clone(),
                thread: id.clone(),
                edit: edit.clone(),
            })
        } else if let Some(cause) = self.creation_failed(id) {
            Some(EditThreadError::CreationFailed {
                host: self.id.clone(),
                thread: id.clone(),
                edit: edit.clone(),
                cause: cause.clone(),
            })
        } else {
            None
        };
        let after = match edit {
            ThreadEdit::Permissions(_) => self.sends.posting_attempts(id),
            ThreadEdit::Rename(_)
            | ThreadEdit::Archive
            | ThreadEdit::Unarchive
            | ThreadEdit::Settle { .. }
            | ThreadEdit::MarkUnread => Vec::new(),
        };
        match refusal {
            Some(error) => deliver(report, Err(error)),
            None => self.enqueue(
                id,
                Pending::Edit {
                    edit,
                    report,
                    after,
                },
            ),
        }
    }

    #[must_use]
    pub fn confirmed_thread(&self, id: &ThreadId) -> Option<&Thread> {
        self.thread_overlay
            .base(id)
            .or_else(|| self.threads.get(id))
    }

    #[must_use]
    pub fn permissions_wait_for_send(&self, id: &ThreadId) -> bool {
        self.thread_overlay
            .patched
            .get(id)
            .and_then(|patched| patched.lanes.get(&Field::Permissions))
            .is_some_and(|lane| {
                lane.sending.is_none()
                    && lane.waiting.front().is_some_and(|next| match next {
                        Pending::Edit { after, .. } => {
                            after.iter().any(|attempt| self.sends.posting(id, *attempt))
                        }
                        Pending::Read(_) => false,
                    })
            })
    }

    #[must_use]
    pub fn permissions_changing(&self, id: &ThreadId) -> bool {
        self.thread_overlay
            .patched
            .get(id)
            .is_some_and(|patched| patched.lanes.contains_key(&Field::Permissions))
    }

    pub(super) fn reads_pending(&self, id: &ThreadId) -> bool {
        self.thread_overlay
            .patched
            .get(id)
            .is_some_and(|patched| patched.lanes.contains_key(&Field::Reading))
    }

    pub(super) fn read_lanes_full(&self) -> bool {
        self.thread_overlay.reading() >= READING
    }

    pub(super) fn acknowledge(&mut self, id: &ThreadId, seq: Seq) {
        self.enqueue(id, Pending::Read(seq));
    }

    pub(super) fn drop_edits(&mut self, id: &ThreadId) {
        self.drop_edits_because(id, |host, thread, edit| EditThreadError::Removed {
            host,
            thread,
            edit,
        });
    }

    pub(super) fn drop_edits_because(
        &mut self,
        id: &ThreadId,
        refusal: impl Fn(HostId, ThreadId, ThreadEdit) -> EditThreadError,
    ) {
        for (edit, report) in self.thread_overlay.forget(id) {
            deliver(report, Err(refusal(self.id.clone(), id.clone(), edit)));
        }
        self.permission_written(id, true);
        self.unpark_reads();
    }

    pub(super) fn release_creation_edits(&mut self, id: &ThreadId) {
        let parked: Vec<Field> = self
            .thread_overlay
            .patched
            .get(id)
            .into_iter()
            .flat_map(|patched| patched.lanes.iter())
            .filter(|(_, lane)| lane.sending.is_none())
            .map(|(field, _)| *field)
            .collect();
        for field in parked {
            self.advance(id, field);
        }
    }

    #[must_use]
    pub fn post_gate(&self, thread: &ThreadId) -> Option<PostGate> {
        let through = *self.permission_queue.get(thread)?.back()?;
        Some(PostGate {
            written: self.permission_written.subscribe(),
            thread: thread.clone(),
            through,
        })
    }

    fn permission_written(&mut self, id: &ThreadId, all: bool) {
        let Some(queue) = self.permission_queue.get_mut(id) else {
            return;
        };
        let done = if all { None } else { queue.pop_front() };
        if let Some(done) = done.filter(|_| !queue.is_empty()) {
            self.permission_written.send_modify(|written| {
                written.insert(id.clone(), done);
            });
        } else {
            self.permission_queue.remove(id);
            self.permission_written.send_modify(|written| {
                written.remove(id);
            });
        }
    }

    fn enqueue(&mut self, id: &ThreadId, pending: Pending) {
        let current = self
            .threads
            .get(id)
            .cloned()
            .expect("invariant: thread edits are queued only for threads this host shows");
        let field = pending.field();
        if pending.writes_permissions() {
            let before = self.permission_serial;
            if !self.permission_queue.contains_key(id) {
                self.permission_written.send_modify(|written| {
                    written.insert(id.clone(), before);
                });
            }
            self.permission_serial = before.wrapping_add(1);
            self.permission_queue
                .entry(id.clone())
                .or_default()
                .push_back(self.permission_serial);
        }
        let left_window = (field == Field::Archived)
            .then(|| self.window_mark(id, usize::from(current.archived)))
            .flatten();
        let patched = self
            .thread_overlay
            .patched
            .entry(id.clone())
            .or_insert_with(|| Patched {
                base: current,
                lanes: BTreeMap::new(),
            });
        let idle = if let Some(lane) = patched.lanes.get_mut(&field) {
            match (pending, lane.waiting.back_mut()) {
                (Pending::Read(seq), Some(Pending::Read(queued))) => {
                    *queued = (*queued).max(seq);
                    false
                }
                (
                    unread @ Pending::Edit {
                        edit: ThreadEdit::MarkUnread,
                        ..
                    },
                    _,
                ) => {
                    lane.waiting.retain(|waiting| !waiting.is_read());
                    if let Some(sending) = lane
                        .sending
                        .as_ref()
                        .filter(|sending| sending.pending.is_read())
                    {
                        sending.superseded.cancel();
                    }
                    lane.waiting.push_back(unread);
                    lane.sending.is_none()
                }
                (pending, _) => {
                    lane.waiting.push_back(pending);
                    false
                }
            }
        } else {
            patched.lanes.insert(
                field,
                Lane {
                    sending: None,
                    waiting: VecDeque::from([pending]),
                    left_window,
                },
            );
            true
        };
        if idle {
            self.advance(id, field);
        }
        let shown = self.thread_overlay.patched[id].shown();
        self.show_thread(shown);
    }

    fn advance(&mut self, id: &ThreadId, field: Field) {
        let reads_full = self.thread_overlay.reads_in_flight() >= READS_AT_ONCE;
        let patched = self
            .thread_overlay
            .patched
            .get_mut(id)
            .expect("invariant: a lane advances only inside its thread's patch");
        let mut lane = patched
            .lanes
            .remove(&field)
            .expect("invariant: a lane advances only while it exists");
        assert!(
            lane.sending.is_none(),
            "invariant: lane {field:?} of thread {id} advances only once its request finished"
        );
        let after_send = lane.waiting.front().is_some_and(|next| match next {
            Pending::Edit { after, .. } => {
                after.iter().any(|attempt| self.sends.posting(id, *attempt))
            }
            Pending::Read(_) => false,
        });
        if self.sends.creating(id) || after_send {
            patched.lanes.insert(field, lane);
            return;
        }
        let next = lane.waiting.pop_front().unwrap_or_else(|| {
            panic!(
                "invariant: lane {field:?} of thread {id} advances only while it has work waiting"
            )
        });
        if next.is_read() && reads_full {
            lane.waiting.push_front(next);
            patched.lanes.insert(field, lane);
            if !self.thread_overlay.parked_reads.contains(id) {
                self.thread_overlay.parked_reads.push_back(id.clone());
            }
            return;
        }
        let base = patched.base.clone();
        lane.sending = Some(self.send_edit(id, next, &base));
        self.thread_overlay
            .patched
            .get_mut(id)
            .expect("invariant: the patch outlives its own lane")
            .lanes
            .insert(field, lane);
    }

    fn unpark_reads(&mut self) {
        while self.thread_overlay.reads_in_flight() < READS_AT_ONCE {
            let Some(id) = self.thread_overlay.parked_reads.pop_front() else {
                return;
            };
            let parked = self
                .thread_overlay
                .patched
                .get(&id)
                .and_then(|patched| patched.lanes.get(&Field::Reading))
                .is_some_and(|lane| lane.sending.is_none());
            if parked {
                self.advance(&id, Field::Reading);
            }
        }
    }

    fn send_edit(&mut self, id: &ThreadId, pending: Pending, base: &Thread) -> Sending {
        let ticket = self.thread_overlay.next_ticket();
        let request = pending.request(base);
        let client = self.client.clone();
        let sender = self.delivery.sender().map(HostMessage::completion);
        let host = self.id.clone();
        let thread = id.clone();
        let superseded = CancellationToken::new();
        let stop = superseded.clone();
        let task = crate::core::runtime::runtime().spawn(async move {
            let result = request.send(&client, &host, &thread, stop).await;
            sender
                .offer(HostCompletion::ThreadEdit(thread, ticket, result))
                .await;
        });
        Sending {
            ticket,
            pending,
            superseded,
            _request: AbortOnDropHandle::new(task),
        }
    }

    pub(super) fn finish_thread_edit(
        &mut self,
        id: &ThreadId,
        ticket: EditTicket,
        result: Result<Thread, CallError>,
    ) {
        let Some(field) = self.thread_overlay.sending_field(id, ticket) else {
            tracing::debug!(
                host = %self.id,
                thread = %id,
                "a thread edit answered after its thread's edits were dropped"
            );
            self.unpark_reads();
            return;
        };
        let patched = self
            .thread_overlay
            .patched
            .get_mut(id)
            .expect("invariant: the lane was just found in this patch");
        let lane = patched
            .lanes
            .get_mut(&field)
            .expect("invariant: the lane was just found");
        let sending = lane
            .sending
            .take()
            .expect("invariant: the lane found by its ticket is sending");
        let superseded = sending.superseded.is_cancelled();
        let left_window = lane.left_window;
        let lane_done = lane.waiting.is_empty();
        if let Ok(saved) = &result
            && saved.seq >= patched.base.seq
        {
            patched.base = saved.clone();
        }
        if lane_done {
            patched.lanes.remove(&field);
        }
        if let Ok(saved) = &result {
            self.thread_overlay.confirm(saved.clone());
        }
        let missing = result.as_ref().err().is_some_and(|error| {
            matches!(
                error.refused(),
                Some(artisan_api::Failure::Thread(artisan_api::ThreadFailure::NotFound { thread }))
                    if thread == id
            )
        });
        if missing {
            self.forget_thread(id);
        } else {
            if !lane_done {
                self.advance(id, field);
            }
            let rollback = left_window.filter(|_| result.is_err() && lane_done);
            self.show_settled(id, rollback);
        }
        self.pending_events.push(HostEvent::ThreadChanged {
            thread_id: id.clone(),
        });
        self.pending_events.push(HostEvent::Changed);
        if sending.pending.writes_permissions() {
            self.permission_written(id, false);
        }
        if sending.pending.is_read() {
            self.unpark_reads();
        }
        match (sending.pending, result) {
            (Pending::Edit { edit, report, .. }, result) => deliver(
                report,
                result.map(drop).map_err(|source| EditThreadError::Refused {
                    host: self.id.clone(),
                    thread: id.clone(),
                    edit,
                    source,
                }),
            ),
            (Pending::Read(_), Ok(_)) => {}
            (Pending::Read(seq), Err(error)) if superseded => tracing::debug!(
                host = %self.id,
                thread = %id,
                seq,
                error = %::diagnostics::chain(&error),
                "a failing read acknowledgement gave way to marking the thread unread"
            ),
            (Pending::Read(seq), Err(error)) => {
                tracing::warn!(
                    host = %self.id,
                    thread = %id,
                    seq,
                    error = %::diagnostics::chain(&error),
                    "Forge refused a read acknowledgement; the thread shows unread again"
                );
            }
        }
    }

    fn show_settled(&mut self, id: &ThreadId, rollback: Option<super::pages::WindowMark>) {
        let patched = self
            .thread_overlay
            .patched
            .get(id)
            .expect("invariant: a settled edit's patch is dropped only here");
        let shown = patched.shown();
        if patched.lanes.is_empty() {
            self.thread_overlay.patched.remove(id);
        }
        if !self.threads.contains_key(id) {
            return;
        }
        let old = self.store_thread(shown);
        if let Some(window) = rollback {
            self.rejoin_page(id, &window);
        }
        self.place_thread(id, old.as_ref());
    }

    pub(super) fn show_thread(&mut self, shown: Thread) {
        let id = shown.id.clone();
        let old = self.store_thread(shown);
        self.place_thread(&id, old.as_ref());
        self.pending_events
            .push(HostEvent::ThreadChanged { thread_id: id });
        self.pending_events.push(HostEvent::Changed);
    }
}
