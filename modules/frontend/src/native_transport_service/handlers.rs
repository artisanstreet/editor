//! Domain request handlers for the native transport service: catalog and
//! snapshot reads, rich links, repository inspection, history images,
//! message submission, and durable engine-configuration saves.
//!
//! The parent `native_transport_service` remains the owner of the command and
//! event vocabulary, session runtime fields, and the bounded bridge
//! publication helper. Root mounts this file as a child module so per-command
//! handler flows stay in one reviewable home.

#![forbid(unsafe_code)]
#![allow(clippy::module_name_repetitions)]

use super::*;

pub(super) async fn load_initial_catalog(
    runtime: &mut ServiceRuntime,
    frames: &mut FrameFactory,
    events: &SyncSender<NativeTransportEvent>,
) -> Result<(), ServiceFailure> {
    let payload = runtime
        .request(frames, project_request(), ExpectedResponse::Projects)
        .await?;
    let ResponsePayload::ProjectListing(listing) = payload else {
        return Err(ServiceFailure::invalid(ServiceFailureStage::Request));
    };
    let first_project = listing
        .projects()
        .first()
        .map(|project| project.project_id.clone());
    publish(events, NativeTransportEvent::Projects(listing))?;
    let Some(project_id) = first_project else {
        publish(events, NativeTransportEvent::EmptyProjects)?;
        return Ok(());
    };
    select_project(runtime, frames, events, project_id).await
}

pub(super) async fn select_project(
    runtime: &mut ServiceRuntime,
    frames: &mut FrameFactory,
    events: &SyncSender<NativeTransportEvent>,
    project_id: ProjectId,
) -> Result<(), ServiceFailure> {
    let payload = runtime
        .request(
            frames,
            threads_request(project_id.clone()),
            ExpectedResponse::Threads(project_id.clone()),
        )
        .await?;
    let ResponsePayload::ThreadListing(listing) = payload else {
        return Err(ServiceFailure::invalid(ServiceFailureStage::Request));
    };
    runtime.known_threads.clear();
    runtime.known_threads.extend(
        listing
            .threads()
            .iter()
            .map(|thread| thread.thread_id.clone()),
    );
    let selection = thread_selection_decision(&listing);
    publish(
        events,
        NativeTransportEvent::Threads {
            project_id: project_id.clone(),
            listing,
        },
    )?;
    match selection {
        ThreadSelectionDecision::AwaitHostSnapshot(_) => Ok(()),
        ThreadSelectionDecision::Empty => {
            publish(events, NativeTransportEvent::EmptyThreads { project_id })
        }
    }
}

pub(super) async fn request_snapshot(
    runtime: &mut ServiceRuntime,
    frames: &mut FrameFactory,
    events: &SyncSender<NativeTransportEvent>,
    thread_id: ThreadId,
) -> Result<(), ServiceFailure> {
    if !runtime.known_threads.contains(&thread_id) {
        return Err(ServiceFailure::invalid(ServiceFailureStage::Request));
    }
    let payload = runtime
        .request(
            frames,
            snapshot_request(thread_id.clone())?,
            ExpectedResponse::Snapshot(thread_id),
        )
        .await?;
    let ResponsePayload::ConversationSnapshot(snapshot) = payload else {
        return Err(ServiceFailure::invalid(ServiceFailureStage::Request));
    };
    publish(events, NativeTransportEvent::Snapshot(snapshot))
}

/// Resolves one assistant-authored HTTP(S) link's page title.
///
/// The request is deliberately not thread-scoped: it reads no durable state,
/// so the one bounded resolve may serve whichever surface asked. Every
/// failure publishes the typed outcome instead of failing the command loop,
/// so the authored label stays visible for exactly one link.
pub(super) async fn resolve_rich_link(
    runtime: &mut ServiceRuntime,
    frames: &mut FrameFactory,
    events: &SyncSender<NativeTransportEvent>,
    url: String,
) -> Result<(), ServiceFailure> {
    let (request, expected_url) = match rich_link_request(&url) {
        Ok(request) => request,
        Err(failure) => {
            return publish(
                events,
                NativeTransportEvent::RichLinkFailed {
                    requested_url: url,
                    failure,
                },
            );
        }
    };
    let outcome = runtime
        .request(
            frames,
            request,
            ExpectedResponse::RichLink {
                requested_url: expected_url.clone(),
            },
        )
        .await;
    match outcome {
        Ok(ResponsePayload::RichLink(result)) => publish(
            events,
            NativeTransportEvent::RichLinkResolved {
                favicon: result.favicon,
                requested_url: result.requested_url,
                page_name: result.page_name,
                expires_at_ms: result.cache_expires_at_ms,
            },
        ),
        Ok(_) => publish(
            events,
            NativeTransportEvent::RichLinkFailed {
                requested_url: expected_url,
                failure: ServiceFailure::invalid(ServiceFailureStage::Request),
            },
        ),
        Err(failure) => publish(
            events,
            NativeTransportEvent::RichLinkFailed {
                requested_url: expected_url,
                failure: failure.into(),
            },
        ),
    }
}

/// Reads one attached project's repository identity.
///
/// The request is not thread-scoped: it inspects only the named project's
/// stored root and persists nothing. Every failure publishes the typed
/// outcome instead of failing the command loop, so the header keeps its
/// project-folder fallback rather than losing the connection.
pub(super) async fn query_project_repository(
    runtime: &mut ServiceRuntime,
    frames: &mut FrameFactory,
    events: &SyncSender<NativeTransportEvent>,
    project_id: ProjectId,
) -> Result<(), ServiceFailure> {
    let request = project_repository_request(project_id.clone());
    match runtime
        .request(
            frames,
            request,
            ExpectedResponse::ProjectRepository {
                project_id: project_id.clone(),
            },
        )
        .await
    {
        Ok(ResponsePayload::ProjectRepository(result)) => {
            let repository = result
                .repositories()
                .iter()
                .find(|entry| entry.project_id() == &project_id)
                .map(|entry| entry.repository().clone());
            publish(
                events,
                NativeTransportEvent::ProjectRepository {
                    project_id,
                    repository,
                },
            )
        }
        Ok(_) => publish(
            events,
            NativeTransportEvent::ProjectRepositoryFailed {
                project_id,
                failure: ServiceFailure::invalid(ServiceFailureStage::Request),
            },
        ),
        Err(error) => publish(
            events,
            NativeTransportEvent::ProjectRepositoryFailed {
                project_id,
                failure: error.into(),
            },
        ),
    }
}

pub(super) async fn read_message_image(
    runtime: &mut ServiceRuntime,
    frames: &mut FrameFactory,
    events: &SyncSender<NativeTransportEvent>,
    reference: artisan_domain::ImageAttachmentRef,
) -> Result<(), ServiceFailure> {
    if !runtime.known_threads.contains(&reference.thread_id) {
        return publish(
            events,
            NativeTransportEvent::MessageImageFailed {
                reference,
                failure: ServiceFailure::invalid(ServiceFailureStage::Request),
            },
        );
    }
    let query = Query::ReadMessageImage(artisan_domain::ReadMessageImage::new(
        reference.thread_id.clone(),
        reference.message_id.clone(),
        reference.index,
    ));
    let response = runtime
        .request(
            frames,
            query_request(query),
            ExpectedResponse::MessageImage(reference.clone()),
        )
        .await;
    let result = match response {
        Ok(ResponsePayload::MessageImage(result)) => result,
        Ok(_) => {
            return publish(
                events,
                NativeTransportEvent::MessageImageFailed {
                    reference,
                    failure: ServiceFailure::invalid(ServiceFailureStage::Request),
                },
            );
        }
        Err(error) => {
            return publish(
                events,
                NativeTransportEvent::MessageImageFailed {
                    reference,
                    failure: error.into(),
                },
            );
        }
    };
    match artisan_domain::ImageAttachment::new(
        reference.mime_type_str(),
        result.bytes,
        reference.name.clone(),
    ) {
        Ok(image) => publish(
            events,
            NativeTransportEvent::MessageImageLoaded { reference, image },
        ),
        Err(_) => publish(
            events,
            NativeTransportEvent::MessageImageFailed {
                reference,
                failure: ServiceFailure::invalid(ServiceFailureStage::Request),
            },
        ),
    }
}

pub(super) async fn load_thread_engine_settings(
    runtime: &mut ServiceRuntime,
    frames: &mut FrameFactory,
    events: &SyncSender<NativeTransportEvent>,
    thread_id: ThreadId,
    generation: SettingsLoadGeneration,
) -> Result<(), ServiceFailure> {
    if !runtime.known_threads.contains(&thread_id) {
        return publish(
            events,
            NativeTransportEvent::ThreadEngineSettingsFailed {
                thread_id,
                generation,
                failure: ServiceFailure::invalid(ServiceFailureStage::Request),
            },
        );
    }
    let payload = match runtime
        .request(
            frames,
            thread_engine_settings_request(thread_id.clone()),
            ExpectedResponse::ThreadEngineSettings(thread_id.clone()),
        )
        .await
    {
        Ok(payload) => payload,
        Err(error) => {
            let failure: ServiceFailure = error.into();
            return publish(
                events,
                NativeTransportEvent::ThreadEngineSettingsFailed {
                    thread_id: thread_id.clone(),
                    generation,
                    failure,
                },
            );
        }
    };
    let ResponsePayload::ThreadEngineSettings(result) = payload else {
        return publish(
            events,
            NativeTransportEvent::ThreadEngineSettingsFailed {
                thread_id,
                generation,
                failure: ServiceFailure::invalid(ServiceFailureStage::Request),
            },
        );
    };
    if result.thread_id() != &thread_id {
        return publish(
            events,
            NativeTransportEvent::ThreadEngineSettingsFailed {
                thread_id,
                generation,
                failure: ServiceFailure::invalid(ServiceFailureStage::Request),
            },
        );
    }
    publish(
        events,
        NativeTransportEvent::ThreadEngineSettings { generation, result },
    )
}

pub(super) async fn list_registered_profiles(
    runtime: &mut ServiceRuntime,
    frames: &mut FrameFactory,
    events: &SyncSender<NativeTransportEvent>,
) -> Result<(), ServiceFailure> {
    let payload = match runtime
        .request(
            frames,
            registered_profiles_request(),
            ExpectedResponse::RegisteredProfiles,
        )
        .await
    {
        Ok(payload) => payload,
        Err(error) => {
            let failure: ServiceFailure = error.into();
            return publish(
                events,
                NativeTransportEvent::RegisteredProfilesFailed(failure),
            );
        }
    };
    let ResponsePayload::RegisteredEngineProfiles(result) = payload else {
        return publish(
            events,
            NativeTransportEvent::RegisteredProfilesFailed(ServiceFailure::invalid(
                ServiceFailureStage::Request,
            )),
        );
    };
    publish(events, NativeTransportEvent::RegisteredProfiles(result))
}

pub(super) async fn queue_first_message(
    runtime: &mut ServiceRuntime,
    frames: &mut FrameFactory,
    events: &SyncSender<NativeTransportEvent>,
    command: QueueFirstMessage,
) -> Result<(), ServiceFailure> {
    let thread_id = command.thread_id.clone();
    let request_id = command.request_id.clone();
    if known_thread_for_queue(&runtime.known_threads, &thread_id).is_err() {
        return publish(
            events,
            NativeTransportEvent::FirstMessageFailed {
                thread_id,
                request_id,
                failure: ServiceFailure::invalid(ServiceFailureStage::Request),
            },
        );
    }
    let mutation = match first_message_stable_mutation(command) {
        Ok(mutation) => mutation,
        Err(failure) => {
            return publish(
                events,
                NativeTransportEvent::FirstMessageFailed {
                    thread_id,
                    request_id,
                    failure,
                },
            );
        }
    };
    let payload = match durable_save_request(
        runtime,
        frames,
        &mutation,
        ExpectedResponse::FirstMessageQueued {
            thread_id: thread_id.clone(),
            request_id: request_id.clone(),
        },
    )
    .await
    {
        Ok(payload) => payload,
        Err(error) => {
            return publish(
                events,
                NativeTransportEvent::FirstMessageFailed {
                    thread_id,
                    request_id,
                    failure: error.into(),
                },
            );
        }
    };
    let ResponsePayload::FirstMessageQueued(receipt) = payload else {
        return publish(
            events,
            NativeTransportEvent::FirstMessageFailed {
                thread_id,
                request_id,
                failure: ServiceFailure::invalid(ServiceFailureStage::Request),
            },
        );
    };
    if receipt.thread_id != thread_id || receipt.request_id != request_id {
        return publish(
            events,
            NativeTransportEvent::FirstMessageFailed {
                thread_id,
                request_id,
                failure: ServiceFailure::new(
                    ServiceFailureStage::Request,
                    ServiceFailureCategory::Integrity,
                ),
            },
        );
    }
    publish(events, NativeTransportEvent::FirstMessageQueued(receipt))
}

/// Sends one thread's composer draft at the revision its body was stored
/// under. The Forge's answer names the message (the first submission's
/// message when this revision was already sent) or refuses a stale revision.
pub(super) async fn submit_composer_draft(
    runtime: &mut ServiceRuntime,
    frames: &mut FrameFactory,
    events: &SyncSender<NativeTransportEvent>,
    command: SubmitComposerDraft,
) -> Result<(), ServiceFailure> {
    let thread_id = command.thread_id.clone();
    let request_id = command.request_id.clone();
    let failed = |failure| NativeTransportEvent::MessageFailed {
        thread_id: thread_id.clone(),
        request_id: request_id.clone(),
        failure,
    };
    if known_thread_for_queue(&runtime.known_threads, &thread_id).is_err() {
        return publish(
            events,
            failed(ServiceFailure::invalid(ServiceFailureStage::Request)),
        );
    }
    let mutation = match draft_submission_mutation(command) {
        Ok(mutation) => mutation,
        Err(failure) => return publish(events, failed(failure)),
    };
    let expected = ExpectedResponse::DraftSubmitted {
        thread_id: thread_id.clone(),
        request_id: request_id.clone(),
    };
    let submitted = match durable_save_request(runtime, frames, &mutation, expected).await {
        Ok(ResponsePayload::ComposerDraftSubmitted(submitted)) => submitted,
        Ok(_) => {
            return publish(
                events,
                failed(ServiceFailure::invalid(ServiceFailureStage::Request)),
            );
        }
        Err(error) => return publish(events, failed(error.into())),
    };
    let scope = artisan_domain::ComposerDraftScope::Thread(thread_id.clone());
    match submitted.outcome {
        artisan_domain::DraftSubmissionOutcome::Queued {
            message_id,
            disposition,
            cleared_revision,
            engine_config_revision,
        } => {
            publish(
                events,
                NativeTransportEvent::ComposerDraft(ComposerDraftEvent::Revision {
                    scope,
                    revision: cleared_revision,
                }),
            )?;
            publish(
                events,
                NativeTransportEvent::ForgeDecision(ForgeDecisionEvent::SendAdmitted {
                    thread_id: thread_id.clone(),
                    engine_config_revision,
                }),
            )?;
            publish(
                events,
                NativeTransportEvent::MessageQueued(QueueMessageReceipt {
                    request_id,
                    message_id,
                    thread_id,
                    disposition,
                }),
            )
        }
        artisan_domain::DraftSubmissionOutcome::Stale { current_revision } => publish(
            events,
            NativeTransportEvent::MessageStale {
                thread_id,
                request_id,
                current_revision,
            },
        ),
        artisan_domain::DraftSubmissionOutcome::Refused(refusal) => publish(
            events,
            NativeTransportEvent::ForgeDecision(ForgeDecisionEvent::SendRefused {
                thread_id,
                request_id,
                refusal,
            }),
        ),
    }
}

pub(super) fn known_thread_for_queue(
    known_threads: &HashSet<ThreadId>,
    thread_id: &ThreadId,
) -> Result<(), ServiceFailure> {
    if known_threads.contains(thread_id) {
        Ok(())
    } else {
        Err(ServiceFailure::invalid(ServiceFailureStage::Request))
    }
}

pub(super) async fn set_thread_engine_config(
    runtime: &mut ServiceRuntime,
    frames: &mut FrameFactory,
    events: &SyncSender<NativeTransportEvent>,
    command: Box<SetThreadEngineConfig>,
) -> Result<(), ServiceFailure> {
    let thread_id = command.thread_id().clone();
    let request_id = command.request_id().clone();
    let retained = command.config().clone();
    if !runtime.known_threads.contains(&thread_id) {
        return publish_engine_config_failure(
            events,
            thread_id,
            request_id,
            ServiceFailure::invalid(ServiceFailureStage::Request),
        );
    }
    let mutation = match engine_config_stable_mutation(command) {
        Ok(mutation) => mutation,
        Err(failure) => {
            return publish_engine_config_failure(events, thread_id, request_id, failure);
        }
    };
    let payload = match durable_save_request(
        runtime,
        frames,
        &mutation,
        expected_engine_config_response(&thread_id, &request_id),
    )
    .await
    {
        Ok(payload) => payload,
        Err(error) if error.code() == Some(ErrorCode::EngineConfigConflict) => {
            return publish_engine_config_conflict(events, thread_id, request_id);
        }
        Err(error) => {
            return publish_engine_config_failure(events, thread_id, request_id, error.into());
        }
    };
    finish_engine_config_save(events, thread_id, request_id, retained, payload)
}

fn expected_engine_config_response(
    thread_id: &ThreadId,
    request_id: &RequestId,
) -> ExpectedResponse {
    ExpectedResponse::ThreadEngineConfigSet {
        thread_id: thread_id.clone(),
        request_id: request_id.clone(),
    }
}

pub(super) async fn durable_save_request(
    runtime: &mut ServiceRuntime,
    frames: &mut FrameFactory,
    mutation: &StableMutation,
    expected: ExpectedResponse,
) -> Result<ResponsePayload, RequestFailure> {
    let first_attempt = runtime
        .request_stable(frames, mutation, expected.clone(), false)
        .await;
    match first_attempt {
        Ok(payload) => Ok(payload),
        Err(error) if error.durable_save_retry_allowed() => {
            runtime
                .request_stable(frames, mutation, expected, true)
                .await
        }
        Err(error) => Err(error),
    }
}

fn publish_engine_config_failure(
    events: &SyncSender<NativeTransportEvent>,
    thread_id: ThreadId,
    request_id: RequestId,
    failure: ServiceFailure,
) -> Result<(), ServiceFailure> {
    publish(
        events,
        NativeTransportEvent::ThreadEngineConfigFailed {
            thread_id,
            request_id,
            failure,
        },
    )
}

fn publish_engine_config_conflict(
    events: &SyncSender<NativeTransportEvent>,
    thread_id: ThreadId,
    request_id: RequestId,
) -> Result<(), ServiceFailure> {
    publish(
        events,
        NativeTransportEvent::ThreadEngineConfigConflict {
            thread_id,
            request_id,
        },
    )
}

fn finish_engine_config_save(
    events: &SyncSender<NativeTransportEvent>,
    thread_id: ThreadId,
    request_id: RequestId,
    retained: EngineRunConfig,
    payload: ResponsePayload,
) -> Result<(), ServiceFailure> {
    let ResponsePayload::ThreadEngineConfigSet(result) = payload else {
        return publish_engine_config_failure(
            events,
            thread_id,
            request_id,
            ServiceFailure::invalid(ServiceFailureStage::Request),
        );
    };
    if result.thread_id != thread_id || result.request_id != request_id {
        return publish_engine_config_failure(
            events,
            thread_id,
            request_id,
            ServiceFailure::new(
                ServiceFailureStage::Request,
                ServiceFailureCategory::Integrity,
            ),
        );
    }
    publish(
        events,
        NativeTransportEvent::ThreadEngineConfigSet(result, Box::new(retained)),
    )
}

/// Refreshes only catalog data; it never publishes selection/empty-state events.
pub(super) async fn read_sidebar_threads(
    runtime: &mut ServiceRuntime,
    frames: &mut FrameFactory,
    events: &SyncSender<NativeTransportEvent>,
    project_id: ProjectId,
    generation: u64,
) -> Result<(), ServiceFailure> {
    let result = match runtime
        .request(
            frames,
            threads_request(project_id.clone()),
            ExpectedResponse::Threads(project_id.clone()),
        )
        .await
    {
        Ok(ResponsePayload::ThreadListing(listing)) => {
            runtime.known_threads.clear();
            runtime.known_threads.extend(
                listing
                    .threads()
                    .iter()
                    .map(|thread| thread.thread_id.clone()),
            );
            Ok(listing)
        }
        Ok(_) => Err(ServiceFailure::invalid(ServiceFailureStage::Request)),
        Err(failure) => Err(failure.into()),
    };
    publish(
        events,
        NativeTransportEvent::SidebarThreads {
            project_id,
            generation,
            result,
        },
    )
}
