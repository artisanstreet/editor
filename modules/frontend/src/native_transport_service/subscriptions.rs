//! Conversation subscription custody and stream handling for the native
//! transport service: subscribe/unsubscribe/acknowledge arms, reconnect with
//! subscription restore, and delivery-loss recovery.
//!
//! The parent `native_transport_service` remains the owner of the command and
//! event vocabulary, session runtime fields, and domain request handlers.
//! Root mounts this file as a child module so subscribe/stream handling stays
//! in one reviewable home.

#![forbid(unsafe_code)]
#![allow(clippy::module_name_repetitions)]

use super::*;

/// Minimal custody for the active subscription; the `ConversationHost` is the projection authority.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SubscriptionCustody {
    active_thread: Option<ThreadId>,
    pending_after: Option<ConversationCursor>,
    last_accepted_cursor: Option<ConversationCursor>,
    /// Transport-forwarded continuity: the end cursor of the latest validated
    /// batch. This advances on publish, independent of the application
    /// acknowledgement above, so a second contiguous batch validates while
    /// the UI ack is still queued behind other commands. It never marks
    /// application acceptance and never seeds a resubscribe baseline.
    received_cursor: Option<ConversationCursor>,
}

impl SubscriptionCustody {
    /// Creates empty custody.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Records a new subscribe, tombstoning the old thread without restarting the receiver.
    pub fn on_subscribe(&mut self, thread_id: ThreadId, after: Option<ConversationCursor>) {
        if self.active_thread.as_ref() != Some(&thread_id) {
            self.active_thread = Some(thread_id);
            self.last_accepted_cursor = None;
        }
        self.pending_after = after;
        self.received_cursor = None;
    }

    /// Derives and records a Fresh or Resumed server cursor without advancing
    /// application custody.
    ///
    /// # Errors
    ///
    /// Returns an integrity failure when the response does not match the
    /// active thread, pending request, requested mode/cursor, or accepted
    /// cursor floor.
    pub fn on_started(
        &mut self,
        expected_after: Option<ConversationCursor>,
        started: &ConversationSubscriptionStarted,
    ) -> Result<(), ServiceFailure> {
        let (thread_id, cursor) = match started {
            ConversationSubscriptionStarted::Fresh(start) => {
                (start.snapshot().thread_id(), start.snapshot().cursor())
            }
            ConversationSubscriptionStarted::Resumed { thread_id, cursor } => (thread_id, *cursor),
        };
        let matches_request = match (expected_after, started) {
            (None, ConversationSubscriptionStarted::Fresh(_)) => true,
            (Some(expected), ConversationSubscriptionStarted::Resumed { cursor, .. }) => {
                *cursor == expected
            }
            _ => false,
        };
        if !matches_request
            || self.pending_after != expected_after
            || self.active_thread.as_ref() != Some(thread_id)
            || self
                .last_accepted_cursor
                .is_some_and(|last| cursor.get() < last.get())
        {
            return Err(ServiceFailure::new(
                ServiceFailureStage::Request,
                ServiceFailureCategory::Integrity,
            ));
        }
        self.pending_after = Some(cursor);
        Ok(())
    }

    /// Advances cursor only after explicit application acknowledgement.
    ///
    /// # Errors
    ///
    /// Returns an integrity failure for a stale thread or a cursor behind the
    /// accepted or pending cursor.
    pub fn on_acknowledge(
        &mut self,
        thread_id: &ThreadId,
        cursor: ConversationCursor,
    ) -> Result<(), ServiceFailure> {
        if self.active_thread.as_ref() != Some(thread_id) {
            return Err(ServiceFailure::new(
                ServiceFailureStage::Request,
                ServiceFailureCategory::Integrity,
            ));
        }
        if self
            .last_accepted_cursor
            .is_some_and(|last| cursor.get() < last.get())
            || self
                .pending_after
                .is_some_and(|after| cursor.get() < after.get())
        {
            return Err(ServiceFailure::new(
                ServiceFailureStage::Request,
                ServiceFailureCategory::Integrity,
            ));
        }
        self.last_accepted_cursor = Some(cursor);
        self.pending_after = None;
        Ok(())
    }

    /// Clears custody on unsubscribe.
    pub fn on_unsubscribe(&mut self, thread_id: &ThreadId) {
        if self.active_thread.as_ref() == Some(thread_id) {
            self.active_thread = None;
            self.pending_after = None;
            self.last_accepted_cursor = None;
            self.received_cursor = None;
        }
    }

    /// Records one validated batch as transport-forwarded without marking
    /// application acceptance.
    ///
    /// The next contiguous batch validates against this cursor even when the
    /// UI acknowledgement is still queued behind other commands. Resubscribe
    /// baselines keep reading the application-accepted cursor, never this
    /// one; a fresh subscribe epoch resets it.
    pub fn on_batch_forwarded(&mut self, to_cursor: ConversationCursor) {
        self.received_cursor = Some(to_cursor);
    }

    /// Returns the cursor the next batch must continue from, if any baseline
    /// exists.
    ///
    /// Forwarded transport continuity wins while an epoch is live; the
    /// subscribe baseline covers the first batch and the accepted cursor
    /// covers a live epoch with nothing forwarded yet. `None` means no
    /// baseline exists and the batch must fail closed.
    #[must_use]
    pub fn expected_batch_from(&self) -> Option<ConversationCursor> {
        self.received_cursor
            .or(self.pending_after)
            .or(self.last_accepted_cursor)
    }

    /// Returns the latest transport-forwarded cursor, if any.
    #[must_use]
    pub fn received_cursor(&self) -> Option<ConversationCursor> {
        self.received_cursor
    }

    /// Returns the active thread.
    #[must_use]
    pub fn active_thread(&self) -> Option<&ThreadId> {
        self.active_thread.as_ref()
    }

    /// Returns the last application-accepted cursor.
    #[must_use]
    pub fn last_accepted_cursor(&self) -> Option<ConversationCursor> {
        self.last_accepted_cursor
    }

    /// Returns the pending after cursor for the in-flight subscribe.
    #[must_use]
    pub fn pending_after(&self) -> Option<ConversationCursor> {
        self.pending_after
    }
}

impl ServiceRuntime {
    pub(super) async fn reconnect(
        &mut self,
        frames: &mut FrameFactory,
        restore_subscription: bool,
    ) -> Result<(), ServiceFailure> {
        // Custody order: cancel delivery task, await its join exactly once,
        // drop the old session, check out the fenced capability, connect,
        // publish the rotated capability, take_delivery exactly once, and
        // start one new delivery task. Request-driven reconnects additionally
        // restore the active subscription before their request continues;
        // delivery-loss recovery leaves that one subscribe to the application.
        if let Some(cancel) = self.delivery_cancel.take() {
            cancel.cancel();
        }
        if let Some(join) = self.delivery_join.take() {
            let _ = join.await;
        }
        drop(self.session.take());
        let lease = self
            .reconnect_lease
            .take()
            .ok_or(ServiceFailure::local_session())?;
        let mut attempt = lease
            .begin_reconnect()
            .map_err(|_| reconnect_custody_failure())?;
        let capability = attempt
            .take_credential()
            .map_err(|_| reconnect_custody_failure())?;
        let hello = match reconnect_hello_with_capability(frames, capability) {
            Ok(hello) => hello,
            Err((failure, capability)) => {
                if let Ok(lease) = attempt.restore_before_handshake(capability) {
                    self.reconnect_lease = Some(lease);
                }
                return Err(failure);
            }
        };
        let connected = ClientSession::connect(
            self.target,
            self.certificate.clone(),
            self.pinned_identity,
            hello,
            self.limits,
            &self.cancel,
        )
        .await;
        let Ok((session, welcome)) = connected else {
            let _ = attempt.quarantine();
            return Err(ServiceFailure::new(
                ServiceFailureStage::Handshake,
                ServiceFailureCategory::Authentication,
            ));
        };
        let reconnect_lease = attempt
            .publish_next(self.reconnect_binding, welcome.welcome.reconnect_capability)
            .map_err(|_| reconnect_custody_failure())?;
        // take_delivery exactly once
        let Ok((session, receiver)) = session.take_delivery() else {
            let _ = reconnect_lease.quarantine();
            return Err(ServiceFailure::local_session());
        };
        let Some(tx) = self.delivery_tx.clone() else {
            drop(session);
            let _ = reconnect_lease.quarantine();
            return Err(ServiceFailure::local_session());
        };
        let version = session.protocol_version();
        self.session = Some(session);
        self.reconnect_lease = Some(reconnect_lease);
        let cancel = Arc::new(CancelHandle::new());
        let join = tokio::spawn(delivery_task_loop(
            receiver,
            tx,
            Arc::clone(&cancel),
            version,
        ));
        self.delivery_cancel = Some(cancel);
        self.delivery_join = Some(join);
        if restore_subscription && let Some(thread_id) = self.custody.active_thread().cloned() {
            let after = self.custody.last_accepted_cursor();
            if let Err(failure) = self
                .resubscribe_after_reconnect(frames, thread_id, after)
                .await
            {
                self.quarantine_reconnect_lease();
                return Err(failure);
            }
        }
        Ok(())
    }

    pub(super) fn quarantine_reconnect_lease(&mut self) {
        if let Some(lease) = self.reconnect_lease.take() {
            let _ = lease.quarantine();
        }
    }

    pub(super) async fn resubscribe_after_reconnect(
        &mut self,
        frames: &mut FrameFactory,
        thread_id: ThreadId,
        after: Option<ConversationCursor>,
    ) -> Result<(), ServiceFailure> {
        if self.custody.active_thread() != Some(&thread_id) {
            return Ok(());
        }
        self.custody.on_subscribe(thread_id.clone(), after);
        let subscribe = match after {
            Some(cursor) => ConversationSubscribe::resume(thread_id.clone(), cursor),
            None => ConversationSubscribe::fresh(thread_id.clone()),
        };
        let request = ClientRequest::Conversation(ConversationRequest::Subscribe(subscribe));
        let protocol_version = self
            .session
            .as_ref()
            .map(ClientSession::protocol_version)
            .ok_or(ServiceFailure::local_session())?;
        let (envelope, request_id) = make_request_frame(frames, protocol_version, request)
            .map_err(|_| ServiceFailure::invalid(ServiceFailureStage::Request))?;
        let expected = ExpectedResponse::ConversationSubscriptionStarted {
            thread_id: thread_id.clone(),
        };
        let session = self.session.take().ok_or(ServiceFailure::local_session())?;
        let attempt = self
            .exchange(session, envelope, request_id.clone(), expected)
            .await;
        match self.finish_request_attempt(attempt) {
            Ok(ResponsePayload::ConversationSubscriptionStarted(started)) => {
                if validate_started_correlation(&thread_id, &started).is_err() {
                    return Err(ServiceFailure::new(
                        ServiceFailureStage::Request,
                        ServiceFailureCategory::Integrity,
                    ));
                }
                self.custody.on_started(after, &started)?;
                Ok(())
            }
            Ok(_) => Err(ServiceFailure::invalid(ServiceFailureStage::Request)),
            Err(error) => Err(error.into()),
        }
    }
}

fn reconnect_custody_failure() -> ServiceFailure {
    ServiceFailure::new(
        ServiceFailureStage::Handshake,
        ServiceFailureCategory::Authentication,
    )
}

pub(super) async fn handle_subscribe(
    runtime: &mut ServiceRuntime,
    frames: &mut FrameFactory,
    events: &SyncSender<NativeTransportEvent>,
    thread_id: ThreadId,
    after: Option<ConversationCursor>,
) -> Result<(), RequestFailure> {
    // Thread switch tombstones old thread but does not restart the session delivery receiver.
    // Do not create an empty projection authority; the ConversationHost is the authority.
    runtime.custody.on_subscribe(thread_id.clone(), after);
    let subscribe = match after {
        Some(cursor) => ConversationSubscribe::resume(thread_id.clone(), cursor),
        None => ConversationSubscribe::fresh(thread_id.clone()),
    };
    let request = ClientRequest::Conversation(ConversationRequest::Subscribe(subscribe));
    let protocol_version = runtime
        .session
        .as_ref()
        .map(ClientSession::protocol_version)
        .ok_or(ServiceFailure::local_session())
        .map_err(RequestFailure::terminal)?;
    let (envelope, request_id) =
        make_request_frame(frames, protocol_version, request).map_err(RequestFailure::terminal)?;
    let expected = ExpectedResponse::ConversationSubscriptionStarted {
        thread_id: thread_id.clone(),
    };
    let session = runtime
        .session
        .take()
        .ok_or(ServiceFailure::local_session())
        .map_err(RequestFailure::terminal)?;
    let attempt = runtime
        .exchange(session, envelope, request_id.clone(), expected)
        .await;
    match runtime.finish_request_attempt(attempt) {
        Ok(ResponsePayload::ConversationSubscriptionStarted(started)) => {
            if validate_started_correlation(&thread_id, &started).is_err() {
                let failure = ServiceFailure::new(
                    ServiceFailureStage::Request,
                    ServiceFailureCategory::Integrity,
                );
                return Err(RequestFailure::terminal(failure));
            }
            runtime
                .custody
                .on_started(after, &started)
                .map_err(RequestFailure::terminal)?;
            // Do not install into a second projection; emit directly.
            // Cursor will be advanced only after explicit AcknowledgePatch from application.
            publish(
                events,
                NativeTransportEvent::ConversationSubscriptionStarted {
                    thread_id,
                    request_id,
                    started,
                },
            )
            .map_err(RequestFailure::terminal)
        }
        Ok(_) => Err(RequestFailure::terminal(ServiceFailure::invalid(
            ServiceFailureStage::Request,
        ))),
        Err(error) => Err(error),
    }
}

pub(super) async fn handle_unsubscribe(
    runtime: &mut ServiceRuntime,
    frames: &mut FrameFactory,
    events: &SyncSender<NativeTransportEvent>,
    thread_id: ThreadId,
) -> Result<(), ServiceFailure> {
    let is_active = runtime.custody.active_thread() == Some(&thread_id);
    if is_active {
        runtime.custody.on_unsubscribe(&thread_id);
    }
    let request =
        ClientRequest::Conversation(ConversationRequest::Unsubscribe(ConversationUnsubscribe {
            thread_id: thread_id.clone(),
        }));
    let protocol_version = runtime
        .session
        .as_ref()
        .map(ClientSession::protocol_version)
        .ok_or(ServiceFailure::local_session())?;
    let (envelope, request_id) = make_request_frame(frames, protocol_version, request)
        .map_err(|_| ServiceFailure::invalid(ServiceFailureStage::Request))?;
    let expected = ExpectedResponse::ConversationSubscriptionStopped {
        thread_id: thread_id.clone(),
    };
    let session = runtime
        .session
        .take()
        .ok_or(ServiceFailure::local_session())?;
    let attempt = runtime
        .exchange(session, envelope, request_id.clone(), expected)
        .await;
    match runtime.finish_request_attempt(attempt) {
        Ok(ResponsePayload::ConversationSubscriptionStopped(stopped)) => {
            if validate_stopped_correlation(&thread_id, &stopped).is_err() {
                let failure = ServiceFailure::new(
                    ServiceFailureStage::Request,
                    ServiceFailureCategory::Integrity,
                );
                return Err(failure);
            }
            publish(
                events,
                NativeTransportEvent::ConversationSubscriptionStopped {
                    thread_id,
                    request_id,
                    stopped,
                },
            )
        }
        Ok(_) => Err(ServiceFailure::invalid(ServiceFailureStage::Request)),
        Err(error) => Err(error.into()),
    }
}

pub(super) fn handle_acknowledge_patch(
    runtime: &mut ServiceRuntime,
    thread_id: &ThreadId,
    cursor: ConversationCursor,
) -> Result<(), ServiceFailure> {
    runtime.custody.on_acknowledge(thread_id, cursor)
}

pub(super) async fn handle_delivery_lost_reconnect(
    runtime: &mut ServiceRuntime,
    frames: &mut FrameFactory,
    events: &SyncSender<NativeTransportEvent>,
    failure: ServiceFailure,
) -> Result<(), ServiceFailure> {
    // Publish the loss first so application can observe
    publish(events, NativeTransportEvent::DeliveryLost(failure))?;
    // Perform cancel->join->lease checkout->connect->publish->take_delivery->new task.
    // The application owns the one recovery Subscribe from its mounted host cursor.
    runtime.reconnect(frames, false).await
}

pub(super) async fn handle_subscribe_command(
    runtime: &mut ServiceRuntime,
    frames: &mut FrameFactory,
    events: &SyncSender<NativeTransportEvent>,
    thread_id: ThreadId,
    after: Option<ConversationCursor>,
) -> Result<(), ServiceFailure> {
    if let Err(failure) = handle_subscribe(runtime, frames, events, thread_id, after).await {
        match subscription_failure_disposition(
            SubscriptionRequestKind::Subscribe,
            failure.failure,
            failure.retryable_local_session_loss,
        ) {
            SubscriptionFailureDisposition::RecoverDelivery => {
                handle_delivery_lost_reconnect(runtime, frames, events, failure.failure).await?;
            }
            SubscriptionFailureDisposition::Terminal => return Err(failure.failure),
        }
    }
    Ok(())
}
