//! Bounded queue, withdrawal, restore, echo, and usage behavior for
//! [`ComposerQueueState`].
//!
//! Extracted verbatim from `composer_queue_state.rs` during the module split.

#[allow(clippy::wildcard_imports)]
use super::*;

/// Completed withdrawal keys are retained only long enough to make a delayed
/// duplicate receipt harmless. They contain no payload bytes.
const COMPLETED_WITHDRAWAL_HISTORY_LIMIT: usize = 8;

/// Recently echoed message ids kept across listing prunes. A taken-up row
/// that re-lists (dispatcher requeue after its echo projected) must not
/// re-enter the lip and duplicate its transcript echo. Small and finite.
const RETIRED_ECHO_HISTORY_LIMIT: usize = 32;

/// The edit/discard intent attached to one pending stable withdrawal.
enum QueueWithdrawalIntent {
    Edit { target: ComposerRecallTarget },
    Discard,
}

/// One stable withdrawal request awaiting an authoritative receipt.
struct PendingWithdrawal {
    identity: ComposerQueueIdentity,
    command: WithdrawQueuedMessageCommand,
    intent: QueueWithdrawalIntent,
}

/// One exact recall read awaiting its byte-bearing result.
struct PendingRecallRead {
    identity: ComposerQueueIdentity,
    query: ReadRecalledMessage,
    target: ComposerRecallTarget,
    dispatched: bool,
}

/// Immutable policy identity for one run-usage read.
#[derive(Clone, Debug, Eq, PartialEq)]
struct UsageScope {
    thread_id: ThreadId,
    run_id: RunId,
    generation: u64,
    model_id: EngineModelId,
    route_id: EngineRouteId,
    variant_id: Option<EngineVariantId>,
}

/// Last accepted provider report and its catalog label.
#[derive(Clone, Debug, Eq, PartialEq)]
struct UsageSnapshot {
    report: RunUsageReport,
    model_name: String,
}

/// Parent-controlled admission state for bounded queue polling.
#[derive(Default)]
struct QueueRefreshState {
    next_serial: u64,
    in_flight: Option<QueueRefreshToken>,
}

/// Complete queue/usage projection owned by the native application.
pub(crate) struct ComposerQueueState {
    current_thread: Option<ThreadId>,
    current_generation: u64,
    entries: Vec<ComposerQueueEntry>,
    total_count: u64,
    has_more: bool,
    order: QueuedMessageListOrder,
    refresh: QueueRefreshState,
    failed_entries: Vec<FailedQueueEntry>,
    failed_total_count: u64,
    failed_has_more: bool,
    failed_refresh: QueueRefreshState,
    pending_withdrawal: Option<PendingWithdrawal>,
    pending_recall_read: Option<PendingRecallRead>,
    restore_candidate: Option<RecallRestoreCandidate>,
    completed_withdrawals: VecDeque<CompletedWithdrawal>,
    terminal_identity: Option<ComposerQueueIdentity>,
    /// Forge message ids whose transcript echo was observed. Taken-up rows
    /// leave the lip even while still listed: the lip yields to the
    /// transcript exactly like the reference `TakeUp`, instead of blindly
    /// mapping every authoritative entry. Pruned against each authoritative
    /// listing (ids that cannot lip need no entry); see also
    /// `retired_echoes` for re-listed echoes.
    taken_up: HashSet<MessageId>,
    /// Finite recently-echoed message ids. Survives listing prunes so a
    /// requeued echo never re-enters the lip beside its transcript twin.
    retired_echoes: VecDeque<MessageId>,
    /// Accepted sends awaiting their echo, keyed by Forge message id (see
    /// [`EchoWatch`]). A bounded collection: the flight ends at receipt, so
    /// several sends can await echo at once.
    echo_watches: Vec<EchoWatch>,
    status: QueueStatus,
    usage_scope: Option<UsageScope>,
    usage: Option<UsageSnapshot>,
    usage_sequence: Option<u64>,
    usage_read_in_flight: Option<UsageReadToken>,
    next_usage_read_sequence: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct CompletedWithdrawal {
    request_id: RequestId,
    thread_id: ThreadId,
    message_id: MessageId,
    original_request_id: RequestId,
    outcome: QueuedMessageWithdrawalOutcome,
}

impl Default for ComposerQueueState {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Debug for ComposerQueueState {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ComposerQueueState")
            .field("current_thread", &self.current_thread)
            .field("current_generation", &self.current_generation)
            .field("entry_count", &self.entries.len())
            .field("total_count", &self.total_count)
            .field("has_more", &self.has_more)
            .field("refresh_in_flight", &self.refresh.in_flight.is_some())
            .field("failed_entry_count", &self.failed_entries.len())
            .field("failed_total_count", &self.failed_total_count)
            .field(
                "failed_refresh_in_flight",
                &self.failed_refresh.in_flight.is_some(),
            )
            .field("withdrawal_pending", &self.pending_withdrawal.is_some())
            .field("recall_read_pending", &self.pending_recall_read.is_some())
            .field("restore_candidate", &self.restore_candidate)
            .field("status", &self.status)
            .field("usage_scope", &self.usage_scope)
            .field("usage_present", &self.usage.is_some())
            .finish_non_exhaustive()
    }
}

impl ComposerQueueState {
    /// Creates an empty, non-polling projection.
    #[must_use]
    pub(crate) fn new() -> Self {
        Self {
            current_thread: None,
            current_generation: 0,
            entries: Vec::new(),
            total_count: 0,
            has_more: false,
            order: QueuedMessageListOrder::OldestFirst,
            refresh: QueueRefreshState::default(),
            failed_entries: Vec::new(),
            failed_total_count: 0,
            failed_has_more: false,
            failed_refresh: QueueRefreshState::default(),
            pending_withdrawal: None,
            pending_recall_read: None,
            restore_candidate: None,
            completed_withdrawals: VecDeque::with_capacity(COMPLETED_WITHDRAWAL_HISTORY_LIMIT),
            terminal_identity: None,
            taken_up: HashSet::new(),
            retired_echoes: VecDeque::new(),
            echo_watches: Vec::new(),
            status: QueueStatus::Idle,
            usage_scope: None,
            usage: None,
            usage_sequence: None,
            usage_read_in_flight: None,
            next_usage_read_sequence: 0,
        }
    }

    /// Replaces the mounted thread/generation and fences old queue pages.
    ///
    /// Pending withdrawal/read/candidate ownership is intentionally retained:
    /// a response can arrive after a thread switch, and a byte-bearing recall
    /// result must still be offered back to the caller rather than dropped.
    pub(crate) fn set_scope(&mut self, thread_id: Option<ThreadId>, generation: u64) {
        if self.current_thread == thread_id && self.current_generation == generation {
            return;
        }
        let thread_changed = self.current_thread != thread_id;
        self.current_thread = thread_id;
        self.current_generation = generation;
        self.entries.clear();
        self.total_count = 0;
        self.has_more = false;
        self.refresh.in_flight = None;
        self.failed_entries.clear();
        self.failed_total_count = 0;
        self.failed_has_more = false;
        self.failed_refresh.in_flight = None;
        self.terminal_identity = None;
        if thread_changed {
            // Take-up and echo watches are thread-scoped: a new thread owns
            // neither the old echoes nor their lip rows.
            self.taken_up.clear();
            self.retired_echoes.clear();
            self.echo_watches.clear();
        }
        if thread_changed
            || self
                .usage_scope
                .as_ref()
                .is_some_and(|scope| scope.generation != generation)
        {
            self.usage_scope = None;
            self.usage = None;
            self.usage_sequence = None;
            self.usage_read_in_flight = None;
            self.next_usage_read_sequence = 0;
        }
        self.status = if self.pending_withdrawal.is_some() {
            QueueStatus::WithdrawalPending
        } else if self.pending_recall_read.is_some() {
            QueueStatus::RecallReadPending
        } else if self.restore_candidate.is_some() {
            QueueStatus::RestoreBlocked
        } else {
            QueueStatus::Idle
        };
    }

    /// Returns the current thread, when a real thread is mounted.
    #[must_use]
    pub(crate) fn current_thread(&self) -> Option<&ThreadId> {
        self.current_thread.as_ref()
    }

    /// Returns the current app/composer generation.
    #[must_use]
    pub(crate) const fn current_generation(&self) -> u64 {
        self.current_generation
    }

    /// Starts one bounded refresh if the parent-visible conditions permit it.
    ///
    /// `force` is for relevant authoritative events such as a newly accepted
    /// queue receipt or a withdrawal receipt. Slow polling passes `false` and
    /// runs only while a queue row or active run exists.
    #[expect(
        clippy::fn_params_excessive_bools,
        reason = "the five parent-visible admission flags are independent conditions evaluated together; bundling them into a one-off struct would not clarify the refresh fence"
    )]
    pub(crate) fn begin_queue_refresh(
        &mut self,
        visible: bool,
        service_ready: bool,
        stopped: bool,
        run_active: bool,
        force: bool,
    ) -> Option<QueueRefreshToken> {
        let thread_id = self.current_thread.clone()?;
        if !visible || !service_ready || stopped || self.refresh.in_flight.is_some() {
            return None;
        }
        if !force && !run_active && self.total_count == 0 {
            return None;
        }
        let serial = self.refresh.next_serial.checked_add(1)?;
        self.refresh.next_serial = serial;
        let token = QueueRefreshToken {
            thread_id,
            generation: self.current_generation,
            serial,
        };
        self.refresh.in_flight = Some(token.clone());
        if matches!(self.status, QueueStatus::Idle | QueueStatus::Restored) {
            self.status = QueueStatus::Refreshing;
        }
        Some(token)
    }

    /// Returns whether one queue listing is currently outstanding.
    #[cfg(test)]
    #[must_use]
    pub(crate) fn queue_refresh_in_flight(&self) -> bool {
        self.refresh.in_flight.is_some()
    }

    /// Returns whether slow polling is still useful for the current scope.
    #[must_use]
    pub(crate) fn slow_poll_is_eligible(&self, run_active: bool) -> bool {
        self.current_thread.is_some()
            && self.refresh.in_flight.is_none()
            && (run_active || self.total_count > 0)
    }

    /// Installs one exact bounded queue page and clears its matching refresh.
    pub(crate) fn apply_queue_listing(
        &mut self,
        token: &QueueRefreshToken,
        listing: &QueuedMessageListing,
    ) -> Result<(), QueueListingRejection> {
        if self.refresh.in_flight.as_ref() != Some(token)
            || self.current_generation != token.generation
            || self.current_thread.as_ref() != Some(&token.thread_id)
        {
            return Err(QueueListingRejection::StaleRefresh);
        }
        self.refresh.in_flight = None;
        if listing.thread_id() != &token.thread_id {
            self.status = QueueStatus::TransportFailed;
            return Err(QueueListingRejection::WrongThread);
        }
        if listing.messages().len() > COMPOSER_QUEUE_PAGE_LIMIT {
            self.status = QueueStatus::TransportFailed;
            return Err(QueueListingRejection::TooManyRows);
        }
        self.order = listing.order();
        self.total_count = listing.total_count();
        self.has_more = listing.has_more();
        let mut seen_commands = HashSet::with_capacity(listing.messages().len());
        let mut entries = Vec::with_capacity(listing.messages().len());
        for summary in listing.messages() {
            let Some(entry) = ComposerQueueEntry::from_summary(summary, token.generation) else {
                self.status = QueueStatus::TransportFailed;
                return Err(QueueListingRejection::InvalidAttachments);
            };
            if !seen_commands.insert(entry.identity.command_id.clone()) {
                self.status = QueueStatus::TransportFailed;
                return Err(QueueListingRejection::DuplicateCommandId);
            }
            entries.push(entry);
        }
        self.entries = entries;
        if self.terminal_identity.as_ref().is_some_and(|identity| {
            !self
                .entries
                .iter()
                .any(|entry| entry.identity() == identity)
        }) {
            self.terminal_identity = None;
        }
        // Bound take-up growth against the authoritative listing: ids that
        // left the page cannot lip, so they need no set entry. Echoes that
        // re-list stay retired through the finite `retired_echoes` history
        // instead (a dispatcher requeue after the echo projected).
        self.taken_up.retain(|message_id| {
            self.entries
                .iter()
                .any(|entry| entry.message_id() == message_id)
        });
        self.status = if self.pending_withdrawal.is_some() {
            QueueStatus::WithdrawalPending
        } else if self.pending_recall_read.is_some() {
            QueueStatus::RecallReadPending
        } else if self.restore_candidate.is_some() {
            QueueStatus::RestoreBlocked
        } else if matches!(
            self.status,
            QueueStatus::TooLate | QueueStatus::NotQueued | QueueStatus::TransportFailed
        ) && self.terminal_identity.is_some()
        {
            self.status
        } else {
            QueueStatus::Idle
        };
        Ok(())
    }

    /// Clears a refresh only when the completion belongs to its exact token.
    pub(crate) fn finish_queue_refresh(&mut self, token: &QueueRefreshToken) -> bool {
        if self.refresh.in_flight.as_ref() != Some(token) {
            return false;
        }
        self.refresh.in_flight = None;
        if self.pending_withdrawal.is_none()
            && self.pending_recall_read.is_none()
            && self.restore_candidate.is_none()
            && self.status == QueueStatus::Refreshing
        {
            self.status = QueueStatus::Idle;
        }
        true
    }

    /// Starts one bounded failed-dispatch refresh if the parent-visible
    /// conditions permit it.
    ///
    /// `force` is for relevant authoritative events such as a newly observed
    /// dispatch failure. Slow polling passes `false` and runs only while a
    /// failed row exists. The failed read never disturbs the queued refresh
    /// fence: both pages are independent projections of one thread.
    #[expect(
        clippy::fn_params_excessive_bools,
        reason = "the four parent-visible admission flags are independent conditions evaluated together; bundling them into a one-off struct would not clarify the refresh fence"
    )]
    pub(crate) fn begin_failed_refresh(
        &mut self,
        visible: bool,
        service_ready: bool,
        stopped: bool,
        force: bool,
    ) -> Option<QueueRefreshToken> {
        let thread_id = self.current_thread.clone()?;
        if !visible || !service_ready || stopped || self.failed_refresh.in_flight.is_some() {
            return None;
        }
        if !force && self.failed_total_count == 0 {
            return None;
        }
        let serial = self.failed_refresh.next_serial.checked_add(1)?;
        self.failed_refresh.next_serial = serial;
        let token = QueueRefreshToken {
            thread_id,
            generation: self.current_generation,
            serial,
        };
        self.failed_refresh.in_flight = Some(token.clone());
        Some(token)
    }

    /// Returns whether one failed-dispatch listing is currently outstanding.
    #[cfg(test)]
    #[must_use]
    pub(crate) fn failed_refresh_in_flight(&self) -> bool {
        self.failed_refresh.in_flight.is_some()
    }

    /// Installs one exact bounded failed-dispatch page and clears its
    /// matching refresh.
    pub(crate) fn apply_failed_listing(
        &mut self,
        token: &QueueRefreshToken,
        listing: &FailedMessageListing,
    ) -> Result<(), FailedListingRejection> {
        if self.failed_refresh.in_flight.as_ref() != Some(token)
            || self.current_generation != token.generation
            || self.current_thread.as_ref() != Some(&token.thread_id)
        {
            return Err(FailedListingRejection::StaleRefresh);
        }
        self.failed_refresh.in_flight = None;
        if listing.thread_id() != &token.thread_id {
            return Err(FailedListingRejection::WrongThread);
        }
        if listing.messages().len() > COMPOSER_QUEUE_PAGE_LIMIT {
            return Err(FailedListingRejection::TooManyRows);
        }
        self.failed_total_count = listing.total_count();
        self.failed_has_more = listing.has_more();
        let mut seen_commands = HashSet::with_capacity(listing.messages().len());
        let mut entries = Vec::with_capacity(listing.messages().len());
        for summary in listing.messages() {
            let Some(entry) = FailedQueueEntry::from_summary(summary, token.generation) else {
                return Err(FailedListingRejection::InvalidAttachments);
            };
            if !seen_commands.insert(entry.identity.command_id.clone()) {
                return Err(FailedListingRejection::DuplicateCommandId);
            }
            entries.push(entry);
        }
        self.failed_entries = entries;
        Ok(())
    }

    /// Clears a failed refresh only when the completion belongs to its exact
    /// token.
    pub(crate) fn finish_failed_refresh(&mut self, token: &QueueRefreshToken) -> bool {
        if self.failed_refresh.in_flight.as_ref() != Some(token) {
            return false;
        }
        self.failed_refresh.in_flight = None;
        true
    }

    /// Returns the installed failed-dispatch rows, newest failures first.
    #[must_use]
    pub(crate) fn failed_entries(&self) -> &[FailedQueueEntry] {
        &self.failed_entries
    }

    /// Returns the exact failed-row count at read time.
    #[cfg(test)]
    #[must_use]
    pub(crate) const fn failed_total_count(&self) -> u64 {
        self.failed_total_count
    }

    /// Invalidates the current refresh without touching payload ownership.
    #[cfg(test)]
    pub(crate) fn cancel_queue_refresh(&mut self) {
        self.refresh.in_flight = None;
        if self.status == QueueStatus::Refreshing {
            self.status = QueueStatus::Idle;
        }
    }

    /// Returns the byte-free rows in the authoritative page order.
    #[cfg(test)]
    #[must_use]
    pub(crate) fn entries(&self) -> &[ComposerQueueEntry] {
        &self.entries
    }

    /// Returns the exact Forge count, including rows beyond this bounded page.
    #[cfg(test)]
    #[must_use]
    pub(crate) const fn total_count(&self) -> u64 {
        self.total_count
    }

    /// Returns whether the page omitted eligible rows.
    #[cfg(test)]
    #[must_use]
    pub(crate) const fn has_more(&self) -> bool {
        self.has_more
    }

    /// Projects rows into the existing controls contract.
    ///
    /// Taken-up and recently-echoed rows are omitted even while still
    /// listed: their transcript echo owns the visual now. Queued-behind rows
    /// (never echoed) stay.
    #[must_use]
    pub(crate) fn pending_lip_rows(&self) -> Vec<QueueLipRow> {
        self.entries
            .iter()
            .filter(|entry| {
                !self.taken_up.contains(entry.message_id())
                    && !self.retired_echoes.contains(entry.message_id())
            })
            .map(|entry| QueueLipRow {
                command_id: entry.identity.command_id.clone(),
                generation: entry.identity.generation,
                text: entry.lip_text().to_owned(),
                editable: self.row_is_editable(entry),
            })
            .collect()
    }

    /// Stages one accepted send to watch for its transcript echo.
    ///
    /// Keyed by Forge message id: a replayed receipt for the same message
    /// replaces its entry instead of duplicating it. Evicts the oldest
    /// watch past [`ECHO_WATCH_LIMIT`].
    pub(crate) fn stage_echo_watch(
        &mut self,
        message_id: MessageId,
        steer_run_id: Option<RunId>,
        engine_label: Option<String>,
    ) {
        self.echo_watches
            .retain(|watch| watch.message_id() != &message_id);
        if self.echo_watches.len() >= ECHO_WATCH_LIMIT {
            self.echo_watches.remove(0);
        }
        self.echo_watches.push(EchoWatch {
            message_id,
            steer_run_id,
            engine_label,
        });
    }

    /// Returns the staged echo watch for one Forge message id, if any.
    #[must_use]
    pub(crate) fn echo_watch_for(&self, message_id: &MessageId) -> Option<EchoWatch> {
        self.echo_watches
            .iter()
            .find(|watch| watch.message_id() == message_id)
            .cloned()
    }

    /// Returns how many echo watches are staged.
    #[must_use]
    pub(crate) fn echo_watch_count(&self) -> usize {
        self.echo_watches.len()
    }

    /// Drops one staged echo watch without retiring anything.
    ///
    /// Returns whether a watch was present.
    pub(crate) fn clear_echo_watch_for(&mut self, message_id: &MessageId) -> bool {
        let before = self.echo_watches.len();
        self.echo_watches
            .retain(|watch| watch.message_id() != message_id);
        self.echo_watches.len() != before
    }

    /// Marks one Forge message taken up and records its echo in the finite
    /// retired history.
    ///
    /// Returns whether the row was newly retired from the lip.
    pub(crate) fn mark_taken_up(&mut self, message_id: &MessageId) -> bool {
        let fresh =
            !self.taken_up.contains(message_id) && !self.retired_echoes.contains(message_id);
        self.taken_up.insert(message_id.clone());
        self.retired_echoes.retain(|retired| retired != message_id);
        self.retired_echoes.push_back(message_id.clone());
        while self.retired_echoes.len() > RETIRED_ECHO_HISTORY_LIMIT {
            self.retired_echoes.pop_front();
        }
        fresh
    }

    fn row_is_editable(&self, entry: &ComposerQueueEntry) -> bool {
        self.pending_withdrawal.is_none()
            && self.pending_recall_read.is_none()
            && self.restore_candidate.is_none()
            && self.current_thread.as_ref() == Some(&entry.thread_id)
            && self.current_generation == entry.identity.generation
            && self.terminal_identity.as_ref() != Some(entry.identity())
    }

    /// Finds the exact byte-free row for a controls event.
    #[must_use]
    pub(crate) fn entry_for_identity(
        &self,
        identity: &ComposerQueueIdentity,
    ) -> Option<&ComposerQueueEntry> {
        self.entries
            .iter()
            .find(|entry| entry.identity() == identity)
    }

    /// Starts an edit withdrawal after the parent captured the exact empty
    /// composer target.
    pub(crate) fn begin_edit(
        &mut self,
        identity: &ComposerQueueIdentity,
        request_id: RequestId,
        target: ComposerRecallTarget,
    ) -> Result<WithdrawQueuedMessageCommand, QueueIntentError> {
        self.begin_withdrawal(identity, request_id, QueueWithdrawalIntent::Edit { target })
    }

    /// Starts a discard withdrawal without ever reading image bytes.
    pub(crate) fn begin_discard(
        &mut self,
        identity: &ComposerQueueIdentity,
        request_id: RequestId,
    ) -> Result<WithdrawQueuedMessageCommand, QueueIntentError> {
        self.begin_withdrawal(identity, request_id, QueueWithdrawalIntent::Discard)
    }

    fn begin_withdrawal(
        &mut self,
        identity: &ComposerQueueIdentity,
        request_id: RequestId,
        intent: QueueWithdrawalIntent,
    ) -> Result<WithdrawQueuedMessageCommand, QueueIntentError> {
        let Some(current_thread) = self.current_thread.as_ref() else {
            return Err(QueueIntentError::NoThread);
        };
        if self.pending_withdrawal.is_some()
            || self.pending_recall_read.is_some()
            || self.restore_candidate.is_some()
        {
            return Err(QueueIntentError::Busy);
        }
        let Some(entry) = self.entry_for_identity(identity) else {
            return Err(QueueIntentError::UnknownRow);
        };
        if current_thread != &entry.thread_id
            || self.current_generation != entry.identity.generation
            || !self.row_is_editable(entry)
        {
            return Err(QueueIntentError::StaleRow);
        }
        let command = WithdrawQueuedMessageCommand::new(
            request_id,
            entry.thread_id.clone(),
            entry.message_id.clone(),
            entry.original_request_id.clone(),
        );
        self.pending_withdrawal = Some(PendingWithdrawal {
            identity: identity.clone(),
            command: command.clone(),
            intent,
        });
        self.status = QueueStatus::WithdrawalPending;
        Ok(command)
    }

    /// Returns the exact pending withdrawal command for an explicit transport
    /// retry. Retrying this value preserves the original request/frame id.
    #[must_use]
    pub(crate) fn pending_withdrawal(&self) -> Option<&WithdrawQueuedMessageCommand> {
        self.pending_withdrawal
            .as_ref()
            .map(|pending| &pending.command)
    }

    /// Marks the pending withdrawal as sent again after a retry decision.
    pub(crate) fn retry_pending_withdrawal(&mut self) -> Option<WithdrawQueuedMessageCommand> {
        let command = self.pending_withdrawal.as_ref()?.command.clone();
        self.status = QueueStatus::WithdrawalPending;
        Some(command)
    }

    /// Leaves the exact withdrawal command pending after a service failure.
    pub(crate) fn mark_withdrawal_failed(&mut self) -> bool {
        if self.pending_withdrawal.is_none() {
            return false;
        }
        self.status = QueueStatus::TransportFailed;
        true
    }

    /// Consumes only a receipt that matches request id and all target ids.
    ///
    /// `TooLate` and `NotQueued` are surfaced as statuses and never trigger a
    /// read. A delayed duplicate of a completed exact receipt is harmless and
    /// does not issue a second command.
    #[expect(
        clippy::result_large_err,
        reason = "the rejection error intentionally carries the unconsumed authoritative withdrawal receipt back to the caller"
    )]
    pub(crate) fn accept_withdrawal_result(
        &mut self,
        result: QueuedMessageWithdrawalResult,
    ) -> Result<WithdrawalReceiptDisposition, WithdrawalReceiptError> {
        let matches_pending = self.pending_withdrawal.as_ref().is_some_and(|pending| {
            result.withdrawal_request_id() == pending.command.request_id()
                && result.thread_id == pending.command.thread_id
                && result.message_id == pending.command.message_id
                && result.original_request_id == pending.command.original_request_id
        });
        if !matches_pending {
            let duplicate = self.completed_withdrawals.iter().any(|completed| {
                result.withdrawal_request_id() == &completed.request_id
                    && result.thread_id == completed.thread_id
                    && result.message_id == completed.message_id
                    && result.original_request_id == completed.original_request_id
                    && result.outcome == completed.outcome
            });
            if duplicate {
                return Ok(WithdrawalReceiptDisposition::DuplicateHandled);
            }
            let reason = if self.pending_withdrawal.is_some() {
                WithdrawalReceiptRejection::TargetMismatch
            } else {
                WithdrawalReceiptRejection::NoPendingOperation
            };
            return Err(WithdrawalReceiptError { result, reason });
        }

        let Some(pending) = self.pending_withdrawal.take() else {
            return Err(WithdrawalReceiptError {
                result,
                reason: WithdrawalReceiptRejection::NoPendingOperation,
            });
        };
        self.remember_completed_withdrawal(&pending.command, &result);
        match result.outcome {
            QueuedMessageWithdrawalOutcome::Withdrawn => match pending.intent {
                QueueWithdrawalIntent::Edit { target } => {
                    let query = ReadRecalledMessage::new(
                        pending.command.thread_id.clone(),
                        pending.command.message_id.clone(),
                        pending.command.original_request_id.clone(),
                    );
                    self.pending_recall_read = Some(PendingRecallRead {
                        identity: pending.identity,
                        query: query.clone(),
                        target,
                        dispatched: false,
                    });
                    self.status = QueueStatus::RecallReadPending;
                    Ok(WithdrawalReceiptDisposition::EditNeedsRead(query))
                }
                QueueWithdrawalIntent::Discard => {
                    self.terminal_identity = Some(pending.identity);
                    self.status = QueueStatus::WithdrawnAwaitingRefresh;
                    Ok(WithdrawalReceiptDisposition::Discarded)
                }
            },
            QueuedMessageWithdrawalOutcome::TooLate => {
                self.terminal_identity = Some(pending.identity);
                self.status = QueueStatus::TooLate;
                Ok(WithdrawalReceiptDisposition::TooLate)
            }
            QueuedMessageWithdrawalOutcome::NotQueued => {
                self.terminal_identity = Some(pending.identity);
                self.status = QueueStatus::NotQueued;
                Ok(WithdrawalReceiptDisposition::NotQueued)
            }
        }
    }

    fn remember_completed_withdrawal(
        &mut self,
        command: &WithdrawQueuedMessageCommand,
        result: &QueuedMessageWithdrawalResult,
    ) {
        if self.completed_withdrawals.len() >= COMPLETED_WITHDRAWAL_HISTORY_LIMIT {
            self.completed_withdrawals.pop_front();
        }
        self.completed_withdrawals.push_back(CompletedWithdrawal {
            request_id: command.request_id.clone(),
            thread_id: command.thread_id.clone(),
            message_id: command.message_id.clone(),
            original_request_id: command.original_request_id.clone(),
            outcome: result.outcome,
        });
    }

    /// Returns the next exact recalled-message query once.
    pub(crate) fn next_recalled_message_read(&mut self) -> Option<ReadRecalledMessage> {
        let pending = self.pending_recall_read.as_mut()?;
        if pending.dispatched {
            return None;
        }
        pending.dispatched = true;
        Some(pending.query.clone())
    }

    #[cfg(test)]
    pub(crate) fn can_retry_recalled_read(&self) -> bool {
        self.pending_recall_read
            .as_ref()
            .is_some_and(|read| !read.dispatched)
    }

    /// Reopens one exact recalled read for a deliberate transport retry.
    pub(crate) fn mark_recalled_read_failed(&mut self, query: &ReadRecalledMessage) -> bool {
        let Some(pending) = self.pending_recall_read.as_mut() else {
            return false;
        };
        if &pending.query != query {
            return false;
        }
        pending.dispatched = false;
        self.status = QueueStatus::RecallReadPending;
        true
    }

    /// Accepts a byte-bearing response only for the exact pending read.
    ///
    /// The pending read is allowed to complete after a thread/generation
    /// change so the payload can be offered back to the caller. The target's
    /// own GPUI precondition then decides whether it can be restored now.
    #[expect(
        clippy::result_large_err,
        reason = "the rejection error intentionally carries the unconsumed byte-bearing recall result back to the caller"
    )]
    pub(crate) fn accept_recalled_message(
        &mut self,
        result: RecalledMessageResult,
    ) -> Result<RecalledMessageDisposition, RecalledMessageError> {
        let Some(pending) = self.pending_recall_read.as_ref() else {
            return Err(RecalledMessageError {
                result,
                reason: RecalledMessageRejection::NoPendingRead,
            });
        };
        if result.thread_id != pending.query.thread_id
            || result.message_id != pending.query.message_id
            || result.original_request_id != pending.query.original_request_id
        {
            return Err(RecalledMessageError {
                result,
                reason: RecalledMessageRejection::ScopeMismatch,
            });
        }
        if self.restore_candidate.is_some() {
            return Err(RecalledMessageError {
                result,
                reason: RecalledMessageRejection::RestoreAlreadyHeld,
            });
        }
        let Some(pending) = self.pending_recall_read.take() else {
            return Err(RecalledMessageError {
                result,
                reason: RecalledMessageRejection::NoPendingRead,
            });
        };
        let Some(payload) = result.payload else {
            self.terminal_identity = Some(pending.identity);
            self.status = QueueStatus::PayloadUnavailable;
            return Ok(RecalledMessageDisposition::NoPayload);
        };
        self.restore_candidate = Some(RecallRestoreCandidate::new(
            pending.identity,
            pending.query.thread_id.clone(),
            pending.query.message_id.clone(),
            pending.query.original_request_id.clone(),
            pending.target,
            payload,
        ));
        self.status = QueueStatus::RestoreReady;
        Ok(RecalledMessageDisposition::PayloadReady)
    }

    /// Returns the owned candidate for one immediate or explicit retry.
    pub(crate) fn take_restore_candidate(&mut self) -> Option<RecallRestoreCandidate> {
        self.restore_candidate.take()
    }

    /// Rebinds the held payload to a newly captured target without copying
    /// image bytes. The caller must have made this an explicit user retry.
    pub(crate) fn take_restore_candidate_with_target(
        &mut self,
        target: ComposerRecallTarget,
    ) -> Option<RecallRestoreCandidate> {
        self.restore_candidate
            .take()
            .map(|candidate| candidate.with_target(target))
    }

    /// Retains the unchanged payload after the composer rejected a restore.
    ///
    /// If another candidate appeared concurrently, the returned candidate is
    /// still owned by the caller; this method never overwrites or drops it.
    #[expect(
        clippy::result_large_err,
        reason = "returning the rejected candidate by value is the API's purpose: the caller must be able to retry without losing the payload"
    )]
    pub(crate) fn retain_restore_candidate(
        &mut self,
        candidate: RecallRestoreCandidate,
    ) -> Result<(), RecallRestoreCandidate> {
        if self.restore_candidate.is_some() {
            return Err(candidate);
        }
        self.restore_candidate = Some(candidate);
        self.status = QueueStatus::RestoreBlocked;
        Ok(())
    }

    /// Marks a candidate accepted by the composer and requests a fresh queue
    /// projection. No payload remains owned after the caller moved it into the
    /// composer.
    pub(crate) fn mark_restore_succeeded(&mut self, identity: &ComposerQueueIdentity) -> bool {
        if self.restore_candidate.is_some() {
            return false;
        }
        self.terminal_identity = Some(identity.clone());
        self.status = QueueStatus::Restored;
        true
    }

    /// Returns the current truthful queue status.
    #[cfg(test)]
    #[must_use]
    pub(crate) const fn status(&self) -> QueueStatus {
        self.status
    }

    /// Returns whether an explicit retry can offer a candidate again.
    #[must_use]
    pub(crate) fn can_retry_restore(&self) -> bool {
        self.restore_candidate.is_some()
    }

    /// Records a transport failure while retaining the exact pending command.
    pub(crate) fn mark_transport_failure(&mut self) {
        if self.pending_withdrawal.is_some() || self.pending_recall_read.is_some() {
            self.status = QueueStatus::TransportFailed;
        }
    }

    /// Drops all transient read fences for a terminal service failure.
    ///
    /// Queue, failed-listing, and usage reads outstanding against a dead
    /// service will never resolve; clearing their fences returns the lip
    /// from a stuck Refreshing to a truthful `TransportFailed`. Entries,
    /// restore candidates, payloads, and drafts are untouched: only the
    /// in-flight read ownership dies with the service.
    pub(crate) fn mark_service_failed(&mut self) {
        self.refresh.in_flight = None;
        self.failed_refresh.in_flight = None;
        self.usage_read_in_flight = None;
        self.status = QueueStatus::TransportFailed;
    }

    /// Starts or retains an immutable reporting scope for one exact run.
    ///
    /// A matching scope keeps its last report after settlement. A new run,
    /// policy, thread, or generation clears the previous report before the
    /// next read is admitted.
    pub(crate) fn begin_usage_scope(
        &mut self,
        thread_id: ThreadId,
        generation: u64,
        run_id: RunId,
        model_id: EngineModelId,
        route_id: EngineRouteId,
        variant_id: Option<EngineVariantId>,
    ) -> bool {
        if self.current_thread.as_ref() != Some(&thread_id) || self.current_generation != generation
        {
            return false;
        }
        let next = UsageScope {
            thread_id,
            run_id,
            generation,
            model_id,
            route_id,
            variant_id,
        };
        if self.usage_scope.as_ref() != Some(&next) {
            self.usage_scope = Some(next);
            self.usage = None;
            self.usage_sequence = None;
            self.usage_read_in_flight = None;
            self.next_usage_read_sequence = 0;
        }
        true
    }

    /// Returns the immutable run currently used for usage reads.
    #[must_use]
    pub(crate) fn usage_scope(&self) -> Option<(&ThreadId, &RunId, u64)> {
        self.usage_scope
            .as_ref()
            .map(|scope| (&scope.thread_id, &scope.run_id, scope.generation))
    }

    /// Starts one exact usage read without allowing overlap.
    pub(crate) fn begin_usage_read(
        &mut self,
    ) -> Option<(UsageReadToken, artisan_domain::ReadRunUsage)> {
        let scope = self.usage_scope.as_ref()?;
        if self.usage_read_in_flight.is_some() {
            return None;
        }
        let sequence = self.next_usage_read_sequence.checked_add(1)?;
        self.next_usage_read_sequence = sequence;
        let token = UsageReadToken {
            thread_id: scope.thread_id.clone(),
            run_id: scope.run_id.clone(),
            generation: scope.generation,
            sequence,
        };
        self.usage_read_in_flight = Some(token.clone());
        Some((
            token,
            artisan_domain::ReadRunUsage::new(scope.thread_id.clone(), scope.run_id.clone()),
        ))
    }

    /// Reopens usage admission after a failed exact read.
    pub(crate) fn mark_usage_read_failed(&mut self, token: &UsageReadToken) -> bool {
        if self.usage_read_in_flight.as_ref() != Some(token) {
            return false;
        }
        self.usage_read_in_flight = None;
        true
    }

    /// Accepts a report only when every app and launch-policy fence agrees.
    ///
    /// `model_name` is a label lookup for the reporting model only. It never
    /// supplies a token denominator or rewrites the report's model identity.
    #[expect(
        clippy::result_large_err,
        reason = "the rejection error intentionally carries the unconsumed usage result back to the caller"
    )]
    pub(crate) fn accept_usage_result(
        &mut self,
        result: RunUsageResult,
        token: &UsageReadToken,
        model_name: String,
    ) -> Result<UsageResultDisposition, UsageResultError> {
        let Some(scope) = self.usage_scope.as_ref() else {
            return Err(UsageResultError {
                result,
                reason: UsageRejection::NoScope,
            });
        };
        if scope.generation != token.generation {
            return Err(UsageResultError {
                result,
                reason: UsageRejection::StaleGeneration,
            });
        }
        if self.usage_read_in_flight.as_ref() != Some(token)
            || scope.thread_id != token.thread_id
            || scope.run_id != token.run_id
        {
            return Err(UsageResultError {
                result,
                reason: UsageRejection::NoPendingRead,
            });
        }
        self.usage_read_in_flight = None;
        if result.thread_id != scope.thread_id {
            return Err(UsageResultError {
                result,
                reason: UsageRejection::WrongThread,
            });
        }
        if result.run_id != scope.run_id {
            return Err(UsageResultError {
                result,
                reason: UsageRejection::WrongRun,
            });
        }
        let Some(report) = result.report else {
            return Ok(UsageResultDisposition::NoReport);
        };
        if report.thread_id() != &scope.thread_id
            || report.run_id() != &scope.run_id
            || report.model_id() != &scope.model_id
            || report.provider_route_id() != &scope.route_id
            || report.variant_id() != scope.variant_id.as_ref()
        {
            return Err(UsageResultError {
                result: RunUsageResult {
                    thread_id: scope.thread_id.clone(),
                    run_id: scope.run_id.clone(),
                    report: Some(report),
                },
                reason: UsageRejection::PolicyMismatch,
            });
        }
        if self
            .usage_sequence
            .is_some_and(|sequence| report.source_sequence() <= sequence)
        {
            return Err(UsageResultError {
                result: RunUsageResult {
                    thread_id: scope.thread_id.clone(),
                    run_id: scope.run_id.clone(),
                    report: Some(report),
                },
                reason: UsageRejection::StaleSequence,
            });
        }
        self.usage_sequence = Some(report.source_sequence());
        self.usage = Some(UsageSnapshot { report, model_name });
        Ok(UsageResultDisposition::Updated)
    }

    /// Returns the latest reporting-run usage for the requested run.
    ///
    /// The UI adapter maps `engine_id` to the existing
    /// `NativeContextUsage.reporting_engine_id` field. No selected-model
    /// catalog capacity is consulted.
    #[must_use]
    pub(crate) fn reporting_usage_for(
        &self,
        current_run_id: Option<&str>,
    ) -> Option<ReportingUsage> {
        let scope = self.usage_scope.as_ref()?;
        let usage = self.usage.as_ref()?;
        if current_run_id != Some(scope.run_id.as_str()) {
            return None;
        }
        let report = &usage.report;
        Some(ReportingUsage {
            run_id: report.run_id().as_str().to_owned(),
            engine_id: report.provider_route_id().as_str().to_owned(),
            model_id: report.model_id().as_str().to_owned(),
            model_name: usage.model_name.clone(),
            context_tokens: report.context_tokens(),
            context_window_tokens: report.context_window_tokens(),
            input_tokens: report.input_tokens(),
            cached_input_tokens: report.cached_input_tokens(),
            output_tokens: report.output_tokens(),
        })
    }
}
