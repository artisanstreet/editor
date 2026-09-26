//! Bounded request delivery for the native transport service: request
//! attempts, session admission, the delivery receive loop, and the
//! command/delivery select loop.
//!
//! The parent `native_transport_service` remains the owner of the command and
//! event vocabulary, session runtime fields, and domain request handlers.
//! Root mounts this file as a child module so send/receive/acknowledge
//! coordination stays in one reviewable home.

#![forbid(unsafe_code)]
#![allow(clippy::module_name_repetitions)]

use super::*;

/// Pushed deliveries on their way from the delivery task to the application.
///
/// The Forge serves a connection strictly in order and pushes after a
/// response, so it can be in the middle of a long push (a thread's history)
/// when the Editor sends its next request. That request is only answered once
/// the push has crossed the wire, and the push only crosses while the Editor
/// reads it. The Editor therefore keeps forwarding deliveries while it waits
/// for any response ([`ServiceRuntime::exchange`]); otherwise the bounded
/// delivery channel and the stream's flow-control window fill and both sides
/// wait for each other until their deadlines expire.
///
/// A delivery that needs the command loop (a loss, or a batch that does not
/// continue the forwarded cursor yet, such as the first batch of a fresh
/// subscription whose response is still being read) is parked, and the
/// exchange stops forwarding until its response settles. At most one
/// delivery is parked; the command loop handles it before anything else.
#[derive(Default)]
pub(super) struct DeliveryInbox {
    pub(super) receiver: Option<tokio::sync::mpsc::Receiver<PrivateDelivery>>,
    events: Option<SyncSender<NativeTransportEvent>>,
    parked: Option<PrivateDelivery>,
}

impl DeliveryInbox {
    /// Installs the delivery channel and the application bridge.
    pub(super) fn attach(
        &mut self,
        receiver: tokio::sync::mpsc::Receiver<PrivateDelivery>,
        events: SyncSender<NativeTransportEvent>,
    ) {
        self.receiver = Some(receiver);
        self.events = Some(events);
    }

    /// The next delivery for the command loop: the parked one first. A closed
    /// channel is a delivery loss; without a channel this never resolves.
    async fn next(&mut self) -> PrivateDelivery {
        if let Some(parked) = self.parked.take() {
            return parked;
        }
        match self.receiver.as_mut() {
            Some(receiver) => receiver.recv().await.unwrap_or_else(closed_delivery),
            None => std::future::pending().await,
        }
    }
}

fn closed_delivery() -> PrivateDelivery {
    PrivateDelivery::Lost(ServiceFailure::new(
        ServiceFailureStage::Delivery,
        ServiceFailureCategory::LocalSession,
    ))
}

/// Whether a delivery can be forwarded without the command loop's recovery.
fn forwards_without_recovery(custody: &SubscriptionCustody, delivery: &PrivateDelivery) -> bool {
    match delivery {
        PrivateDelivery::Batch(batch) => {
            let stale = custody
                .active_thread()
                .is_none_or(|thread| thread != batch.thread_id());
            stale || custody.expected_batch_from() == Some(batch.from_cursor())
        }
        PrivateDelivery::Observation(_)
        | PrivateDelivery::Outbox(_)
        | PrivateDelivery::HostState(_) => true,
        PrivateDelivery::Lost(_) => false,
    }
}

/// Forwards one delivery to the application, or names the failure the
/// command loop must recover from.
fn forward_delivery(
    custody: &mut SubscriptionCustody,
    events: &SyncSender<NativeTransportEvent>,
    delivery: PrivateDelivery,
) -> Result<Option<ServiceFailure>, ServiceFailure> {
    match delivery {
        PrivateDelivery::Batch(batch) => {
            let is_stale = custody
                .active_thread()
                .is_none_or(|thread| thread != batch.thread_id());
            if is_stale {
                return Ok(None);
            }
            // Validate cursor continuity against transport-forwarded
            // position first: a second contiguous batch may arrive before the
            // UI acknowledgement is selected from the command queue. The
            // subscribe baseline covers the first batch of an epoch and the
            // accepted cursor covers a live epoch with nothing forwarded yet.
            // Forwarding never marks application acceptance: the resume
            // baseline stays on the last explicit ack.
            if custody.expected_batch_from() != Some(batch.from_cursor()) {
                return Ok(Some(ServiceFailure::new(
                    ServiceFailureStage::Delivery,
                    ServiceFailureCategory::Integrity,
                )));
            }
            custody.on_batch_forwarded(batch.to_cursor());
            publish(events, NativeTransportEvent::PatchBatch(batch))?;
        }
        PrivateDelivery::Observation(observation) => {
            let artisan_domain::Event::EngineObservation(paired) = &observation.event else {
                return Ok(None);
            };
            if custody
                .active_thread()
                .is_some_and(|active| active == &paired.thread_id)
            {
                // The application owns cursor ordering and replay dedup; emit
                // without advancing custody, like patch batches.
                publish(events, NativeTransportEvent::EngineObservation(observation))?;
            }
        }
        PrivateDelivery::Outbox(outbox) => {
            if custody.active_thread() == Some(outbox.thread_id()) {
                publish(events, NativeTransportEvent::MessageOutbox(outbox))?;
            }
        }
        PrivateDelivery::HostState(state) => {
            publish(events, NativeTransportEvent::HostState(state))?;
        }
        PrivateDelivery::Lost(failure) => return Ok(Some(failure)),
    }
    Ok(None)
}

pub(super) async fn request_envelope_payload(
    session: ClientSession,
    envelope: WireEnvelope,
    expected_request_id: RequestId,
    expected: ExpectedResponse,
    cancel: &CancelHandle,
) -> Result<(ClientSession, ResponsePayload), RequestAttemptError> {
    let (session, resolved) =
        session
            .request(envelope, cancel)
            .await
            .map_err(|error| RequestAttemptError::Terminal {
                failure: ServiceFailure::local_session(),
                retryable_local_session_loss: local_session_request_loss_is_retryable(&error),
            })?;
    let (settled_request_id, outcome) = resolved.into_parts();
    if !request_id_matches(&expected_request_id, &settled_request_id) {
        return Err(RequestAttemptError::Retained {
            session: Box::new(session),
            failure: ServiceFailure::new(
                ServiceFailureStage::Request,
                ServiceFailureCategory::Integrity,
            ),
            peer: None,
        });
    }

    match outcome {
        RequestOutcome::Failure(failure) => {
            if !optional_request_id_matches(&expected_request_id, failure.request_id.as_ref()) {
                return Err(RequestAttemptError::Retained {
                    session: Box::new(session),
                    failure: ServiceFailure::new(
                        ServiceFailureStage::Request,
                        ServiceFailureCategory::Integrity,
                    ),
                    peer: None,
                });
            }
            let peer = PeerFailure {
                code: failure.code,
                retryable: failure.retryable,
            };
            Err(RequestAttemptError::Retained {
                session: Box::new(session),
                failure: ServiceFailure::new(
                    ServiceFailureStage::Request,
                    ServiceFailureCategory::Peer,
                ),
                peer: Some(peer),
            })
        }
        RequestOutcome::Response(response) => {
            if !request_id_matches(&expected_request_id, &response.request_id) {
                return Err(RequestAttemptError::Retained {
                    session: Box::new(session),
                    failure: ServiceFailure::new(
                        ServiceFailureStage::Request,
                        ServiceFailureCategory::Integrity,
                    ),
                    peer: None,
                });
            }
            match validate_response_family(expected, response.payload) {
                Ok(payload) => Ok((session, payload)),
                Err(failure) => Err(RequestAttemptError::Retained {
                    session: Box::new(session),
                    failure,
                    peer: None,
                }),
            }
        }
    }
}

impl ServiceRuntime {
    pub(super) async fn ensure_session(
        &mut self,
        frames: &mut FrameFactory,
        force_reconnect: bool,
    ) -> Result<(), ServiceFailure> {
        let needs_reconnect = match self.session.as_ref() {
            Some(session) => session_needs_reconnect(
                session.admitted(),
                session.admission_budget(),
                force_reconnect,
            ),
            None => true,
        };
        if needs_reconnect {
            self.reconnect(frames, true).await?;
        }
        Ok(())
    }

    pub(super) fn finish_request_attempt(
        &mut self,
        attempt: Result<(ClientSession, ResponsePayload), RequestAttemptError>,
    ) -> Result<ResponsePayload, RequestFailure> {
        match attempt {
            Ok((session, payload)) => {
                self.session = Some(session);
                Ok(payload)
            }
            Err(RequestAttemptError::Retained {
                session,
                failure,
                peer,
            }) => {
                self.session = Some(*session);
                Err(RequestFailure {
                    failure,
                    peer,
                    retryable_local_session_loss: false,
                })
            }
            Err(RequestAttemptError::Terminal {
                failure,
                retryable_local_session_loss,
            }) => {
                self.session = None;
                Err(RequestFailure {
                    failure,
                    peer: None,
                    retryable_local_session_loss,
                })
            }
        }
    }

    pub(super) async fn request(
        &mut self,
        frames: &mut FrameFactory,
        request: ClientRequest,
        expected: ExpectedResponse,
    ) -> Result<ResponsePayload, RequestFailure> {
        self.ensure_session(frames, false)
            .await
            .map_err(RequestFailure::terminal)?;
        let protocol_version = self
            .session
            .as_ref()
            .map(ClientSession::protocol_version)
            .ok_or(RequestFailure::terminal(ServiceFailure::local_session()))?;
        let (envelope, expected_request_id) = make_request_frame(frames, protocol_version, request)
            .map_err(RequestFailure::terminal)?;
        let session = self
            .session
            .take()
            .ok_or(RequestFailure::terminal(ServiceFailure::local_session()))?;
        let attempt = self
            .exchange(session, envelope, expected_request_id, expected)
            .await;
        self.finish_request_attempt(attempt)
    }

    /// Runs one request exchange while forwarding the Forge's pushes.
    ///
    /// See [`DeliveryInbox`]: the Forge may only answer this request after
    /// the push it is writing has been read, so waiting without reading would
    /// stall both sides until their deadlines expire.
    pub(super) async fn exchange(
        &mut self,
        session: ClientSession,
        envelope: WireEnvelope,
        expected_request_id: RequestId,
        expected: ExpectedResponse,
    ) -> Result<(ClientSession, ResponsePayload), RequestAttemptError> {
        let request = request_envelope_payload(
            session,
            envelope,
            expected_request_id,
            expected,
            &self.cancel,
        );
        let mut request = std::pin::pin!(request);
        let inbox = &mut self.deliveries;
        let custody = &mut self.custody;
        loop {
            let (Some(receiver), Some(events), None) = (
                inbox.receiver.as_mut(),
                inbox.events.as_ref(),
                &inbox.parked,
            ) else {
                return request.await;
            };
            tokio::select! {
                biased;
                attempt = &mut request => return attempt,
                delivery = receiver.recv() => {
                    let delivery = delivery.unwrap_or_else(closed_delivery);
                    if !forwards_without_recovery(custody, &delivery) {
                        inbox.parked = Some(delivery);
                        continue;
                    }
                    // A closed bridge ends the service at the handler's own
                    // publish; stop forwarding and let the request settle.
                    if forward_delivery(custody, events, delivery).is_err() {
                        return request.await;
                    }
                }
            }
        }
    }

    pub(super) async fn request_stable(
        &mut self,
        frames: &mut FrameFactory,
        mutation: &StableMutation,
        expected: ExpectedResponse,
        force_reconnect: bool,
    ) -> Result<ResponsePayload, RequestFailure> {
        self.ensure_session(frames, force_reconnect)
            .await
            .map_err(RequestFailure::terminal)?;
        let protocol_version = self
            .session
            .as_ref()
            .map(ClientSession::protocol_version)
            .ok_or(RequestFailure::terminal(ServiceFailure::local_session()))?;
        let (envelope, expected_request_id) = mutation
            .envelope(protocol_version)
            .map_err(RequestFailure::terminal)?;
        let session = self
            .session
            .take()
            .ok_or(RequestFailure::terminal(ServiceFailure::local_session()))?;
        let attempt = self
            .exchange(session, envelope, expected_request_id, expected)
            .await;
        self.finish_request_attempt(attempt)
    }
}

pub(super) fn session_needs_reconnect(
    admitted: usize,
    admission_budget: usize,
    force: bool,
) -> bool {
    force || admitted >= admission_budget
}

pub async fn delivery_task_loop(
    mut receiver: DeliveryReceiver,
    tx: tokio::sync::mpsc::Sender<PrivateDelivery>,
    cancel: Arc<CancelHandle>,
    expected_version: ProtocolVersion,
) {
    loop {
        if let Ok((next_receiver, envelope)) = receiver.recv(cancel.as_ref()).await {
            receiver = next_receiver;
            let result = match validate_uni_envelope(&envelope, expected_version) {
                Ok(UniDelivery::Batch(batch)) => PrivateDelivery::Batch(batch),
                Ok(UniDelivery::Observation(observation)) => {
                    PrivateDelivery::Observation(observation)
                }
                Ok(UniDelivery::Outbox(outbox)) => PrivateDelivery::Outbox(outbox),
                Ok(UniDelivery::HostState(state)) => PrivateDelivery::HostState(state),
                Err(failure) => PrivateDelivery::Lost(failure),
            };
            let is_lost = matches!(result, PrivateDelivery::Lost(_));
            if !hand_over(&tx, &cancel, result).await || is_lost {
                break;
            }
        } else {
            let _ = hand_over(&tx, &cancel, closed_delivery()).await;
            break;
        }
    }
}

/// Hands one delivery to the service loop. Cancellation wins over a full
/// channel, so a reconnect that cancels and joins this task never waits for
/// a consumer that is itself waiting on the join.
async fn hand_over(
    tx: &tokio::sync::mpsc::Sender<PrivateDelivery>,
    cancel: &CancelHandle,
    delivery: PrivateDelivery,
) -> bool {
    tokio::select! {
        biased;
        () = cancel.wait() => false,
        sent = tx.send(delivery) => sent.is_ok(),
    }
}

#[expect(
    clippy::too_many_lines,
    reason = "one select loop coordinates commands, private delivery, and shutdown; splitting it would scatter the shared frame factory"
)]
pub(super) async fn command_loop_with_delivery(
    commands: &mut tokio::sync::mpsc::Receiver<QueuedCommand>,
    runtime: &mut ServiceRuntime,
    frames: &mut FrameFactory,
    events: &SyncSender<NativeTransportEvent>,
) -> Result<(), ServiceFailure> {
    loop {
        let step = tokio::select! {
            cmd = commands.recv() => LoopStep::Command(cmd),
            delivery = runtime.deliveries.next() => LoopStep::Delivery(delivery),
        };
        match step {
            LoopStep::Command(cmd) => {
                // The hold travels with its command and drops when this arm
                // ends: after the handler returns, or with its failure.
                let Some(QueuedCommand {
                    command,
                    hold: _hold,
                }) = cmd
                else {
                    return Ok(());
                };
                match command {
                    NativeTransportCommand::Shutdown => return Ok(()),
                    NativeTransportCommand::BeginProjectIntakeAt(path) => {
                        project_intake::begin_project_intake_at(runtime, frames, events, path)
                            .await?;
                    }
                    NativeTransportCommand::BeginProjectIntake => {
                        begin_project_intake(runtime, frames, events).await?;
                    }
                    NativeTransportCommand::RetryProjectIntake => {
                        retry_project_intake(runtime, frames, events).await?;
                    }
                    NativeTransportCommand::ReadSidebarThreads {
                        project_id,
                        generation,
                    } => {
                        handlers::read_sidebar_threads(
                            runtime, frames, events, project_id, generation,
                        )
                        .await?;
                    }
                    NativeTransportCommand::SelectProject(project_id) => {
                        select_project(runtime, frames, events, project_id).await?;
                    }
                    NativeTransportCommand::CreateTask(project_id) => {
                        create_task_in_project(runtime, frames, events, project_id).await?;
                    }
                    NativeTransportCommand::RecoverFailedMessage {
                        project_id,
                        command,
                    } => {
                        project_intake::recover_failed_message(
                            runtime, frames, events, project_id, *command,
                        )
                        .await?;
                    }
                    NativeTransportCommand::ComposerState(command) => {
                        composer_state_operations::handle_composer_state_command(
                            runtime, frames, events, command,
                        )
                        .await?;
                    }
                    NativeTransportCommand::ComposerDraft(command) => {
                        composer_draft_operations::handle_composer_draft_command(
                            runtime, frames, events, command,
                        )
                        .await?;
                    }
                    NativeTransportCommand::Preferences(command) => {
                        preferences_operations::handle_preferences_command(
                            runtime, frames, events, command,
                        )
                        .await?;
                    }
                    NativeTransportCommand::ForgeDecision(command) => {
                        forge_decision_operations::handle_forge_decision_command(
                            runtime, frames, events, command,
                        )
                        .await?;
                    }
                    NativeTransportCommand::ReadActiveRun {
                        thread_id,
                        generation,
                    } => {
                        composer_operations::read_active_run(
                            runtime, frames, events, thread_id, generation,
                        )
                        .await?;
                    }
                    NativeTransportCommand::StopRun(command) => {
                        composer_operations::stop_run(runtime, frames, events, command).await?;
                    }
                    NativeTransportCommand::RespondApproval(command) => {
                        respond_approval(runtime, frames, events, *command).await?;
                    }
                    NativeTransportCommand::RespondQuestion(command) => {
                        respond_question(runtime, frames, events, *command).await?;
                    }
                    NativeTransportCommand::ReadMessageImage(reference) => {
                        read_message_image(runtime, frames, events, reference).await?;
                    }
                    NativeTransportCommand::RequestSnapshot(thread_id) => {
                        request_snapshot(runtime, frames, events, thread_id).await?;
                    }
                    NativeTransportCommand::LoadThreadEngineSettings {
                        thread_id,
                        generation,
                    } => {
                        load_thread_engine_settings(runtime, frames, events, thread_id, generation)
                            .await?;
                    }
                    NativeTransportCommand::ReadComposerCatalog {
                        thread_id,
                        profile_id,
                        generation,
                    } => {
                        composer_operations::read_composer_catalog(
                            runtime, frames, events, thread_id, profile_id, generation,
                        )
                        .await?;
                    }
                    NativeTransportCommand::ReadModelFavorites {
                        thread_id,
                        profile_id,
                        generation,
                    } => {
                        composer_operations::read_model_favorites(
                            runtime, frames, events, thread_id, profile_id, generation,
                        )
                        .await?;
                    }
                    NativeTransportCommand::ListRegisteredProfiles => {
                        list_registered_profiles(runtime, frames, events).await?;
                    }
                    NativeTransportCommand::ReadAccountUsage {
                        engine_id,
                        generation,
                        request_seq,
                        force,
                    } => {
                        profile_usage_operations::read_account_usage(
                            runtime,
                            frames,
                            events,
                            engine_id,
                            generation,
                            request_seq,
                            force,
                        )
                        .await?;
                    }
                    NativeTransportCommand::SetThreadEngineConfig(command) => {
                        set_thread_engine_config(runtime, frames, events, command).await?;
                    }
                    NativeTransportCommand::SetModelFavorite(command) => {
                        composer_operations::set_model_favorite(runtime, frames, events, *command)
                            .await?;
                    }
                    NativeTransportCommand::QueueFirstMessage(command) => {
                        queue_first_message(runtime, frames, events, *command).await?;
                    }
                    NativeTransportCommand::SubmitComposerDraft(command) => {
                        submit_composer_draft(runtime, frames, events, *command).await?;
                    }
                    NativeTransportCommand::ResolveRichLink { url } => {
                        resolve_rich_link(runtime, frames, events, url).await?;
                    }
                    NativeTransportCommand::QueryProjectRepository { project_id } => {
                        query_project_repository(runtime, frames, events, project_id).await?;
                    }
                    NativeTransportCommand::Subscribe { thread_id, after } => {
                        handle_subscribe_command(runtime, frames, events, thread_id, after).await?;
                    }
                    NativeTransportCommand::Unsubscribe { thread_id } => {
                        // The host is retiring; never turn an unsubscribe failure into a
                        // recovery Subscribe for that same host.
                        handle_unsubscribe(runtime, frames, events, thread_id).await?;
                    }
                    NativeTransportCommand::AcknowledgePatch { thread_id, cursor } => {
                        handle_acknowledge_patch(runtime, &thread_id, cursor)?
                    }
                }
            }
            LoopStep::Delivery(delivery) => {
                let recovery = forward_delivery(&mut runtime.custody, events, delivery)?;
                if let Some(failure) = recovery {
                    handle_delivery_lost_reconnect(runtime, frames, events, failure).await?;
                }
            }
        }
    }
}

enum LoopStep {
    Command(Option<QueuedCommand>),
    Delivery(PrivateDelivery),
}
