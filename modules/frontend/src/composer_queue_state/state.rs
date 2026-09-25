//! Forge outbox, withdrawal, and usage behavior for [`ComposerQueueState`].
//!
//! The queued and failed rows are the thread's Forge message outbox, pushed
//! over the conversation subscription and installed whole. Nothing here
//! polls, matches transcript echoes, or holds a message payload: the Forge
//! owns every accepted message and its delivery state.

#[allow(clippy::wildcard_imports)]
use super::*;

/// Completed withdrawal keys are retained only long enough to make a delayed
/// duplicate receipt harmless. They contain no payload bytes.
const COMPLETED_WITHDRAWAL_HISTORY_LIMIT: usize = 8;

/// The edit/discard intent attached to one pending stable withdrawal.
#[derive(Clone, Copy)]
enum QueueWithdrawalIntent {
    Edit,
    Discard,
}

/// One stable withdrawal request awaiting an authoritative receipt.
struct PendingWithdrawal {
    identity: ComposerQueueIdentity,
    command: WithdrawQueuedMessageCommand,
    intent: QueueWithdrawalIntent,
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

/// Complete outbox/usage projection owned by the native application.
pub(crate) struct ComposerQueueState {
    current_thread: Option<ThreadId>,
    current_generation: u64,
    /// Whether the mounted scope has received its first Forge outbox.
    outbox_received: bool,
    entries: Vec<ComposerQueueEntry>,
    total_count: u64,
    has_more: bool,
    failed_entries: Vec<FailedQueueEntry>,
    pending_withdrawal: Option<PendingWithdrawal>,
    completed_withdrawals: VecDeque<CompletedWithdrawal>,
    terminal_identity: Option<ComposerQueueIdentity>,
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
            .field("outbox_received", &self.outbox_received)
            .field("entry_count", &self.entries.len())
            .field("total_count", &self.total_count)
            .field("has_more", &self.has_more)
            .field("failed_entry_count", &self.failed_entries.len())
            .field("withdrawal_pending", &self.pending_withdrawal.is_some())
            .field("status", &self.status)
            .field("usage_scope", &self.usage_scope)
            .field("usage_present", &self.usage.is_some())
            .finish_non_exhaustive()
    }
}

impl ComposerQueueState {
    /// Creates an empty projection with no mounted scope.
    #[must_use]
    pub(crate) fn new() -> Self {
        Self {
            current_thread: None,
            current_generation: 0,
            outbox_received: false,
            entries: Vec::new(),
            total_count: 0,
            has_more: false,
            failed_entries: Vec::new(),
            pending_withdrawal: None,
            completed_withdrawals: VecDeque::with_capacity(COMPLETED_WITHDRAWAL_HISTORY_LIMIT),
            terminal_identity: None,
            status: QueueStatus::Idle,
            usage_scope: None,
            usage: None,
            usage_sequence: None,
            usage_read_in_flight: None,
            next_usage_read_sequence: 0,
        }
    }

    /// Replaces the mounted thread/generation and drops the old outbox.
    ///
    /// A pending withdrawal is intentionally retained: its receipt can
    /// arrive after a thread switch and must still settle.
    pub(crate) fn set_scope(&mut self, thread_id: Option<ThreadId>, generation: u64) {
        if self.current_thread == thread_id && self.current_generation == generation {
            return;
        }
        let thread_changed = self.current_thread != thread_id;
        self.current_thread = thread_id;
        self.current_generation = generation;
        self.outbox_received = false;
        self.entries.clear();
        self.total_count = 0;
        self.has_more = false;
        self.failed_entries.clear();
        self.terminal_identity = None;
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
        } else {
            QueueStatus::Idle
        };
    }

    /// Returns the current thread, when a real thread is mounted.
    #[must_use]
    pub(crate) fn current_thread(&self) -> Option<&ThreadId> {
        self.current_thread.as_ref()
    }

    /// Installs the mounted thread's complete Forge outbox.
    ///
    /// Returns the queued messages that arrived since the previous outbox of
    /// this scope, oldest first; the first outbox after a scope change
    /// reports none (those rows were already there). A rejected outbox
    /// leaves the previous one in place.
    ///
    /// # Errors
    ///
    /// Returns [`OutboxRejection`] when the outbox names another thread or
    /// violates the row bounds.
    pub(crate) fn apply_outbox(
        &mut self,
        outbox: &MessageOutbox,
    ) -> Result<Vec<MessageId>, OutboxRejection> {
        if self.current_thread.as_ref() != Some(outbox.thread_id()) {
            return Err(OutboxRejection::WrongThread);
        }
        let queued = outbox.queued();
        let failed = outbox.failed();
        if queued.messages().len() > COMPOSER_QUEUE_PAGE_LIMIT
            || failed.messages().len() > COMPOSER_QUEUE_PAGE_LIMIT
        {
            return Err(OutboxRejection::TooManyRows);
        }
        let generation = self.current_generation;
        let entries = unique_rows(queued.messages(), |summary| {
            ComposerQueueEntry::from_summary(summary, generation)
        })?;
        let failed_entries = unique_rows(failed.messages(), |summary| {
            FailedQueueEntry::from_summary(summary, generation)
        })?;
        let arrivals = if self.outbox_received {
            entries
                .iter()
                .filter(|entry| {
                    !self
                        .entries
                        .iter()
                        .any(|known| known.message_id == entry.message_id)
                })
                .map(|entry| entry.message_id.clone())
                .collect()
        } else {
            Vec::new()
        };
        self.outbox_received = true;
        self.entries = entries;
        self.total_count = queued.total_count();
        self.has_more = queued.has_more();
        self.failed_entries = failed_entries;
        if self.terminal_identity.as_ref().is_some_and(|identity| {
            !self
                .entries
                .iter()
                .any(|entry| entry.identity() == identity)
        }) {
            self.terminal_identity = None;
        }
        if matches!(self.status, QueueStatus::Withdrawn) && self.terminal_identity.is_none() {
            self.status = QueueStatus::Idle;
        }
        Ok(arrivals)
    }

    /// Returns the Forge rows of messages not yet in the transcript, in
    /// acceptance order.
    #[must_use]
    pub(crate) fn entries(&self) -> &[ComposerQueueEntry] {
        &self.entries
    }

    /// Returns the Forge failure rows still offered, newest first.
    #[must_use]
    pub(crate) fn failed_entries(&self) -> &[FailedQueueEntry] {
        &self.failed_entries
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

    /// Projects queued rows into the existing controls contract.
    #[must_use]
    pub(crate) fn pending_lip_rows(&self) -> Vec<QueueLipRow> {
        self.entries
            .iter()
            .map(|entry| QueueLipRow {
                command_id: entry.identity.command_id.clone(),
                generation: entry.identity.generation,
                text: entry.lip_text().to_owned(),
                editable: self.row_is_editable(entry),
            })
            .collect()
    }

    fn row_is_editable(&self, entry: &ComposerQueueEntry) -> bool {
        self.pending_withdrawal.is_none()
            && entry.state() == QueuedMessageState::Queued
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

    /// Finds the exact failure row for a controls event.
    #[must_use]
    pub(crate) fn failed_entry_for_identity(
        &self,
        identity: &ComposerQueueIdentity,
    ) -> Option<&FailedQueueEntry> {
        self.failed_entries
            .iter()
            .find(|entry| entry.identity() == identity)
    }

    /// Starts an edit withdrawal once the parent confirmed an empty, locked
    /// composer. The Forge moves the withdrawn payload into the thread's
    /// composer draft.
    pub(crate) fn begin_edit(
        &mut self,
        identity: &ComposerQueueIdentity,
        request_id: RequestId,
    ) -> Result<WithdrawQueuedMessageCommand, QueueIntentError> {
        self.begin_withdrawal(identity, request_id, QueueWithdrawalIntent::Edit)
    }

    /// Starts a discard withdrawal.
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
        if self.pending_withdrawal.is_some() {
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
        let command = match intent {
            QueueWithdrawalIntent::Edit => command.recalling_to_draft(),
            QueueWithdrawalIntent::Discard => command,
        };
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

    /// Whether an edit withdrawal is waiting for its receipt; the composer
    /// stays locked until the Forge draft holds the recalled prompt.
    #[must_use]
    pub(crate) fn edit_pending(&self) -> bool {
        self.pending_withdrawal
            .as_ref()
            .is_some_and(|pending| matches!(pending.intent, QueueWithdrawalIntent::Edit))
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
    /// `TooLate` and `NotQueued` are surfaced as statuses. A delayed
    /// duplicate of a completed exact receipt is harmless and does not issue
    /// a second command.
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
        self.terminal_identity = Some(pending.identity);
        match result.outcome {
            QueuedMessageWithdrawalOutcome::Withdrawn => match pending.intent {
                QueueWithdrawalIntent::Edit => {
                    self.status = QueueStatus::RecalledToDraft;
                    Ok(WithdrawalReceiptDisposition::Recalled)
                }
                QueueWithdrawalIntent::Discard => {
                    self.status = QueueStatus::Withdrawn;
                    Ok(WithdrawalReceiptDisposition::Discarded)
                }
            },
            QueuedMessageWithdrawalOutcome::TooLate => {
                self.status = QueueStatus::TooLate;
                Ok(WithdrawalReceiptDisposition::TooLate)
            }
            QueuedMessageWithdrawalOutcome::NotQueued => {
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

    /// Returns the current truthful queue status.
    #[cfg(test)]
    #[must_use]
    pub(crate) const fn status(&self) -> QueueStatus {
        self.status
    }

    /// Drops the in-flight usage read for a terminal service failure. The
    /// outbox stays as last pushed; a new connection pushes it again.
    pub(crate) fn mark_service_failed(&mut self) {
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

/// A row that resolves one controls event.
trait IdentifiedRow {
    fn row_identity(&self) -> &ComposerQueueIdentity;
}

impl IdentifiedRow for ComposerQueueEntry {
    fn row_identity(&self) -> &ComposerQueueIdentity {
        self.identity()
    }
}

impl IdentifiedRow for FailedQueueEntry {
    fn row_identity(&self) -> &ComposerQueueIdentity {
        self.identity()
    }
}

/// Converts one listing's rows, refusing out-of-bounds attachments and a
/// reused original request id: a controls event must resolve to exactly one
/// row.
fn unique_rows<S, T: IdentifiedRow>(
    rows: &[S],
    convert: impl Fn(&S) -> Option<T>,
) -> Result<Vec<T>, OutboxRejection> {
    let mut seen = HashSet::with_capacity(rows.len());
    let mut converted = Vec::with_capacity(rows.len());
    for row in rows {
        let row = convert(row).ok_or(OutboxRejection::InvalidAttachments)?;
        if !seen.insert(row.row_identity().command_id.clone()) {
            return Err(OutboxRejection::DuplicateCommandId);
        }
        converted.push(row);
    }
    Ok(converted)
}
