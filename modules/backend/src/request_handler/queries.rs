//! Read-side request handling: repository queries, directory picking,
//! and the bounded rich-link and project-repository adapter reads.
//!
//! Entry methods are `pub(super)` so the root dispatcher keeps one
//! `respond` seam; every result maps through the shared protocol
//! vocabulary owned by the parent module.

use std::time::Instant;

use artisan_domain::{ConversationRequest, DirectoryId, Query, RequestId, RootPath};
use artisan_protocol::{
    ActiveRunResult, DirectoryPickOutcome as ProtocolDirectoryPickOutcome, ErrorCode,
    MessageImageResult, ProjectRepositoryQuery, ProjectRepositoryQueryResult, ProtocolFailure,
    RegisteredEngineProfilesResult, ResolveRichLinkRequest, ResponsePayload, RichLinkPageMetadata,
    RunLiveStatus, ServerResponse,
};

use crate::directory_controller::{AdmissionError, DirectoryPickOutcome, HelperOperationError};
use crate::directory_selection::DirectorySelectionAdmissionError;

use super::failures::{
    forged_identity_failure, origin_entropy_failure, outcome, repository_failure,
    run_cancellation_failure, typed_failure, unbacked_failure, unknown_directory_failure,
};
use super::{RUN_CANCELLATION_UNAVAILABLE_DETAIL, RequestHandler};

impl RequestHandler {
    pub(super) async fn pick_directory_outcome(
        &self,
        request_id: &RequestId,
        selected_path: Option<&str>,
    ) -> Result<ServerResponse, ProtocolFailure> {
        let Some(directory_picker) = self.directory_picker.as_ref() else {
            return Err(unbacked_failure(request_id, "native directory picking"));
        };

        let operation = match selected_path {
            Some(path) => directory_picker
                .controller
                .validate_directory(directory_picker.budget, path),
            None => directory_picker
                .controller
                .pick_directory(directory_picker.budget),
        };
        let pick_result = operation
            .map_err(|error| directory_admission_failure(error, request_id))?
            .await
            .map_err(|error| helper_operation_failure(&error, request_id))?;
        let root_path = match pick_result {
            DirectoryPickOutcome::Selected { canonical_path } => RootPath::parse(canonical_path)
                .map_err(|_| {
                    typed_failure(
                        ErrorCode::Internal,
                        "native directory picker returned an invalid canonical root path",
                        false,
                        request_id,
                    )
                })?,
            DirectoryPickOutcome::Cancelled => {
                return Ok(outcome(
                    request_id,
                    ResponsePayload::DirectoryPicked(ProtocolDirectoryPickOutcome::Cancelled),
                ));
            }
            other => return Err(picker_outcome_failure(&other, request_id)),
        };

        let identity = self
            .origin
            .mint_identity()
            .map_err(|error| origin_entropy_failure(&error, request_id))?;
        let directory_id = DirectoryId::parse(identity)
            .map_err(|_| forged_identity_failure("directory", request_id))?;
        let mut authority = directory_picker.authority.lock().await;
        let issued = authority
            .register(directory_id, root_path, Instant::now())
            .map_err(|error| directory_selection_failure(error, request_id))?;
        Ok(outcome(
            request_id,
            ResponsePayload::DirectoryPicked(ProtocolDirectoryPickOutcome::Selected(
                issued.directory_id,
            )),
        ))
    }

    /// Resolves one bounded rich-link metadata read through the attached
    /// resolver.
    ///
    /// The resolver applies its own URL policy before any fetch; this adapter
    /// only classifies the typed failure into the protocol vocabulary and
    /// re-validates the resolved metadata before it crosses the wire.
    pub(super) async fn resolve_rich_link_outcome(
        &self,
        request_id: &RequestId,
        request: &ResolveRichLinkRequest,
    ) -> Result<ServerResponse, ProtocolFailure> {
        let Some(resolver) = self.rich_link_resolver.as_ref() else {
            return Err(unbacked_failure(
                request_id,
                "rich-link metadata resolution",
            ));
        };
        let resolution = resolver
            .resolve(request.url())
            .await
            .map_err(|error| rich_link_failure(error, request_id))?;
        let metadata = RichLinkPageMetadata::new(
            resolution.requested_url,
            resolution.page_name,
            resolution.expires_at_ms,
        )
        .and_then(|metadata| metadata.with_favicon(resolution.favicon))
        .map_err(|_| {
            typed_failure(
                ErrorCode::Internal,
                "rich-link resolution produced invalid metadata",
                false,
                request_id,
            )
        })?;
        Ok(outcome(request_id, ResponsePayload::RichLink(metadata)))
    }

    /// Resolves one bounded project-repository read through the attached
    /// service.
    ///
    /// The service reads the durable catalog and runs the bounded Git
    /// inspection; this adapter only re-validates the projected entries
    /// before they cross the wire and classifies catalog failures.
    pub(super) async fn query_project_repository_outcome(
        &self,
        request_id: &RequestId,
        query: &ProjectRepositoryQuery,
    ) -> Result<ServerResponse, ProtocolFailure> {
        let Some(service) = self.project_repository.as_ref() else {
            return Err(unbacked_failure(
                request_id,
                "project repository inspection",
            ));
        };
        let entries = service
            .inspect(query.project_ids())
            .await
            .map_err(|error| project_repository_failure(error, request_id))?;
        let result = ProjectRepositoryQueryResult::new(entries).map_err(|_| {
            typed_failure(
                ErrorCode::Internal,
                "project repository read produced an invalid result",
                false,
                request_id,
            )
        })?;
        Ok(outcome(
            request_id,
            ResponsePayload::ProjectRepository(result),
        ))
    }

    /// Answers pure reads from repository listings.
    #[expect(
        clippy::too_many_lines,
        reason = "one read-dispatch over the query vocabulary; extraction would fragment the shared typed-failure mapping"
    )]
    pub(super) async fn query_outcome(
        &self,
        request_id: &RequestId,
        query: &Query,
    ) -> Result<ServerResponse, ProtocolFailure> {
        match query {
            Query::ListAttachedProjects(_) => {
                let listing = self
                    .repository
                    .list_projects()
                    .await
                    .map_err(|error| repository_failure(&error, request_id))?;
                Ok(outcome(
                    request_id,
                    ResponsePayload::ProjectListing(listing),
                ))
            }
            Query::ListProjectThreads(listing) => {
                let threads = self
                    .repository
                    .list_threads(&listing.project_id)
                    .await
                    .map_err(|error| repository_failure(&error, request_id))?;
                let mut rows = threads.threads().to_vec();
                if let Some(registry) = self.run_cancellation.as_ref() {
                    for thread in &mut rows {
                        let run = registry
                            .active_run(&thread.thread_id)
                            .map_err(|error| run_cancellation_failure(error, request_id))?;
                        if let Some(run) = run {
                            thread.has_active_work = self
                                .repository
                                .read_assistant_run_status(&thread.thread_id, &run)
                                .await
                                .map_err(|error| repository_failure(&error, request_id))?
                                .is_some_and(|(lifecycle, _)| {
                                    run_live_status(&lifecycle).is_some()
                                });
                        }
                    }
                }
                let threads = artisan_domain::ThreadListing::new(rows)
                    .expect("enriching existing rows preserves listing identities and bounds");
                Ok(outcome(request_id, ResponsePayload::ThreadListing(threads)))
            }
            Query::ListDirectories(browse) => match &browse.parent {
                Some(parent) => Err(unknown_directory_failure(request_id, parent)),
                None => Err(unbacked_failure(request_id, "directory browsing")),
            },
            Query::ReadThreadEngineSettings(read) => {
                let thread_id = read.thread_id();
                match self.repository.read_thread_engine_settings(thread_id).await {
                    Ok(None) => Ok(outcome(
                        request_id,
                        ResponsePayload::ThreadEngineSettings(
                            artisan_protocol::ThreadEngineSettingsResult::Unconfigured {
                                thread_id: thread_id.clone(),
                            },
                        ),
                    )),
                    Ok(Some(settings)) => Ok(outcome(
                        request_id,
                        ResponsePayload::ThreadEngineSettings(
                            artisan_protocol::ThreadEngineSettingsResult::Configured {
                                thread_id: thread_id.clone(),
                                revision: settings.revision(),
                                config: Box::new(settings.config().clone()),
                            },
                        ),
                    )),
                    Err(error) => Err(repository_failure(&error, request_id)),
                }
            }
            Query::ReadMessageImage(read) => {
                let image = self
                    .repository
                    .read_message_image(read.thread_id(), read.message_id(), read.index())
                    .await
                    .map_err(|error| repository_failure(&error, request_id))?;
                let Some(image) = image else {
                    return Err(typed_failure(
                        ErrorCode::InvalidInput,
                        "message image is unavailable",
                        false,
                        request_id,
                    ));
                };
                Ok(outcome(
                    request_id,
                    ResponsePayload::MessageImage(MessageImageResult {
                        reference: image.reference,
                        bytes: image.bytes,
                    }),
                ))
            }
            Query::ReadActiveRun(read) => {
                let Some(registry) = self.run_cancellation.as_ref() else {
                    return Err(typed_failure(
                        ErrorCode::UnsupportedFeature,
                        RUN_CANCELLATION_UNAVAILABLE_DETAIL,
                        false,
                        request_id,
                    ));
                };
                let result = registry
                    .active_run(read.thread_id())
                    .map_err(|error| run_cancellation_failure(error, request_id))?;
                let result = match result {
                    Some(run_id) => {
                        match self
                            .repository
                            .read_assistant_run_status(read.thread_id(), &run_id)
                            .await
                        {
                            Ok(Some((lifecycle, engine_id))) => {
                                match run_live_status(&lifecycle) {
                                    Some(status) => ActiveRunResult::Active {
                                        thread_id: read.thread_id().clone(),
                                        run_id,
                                        status,
                                        engine_id,
                                    },
                                    // Settled lifecycles are not live, however
                                    // the registry entry reads.
                                    None => ActiveRunResult::NoActive {
                                        thread_id: read.thread_id().clone(),
                                    },
                                }
                            }
                            // Missing or thread-mismatched rows are not live.
                            Ok(None) => ActiveRunResult::NoActive {
                                thread_id: read.thread_id().clone(),
                            },
                            Err(_) => {
                                return Err(typed_failure(
                                    ErrorCode::Internal,
                                    "active run status is unavailable",
                                    true,
                                    request_id,
                                ));
                            }
                        }
                    }
                    None => ActiveRunResult::NoActive {
                        thread_id: read.thread_id().clone(),
                    },
                };
                Ok(outcome(request_id, ResponsePayload::ActiveRun(result)))
            }
            Query::ListQueuedMessages(query) => self.read_composer_queue(request_id, query).await,
            Query::ListFailedMessages(query) => {
                self.read_failed_dispatches(request_id, query).await
            }
            Query::ReadRecalledMessage(query) => {
                self.read_recalled_composer_message(request_id, query).await
            }
            Query::ReadRunUsage(query) => self.read_composer_usage(request_id, query).await,
            Query::ReadComposerDraft(read) => self.read_composer_draft(request_id, read).await,
            Query::ReadComposerAttachment(read) => {
                self.read_composer_attachment(request_id, read).await
            }
            Query::ReadComposerCatalog(read) => {
                crate::composer_catalog_handler::read_composer_catalog(
                    self.composer_catalog.as_ref(),
                    self.account_usage.as_deref(),
                    &self.repository,
                    request_id,
                    read,
                )
                .await
            }
            Query::ResolveModelSelection(query) => {
                self.resolve_model_selection_outcome(request_id, query)
                    .await
            }
            Query::ReadHostCatalog(_) => {
                crate::composer_catalog_handler::read_host_catalog(
                    self.account_usage.as_deref(),
                    request_id,
                )
                .await
            }
            Query::ReadUserPreferences(_) => self.read_user_preferences_outcome(request_id).await,
            Query::ResolveEngineConfiguration(query) => {
                Ok(self.resolve_engine_configuration_outcome(request_id, query))
            }
            Query::ReadModelFavorites(_) => {
                crate::composer_catalog_handler::read_model_favorites(&self.repository, request_id)
                    .await
            }
            Query::ReadAccountUsage(query) => {
                crate::account_usage_handler::read_account_usage(
                    self.account_usage.as_deref(),
                    request_id,
                    query,
                )
                .await
            }
            Query::ListRegisteredEngineProfiles(_) => {
                let Some(reader) = self.registered_engine_profiles.as_ref() else {
                    return Err(typed_failure(
                        ErrorCode::UnsupportedFeature,
                        "registered engine profiles are not supported",
                        false,
                        request_id,
                    ));
                };
                match reader.list_profiles() {
                    Ok(None) => Ok(outcome(
                        request_id,
                        ResponsePayload::RegisteredEngineProfiles(
                            RegisteredEngineProfilesResult::RegistryMissing,
                        ),
                    )),
                    Ok(Some(profile_ids)) => Ok(outcome(
                        request_id,
                        ResponsePayload::RegisteredEngineProfiles(
                            RegisteredEngineProfilesResult::RegistryPresent { profile_ids },
                        ),
                    )),
                    Err(_) => Err(typed_failure(
                        ErrorCode::Internal,
                        "registered engine profiles are unavailable",
                        false,
                        request_id,
                    )),
                }
            }
        }
    }

    /// Answers conversation reads and subscription control.
    ///
    /// `ConversationRequest::Query` calls `Repository::read_conversation_snapshot`
    /// exactly once and preserves the caller's correlated `RequestId`. A query
    /// carries no command id and never consults [`CommandOrigin`]. Failures map
    /// through [`repository_failure`]: absent thread is `ThreadUnknown`,
    /// corrupt or invariant persisted state is non-retryable `Internal`, and
    /// database operation failures are retryable `Internal`. Subscribe and
    /// unsubscribe remain unbacked in this build pending a per-connection
    /// registrar and answer with a subscription-specific non-retryable
    /// `Internal` failure.
    pub(super) async fn conversation_outcome(
        &self,
        request_id: &RequestId,
        conversation: &ConversationRequest,
    ) -> Result<ServerResponse, ProtocolFailure> {
        match conversation {
            ConversationRequest::Query(query) => {
                let snapshot = self
                    .repository
                    .read_conversation_snapshot(query)
                    .await
                    .map_err(|error| repository_failure(&error, request_id))?;
                Ok(outcome(
                    request_id,
                    ResponsePayload::ConversationSnapshot(
                        crate::citation_projection::resolve_snapshot(&self.repository, snapshot)
                            .await,
                    ),
                ))
            }
            ConversationRequest::Subscribe(_) => {
                Err(unbacked_failure(request_id, "conversation subscription"))
            }
            ConversationRequest::Unsubscribe(_) => {
                Err(unbacked_failure(request_id, "conversation unsubscription"))
            }
        }
    }
}

/// Maps a durable run lifecycle to the live status the composer
/// starting-guard reads. Settled lifecycles map to `None`: they are not
/// live, however any registry entry reads.
pub(super) fn run_live_status(
    lifecycle: &artisan_database::entities::AssistantRunLifecycle,
) -> Option<RunLiveStatus> {
    use artisan_database::entities::AssistantRunLifecycle as Lifecycle;
    match lifecycle {
        Lifecycle::Queued | Lifecycle::Launching => Some(RunLiveStatus::Queued),
        Lifecycle::Running => Some(RunLiveStatus::Running),
        Lifecycle::Waiting | Lifecycle::CancelRequested => Some(RunLiveStatus::Waiting),
        Lifecycle::Interrupted
        | Lifecycle::Completed
        | Lifecycle::Failed
        | Lifecycle::Cancelled => None,
    }
}

/// Maps controller queue admission without exposing any operation payload.
fn directory_admission_failure(error: AdmissionError, request_id: &RequestId) -> ProtocolFailure {
    let (detail, retryable) = match error {
        AdmissionError::Unavailable => ("native directory picker is unavailable", false),
        AdmissionError::Busy => ("native directory picker is busy", true),
        AdmissionError::InvalidDeadline => ("native directory picker deadline is invalid", false),
        AdmissionError::EmptyPath => ("native directory picker path is empty", false),
        AdmissionError::PathTooLong => ("native directory picker path is too long", false),
    };
    typed_failure(ErrorCode::Internal, detail, retryable, request_id)
}

/// Maps one controller operation failure to the stable protocol vocabulary.
fn helper_operation_failure(
    error: &HelperOperationError,
    request_id: &RequestId,
) -> ProtocolFailure {
    let (detail, retryable) = match error {
        HelperOperationError::Cancelled => (
            "native directory picker was cancelled or abandoned by its controller",
            true,
        ),
        HelperOperationError::Deadline => {
            ("native directory picker exceeded its request budget", true)
        }
        HelperOperationError::Shutdown => (
            "native directory picker shut down during the request",
            false,
        ),
        HelperOperationError::TaskLost => ("native directory picker owner task was lost", false),
        HelperOperationError::GenerationExhausted => (
            "native directory picker generation capacity is exhausted",
            false,
        ),
        HelperOperationError::SpawnFailed => {
            ("native directory picker helper could not start", true)
        }
        HelperOperationError::InvalidRequest => {
            ("native directory picker request was invalid", false)
        }
        HelperOperationError::WriteFailed => ("native directory picker request pipe failed", true),
        HelperOperationError::ReadFailed => ("native directory picker response pipe failed", true),
        HelperOperationError::MalformedFrame => (
            "native directory picker returned a malformed response",
            false,
        ),
        HelperOperationError::TruncatedFrame => (
            "native directory picker returned a truncated response",
            false,
        ),
        HelperOperationError::TrailingOutput => {
            ("native directory picker returned trailing output", false)
        }
        HelperOperationError::StaleGeneration => {
            ("native directory picker returned a stale response", false)
        }
        HelperOperationError::OversizedOutput => {
            ("native directory picker response exceeded its bound", false)
        }
        HelperOperationError::StderrCapExceeded => (
            "native directory picker diagnostic output exceeded its bound",
            false,
        ),
        HelperOperationError::ExitFailure => ("native directory picker helper failed", true),
        HelperOperationError::UnresolvedReapDuring { .. }
        | HelperOperationError::ReapUnresolved => (
            "native directory picker helper cleanup could not confirm reaping",
            false,
        ),
    };
    typed_failure(ErrorCode::Internal, detail, retryable, request_id)
}

/// Maps a successful helper outcome that cannot be represented by the wire
/// picker outcome.
fn picker_outcome_failure(
    outcome: &DirectoryPickOutcome,
    request_id: &RequestId,
) -> ProtocolFailure {
    let (code, detail, retryable) = match outcome {
        DirectoryPickOutcome::InvalidPath => (
            ErrorCode::Internal,
            "native directory picker returned an invalid path",
            false,
        ),
        DirectoryPickOutcome::UnsupportedEncoding => (
            ErrorCode::Internal,
            "native directory picker returned unsupported path encoding",
            false,
        ),
        DirectoryPickOutcome::UnsupportedPlatform => (
            ErrorCode::UnsupportedFeature,
            "native directory picking is unsupported on this platform",
            false,
        ),
        DirectoryPickOutcome::DialogFailed => (
            ErrorCode::Internal,
            "native directory picker dialog failed",
            true,
        ),
        DirectoryPickOutcome::Cancelled | DirectoryPickOutcome::Selected { .. } => {
            unreachable!("user cancellation and selection are handled before failure mapping")
        }
    };
    typed_failure(code, detail, retryable, request_id)
}

/// Maps authority admission while keeping selection payloads private.
fn directory_selection_failure(
    error: DirectorySelectionAdmissionError,
    request_id: &RequestId,
) -> ProtocolFailure {
    let (detail, retryable) = match error {
        DirectorySelectionAdmissionError::IdentityAlreadyIssued => (
            "native directory selection identity was already issued",
            false,
        ),
        DirectorySelectionAdmissionError::LiveCapacityFull => {
            ("native directory selection capacity is full", true)
        }
        DirectorySelectionAdmissionError::LifetimeExhausted => (
            "native directory selection lifetime capacity is exhausted",
            false,
        ),
        DirectorySelectionAdmissionError::DeadlineOverflow => {
            ("native directory selection deadline is invalid", false)
        }
        DirectorySelectionAdmissionError::DisplayName(_) => {
            ("native directory selection display name is invalid", false)
        }
        DirectorySelectionAdmissionError::UnnamedRootForm => {
            ("native directory selection root is unnamed", false)
        }
    };
    typed_failure(ErrorCode::Internal, detail, retryable, request_id)
}

/// Classifies one rich-link resolver failure into the stable protocol
/// vocabulary without formatting URLs, HTML, or transport payloads.
fn rich_link_failure(
    error: crate::rich_link_service::RichLinkError,
    request_id: &RequestId,
) -> ProtocolFailure {
    use crate::rich_link_service::RichLinkError as Failure;

    let (code, detail_text, retryable) = match error {
        Failure::InvalidUrl => (
            ErrorCode::InvalidInput,
            "rich link url is not an absolute HTTP(S) destination",
            false,
        ),
        Failure::BlockedAddress => (
            ErrorCode::InvalidInput,
            "rich link address is not publicly routable",
            false,
        ),
        Failure::UnsupportedContentType => (
            ErrorCode::InvalidInput,
            "rich link target is not an HTML page",
            false,
        ),
        Failure::HttpStatus => (
            ErrorCode::Internal,
            "rich link target returned an unexpected HTTP status",
            true,
        ),
        Failure::ResponseTooLarge => (
            ErrorCode::Internal,
            "rich link target exceeded its HTML byte bound",
            false,
        ),
        Failure::Timeout => (ErrorCode::Internal, "rich link resolution timed out", true),
        Failure::Transport => (ErrorCode::Internal, "rich link transport failed", true),
        Failure::Configuration => (
            ErrorCode::Internal,
            "rich link resolution is misconfigured",
            false,
        ),
        Failure::Unavailable => (
            ErrorCode::Internal,
            "rich link resolution owner is unavailable",
            true,
        ),
    };
    typed_failure(code, detail_text, retryable, request_id)
}

/// Classifies one project-repository service failure into the stable protocol
/// vocabulary without formatting catalog, path, or database detail.
fn project_repository_failure(
    error: crate::project_repository_service::ProjectRepositoryServiceError,
    request_id: &RequestId,
) -> ProtocolFailure {
    use crate::project_repository_service::ProjectRepositoryServiceError as Failure;

    let (code, detail_text, retryable) = match error {
        Failure::CatalogUnavailable => (
            ErrorCode::Internal,
            "project repository catalog is unavailable",
            true,
        ),
    };
    typed_failure(code, detail_text, retryable, request_id)
}

#[cfg(test)]
mod rich_link_failure_tests {
    use super::{ErrorCode, RequestId, rich_link_failure};

    #[test]
    fn rich_link_failures_are_bounded_and_classified() {
        let request_id = RequestId::parse("rich-link-request").expect("request id is valid");
        let invalid = rich_link_failure(
            crate::rich_link_service::RichLinkError::InvalidUrl,
            &request_id,
        );
        assert_eq!(invalid.code, ErrorCode::InvalidInput);
        assert!(!invalid.retryable);
        assert_eq!(invalid.request_id.as_ref(), Some(&request_id));

        let timeout = rich_link_failure(
            crate::rich_link_service::RichLinkError::Timeout,
            &request_id,
        );
        assert_eq!(timeout.code, ErrorCode::Internal);
        assert!(timeout.retryable);

        let oversized = rich_link_failure(
            crate::rich_link_service::RichLinkError::ResponseTooLarge,
            &request_id,
        );
        assert!(!oversized.retryable);
        assert!(format!("{invalid:?}").len() < 256);
    }
}

#[cfg(test)]
mod project_repository_failure_tests {
    use super::{ErrorCode, RequestId, project_repository_failure};
    use crate::project_repository_service::ProjectRepositoryServiceError;

    #[test]
    fn project_repository_catalog_failures_are_bounded_and_retryable() {
        let request_id = RequestId::parse("repository-request").expect("request id is valid");
        let failure = project_repository_failure(
            ProjectRepositoryServiceError::CatalogUnavailable,
            &request_id,
        );
        assert_eq!(failure.code, ErrorCode::Internal);
        assert!(failure.retryable);
        assert_eq!(failure.request_id.as_ref(), Some(&request_id));
        assert!(failure.detail.as_str().contains("catalog"));
        assert!(format!("{failure:?}").len() < 256);
    }
}
