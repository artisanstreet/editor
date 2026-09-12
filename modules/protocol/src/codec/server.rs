//! Server-originated payload and shared wire-scalar codec.
//!
//! Owns engine-usage and model-favorites response encode, project/thread
//! and directory listing codec, lifecycle and run dispositions, protocol
//! errors, engine-observation events, and rich-link metadata decode.

#[allow(clippy::wildcard_imports)]
use super::*;

pub(crate) fn encode_model_favorites_snapshot(
    mut builder: artisan_capnp::model_favorites_snapshot::Builder<'_>,
    snapshot: &ModelFavoritesSnapshot,
) -> Result<(), ProtocolEncodeError> {
    ModelFavoritesSnapshot::new(snapshot.revision, snapshot.model_ids.clone())
        .map_err(|source| ProtocolEncodeError::ModelFavoritesSnapshot { source })?;
    builder.set_revision(snapshot.revision.get());
    let mut model_ids = builder.init_model_ids(list_length(
        "modelFavoritesSnapshot.modelIds",
        snapshot.model_ids.len(),
    )?);
    for (index, model_id) in snapshot.model_ids.iter().enumerate() {
        model_ids.set(
            list_index("modelFavoritesSnapshot.modelIds", index)?,
            model_id.as_str(),
        );
    }
    Ok(())
}

pub(crate) fn encode_engine_usage_window_kind(
    kind: EngineUsageWindowKind,
) -> artisan_capnp::EngineUsageWindowKind {
    match kind {
        EngineUsageWindowKind::Session => artisan_capnp::EngineUsageWindowKind::Session,
        EngineUsageWindowKind::Weekly => artisan_capnp::EngineUsageWindowKind::Weekly,
        EngineUsageWindowKind::Monthly => artisan_capnp::EngineUsageWindowKind::Monthly,
        EngineUsageWindowKind::Unknown => artisan_capnp::EngineUsageWindowKind::Unknown,
    }
}

pub(crate) fn encode_engine_usage_authentication(
    state: EngineUsageAuthentication,
) -> artisan_capnp::EngineUsageAuthentication {
    match state {
        EngineUsageAuthentication::Authenticated => {
            artisan_capnp::EngineUsageAuthentication::Authenticated
        }
        EngineUsageAuthentication::Unauthenticated => {
            artisan_capnp::EngineUsageAuthentication::Unauthenticated
        }
        EngineUsageAuthentication::Unknown => artisan_capnp::EngineUsageAuthentication::Unknown,
    }
}

pub(crate) fn encode_quota_surface(surface: QuotaSurface) -> artisan_capnp::QuotaSurface {
    match surface {
        QuotaSurface::Supported => artisan_capnp::QuotaSurface::Supported,
        QuotaSurface::Unknown => artisan_capnp::QuotaSurface::Unknown,
        QuotaSurface::Unsupported => artisan_capnp::QuotaSurface::Unsupported,
    }
}

pub(crate) fn encode_engine_usage_window(
    mut builder: artisan_capnp::engine_usage_window::Builder<'_>,
    window: &EngineUsageWindow,
) {
    builder.set_id(window.id());
    builder.set_kind(encode_engine_usage_window_kind(window.kind()));
    builder.set_label(window.label().unwrap_or(""));
    builder.set_percent_used(window.percent_used());
    builder.set_resets_at(window.resets_at().unwrap_or(""));
    builder.set_window_minutes(window.window_minutes().unwrap_or(0));
}

pub(crate) fn encode_engine_usage_report(
    mut builder: artisan_capnp::engine_usage_report::Builder<'_>,
    report: &EngineUsageReport,
) -> Result<(), ProtocolEncodeError> {
    builder.set_engine_id(report.engine_id());
    builder.set_display_name(report.display_name());
    builder.set_authentication(encode_engine_usage_authentication(
        report.authentication().state(),
    ));
    builder.set_auth_reason(report.authentication().reason().unwrap_or(""));
    builder.set_account_email(report.account_email().unwrap_or(""));
    match report.quota_surface() {
        None => builder.reborrow().init_quota_surface().set_absent(()),
        Some(surface) => builder
            .reborrow()
            .init_quota_surface()
            .set_present(encode_quota_surface(surface)),
    }
    builder.set_failure(report.failure().unwrap_or(""));
    let mut windows = builder.init_windows(list_length(
        "response.accountUsage.windows",
        report.windows().len(),
    )?);
    for (index, window) in report.windows().iter().enumerate() {
        encode_engine_usage_window(
            windows
                .reborrow()
                .get(list_index("response.accountUsage.windows", index)?),
            window,
        );
    }
    Ok(())
}

pub(crate) fn encode_engine_usage_snapshot(
    mut builder: artisan_capnp::engine_usage_snapshot::Builder<'_>,
    snapshot: &EngineUsageSnapshot,
) -> Result<(), ProtocolEncodeError> {
    builder.set_fetched_at(snapshot.fetched_at());
    let mut engines = builder.init_engines(list_length(
        "response.accountUsage.engines",
        snapshot.engines().len(),
    )?);
    for (index, engine) in snapshot.engines().iter().enumerate() {
        encode_engine_usage_report(
            engines
                .reborrow()
                .get(list_index("response.accountUsage.engines", index)?),
            engine,
        )?;
    }
    Ok(())
}

pub(crate) fn encode_project_listing_response(
    builder: artisan_capnp::response::Builder<'_>,
    listing: &ProjectListing,
) -> Result<(), ProtocolEncodeError> {
    let mut projects = builder.init_project_list().init_projects(list_length(
        "response.projectList.projects",
        listing.projects().len(),
    )?);
    for (index, project) in listing.projects().iter().enumerate() {
        encode_project(
            projects
                .reborrow()
                .get(list_index("response.projectList.projects", index)?),
            project,
        );
    }
    Ok(())
}

pub(crate) fn encode_lifecycle_response(
    mut builder: artisan_capnp::lifecycle_response::Builder<'_>,
    value: &LifecycleResponse,
) -> Result<(), ProtocolEncodeError> {
    match value {
        LifecycleResponse::Status(status) => {
            status.validate()?;
            let mut encoded = builder.reborrow().init_status();
            encoded.set_state(encode_lifecycle_state(status.state));
            encoded.set_active_work_count(status.active_work_count);
        }
        LifecycleResponse::Stop(receipt) => {
            let mut encoded = builder.reborrow().init_stop();
            encoded.set_disposition(encode_lifecycle_stop_disposition(receipt.disposition));
            encoded.set_state(encode_lifecycle_state(receipt.state));
        }
    }
    Ok(())
}

pub(crate) fn encode_directory_picked(
    mut builder: artisan_capnp::directory_pick_outcome::Builder<'_>,
    outcome: &DirectoryPickOutcome,
) {
    match outcome {
        DirectoryPickOutcome::Selected(directory_id) => {
            builder.set_selected(directory_id.as_str());
        }
        DirectoryPickOutcome::Cancelled => {
            builder.set_cancelled(());
        }
    }
}

pub(crate) fn encode_event(
    mut builder: artisan_capnp::event::Builder<'_>,
    value: &ServerEvent,
) -> Result<(), ProtocolEncodeError> {
    builder.set_cursor(value.cursor.get());
    match &value.event {
        Event::ProjectAttached(event) => {
            encode_project(builder.reborrow().init_project_attached(), &event.project);
        }
        Event::ThreadCreated(event) => {
            encode_thread(builder.reborrow().init_thread_created(), &event.thread);
        }
        Event::FirstMessageQueued(event) => {
            let mut queued = builder.reborrow().init_first_message_queued();
            queued.set_request_id(event.message.request_id.as_str());
            queued.set_message_id(event.message.message_id.as_str());
            queued.set_thread_id(event.message.thread_id.as_str());
            queued.set_body(event.message.body.as_str());
        }
        Event::EngineObservation(event) => {
            let mut observation = builder.reborrow().init_engine_observation();
            observation.set_thread_id(event.thread_id.as_str());
            encode_engine_observation(
                observation.reborrow().init_observation(),
                &event.observation,
            )?;
            match &event.attribution {
                Some(attribution) => {
                    let mut encoded = observation.reborrow().init_attribution().init_attribution();
                    encoded.set_run_id(attribution.run_id.as_str());
                    encoded.set_turn_id(attribution.turn_id.as_str());
                    encoded.set_committed_at_millis(attribution.committed_at.as_millis());
                    encoded.set_delivery_sequence(attribution.delivery_sequence);
                }
                None => {
                    observation
                        .reborrow()
                        .init_attribution()
                        .set_no_attribution(());
                }
            }
        }
    }
    Ok(())
}

pub(crate) fn encode_protocol_error(
    mut builder: artisan_capnp::protocol_error::Builder<'_>,
    value: &ProtocolFailure,
) {
    builder.set_code(encode_error_code(value.code));
    builder.set_message(value.detail.as_str());
    builder.set_retryable(value.retryable);
    if let Some(request_id) = &value.request_id {
        builder.set_correlated(request_id.as_str());
    } else {
        builder.set_uncorrelated(());
    }
}

pub(crate) fn encode_directory_listing(
    mut builder: artisan_capnp::directory_listing::Builder<'_>,
    value: &DirectoryListing,
) -> Result<(), ProtocolEncodeError> {
    let mut parent = builder.reborrow().init_parent();
    if let Some(directory_id) = value.parent() {
        parent.set_parent(directory_id.as_str());
    } else {
        parent.set_no_parent(());
    }

    let mut places = builder.reborrow().init_places(list_length(
        "directoryListing.places",
        value.places().len(),
    )?);
    for (index, place) in value.places().iter().enumerate() {
        let mut encoded = places
            .reborrow()
            .get(list_index("directoryListing.places", index)?);
        encoded.set_kind(encode_place_kind(place.kind));
        encoded.set_directory_id(place.directory_id.as_str());
        encoded.set_display_name(place.display_name.as_str());
    }

    let mut entries = builder.reborrow().init_entries(list_length(
        "directoryListing.entries",
        value.entries().len(),
    )?);
    for (index, entry) in value.entries().iter().enumerate() {
        let mut encoded = entries
            .reborrow()
            .get(list_index("directoryListing.entries", index)?);
        encoded.set_directory_id(entry.directory_id.as_str());
        encoded.set_display_name(entry.display_name.as_str());
        encoded.set_kind(encode_directory_kind(entry.kind));
        encoded.set_has_children(entry.has_children);
    }
    Ok(())
}

pub(crate) fn encode_project(
    mut builder: artisan_capnp::project::Builder<'_>,
    value: &ProjectSummary,
) {
    builder.set_project_id(value.project_id.as_str());
    builder.set_display_name(value.display_name.as_str());
    builder.set_root_path(value.root_path.as_str());
    builder.set_attached_at_millis(value.attached_at.as_millis());
}

pub(crate) fn encode_thread(
    mut builder: artisan_capnp::thread_summary::Builder<'_>,
    value: &ThreadSummary,
) {
    builder.set_has_active_work(value.has_active_work);
    builder.set_has_last_message(value.last_message_at.is_some());
    if let Some(at) = value.last_message_at {
        builder.set_last_message_at_millis(at.as_millis());
    }
    builder.set_thread_id(value.thread_id.as_str());
    builder.set_project_id(value.project_id.as_str());
    builder.set_title(value.title.as_str());
    builder.set_created_at_millis(value.created_at.as_millis());
    builder.set_updated_at_millis(value.updated_at.as_millis());
}

pub(crate) const fn encode_disposition(
    value: ReceiptDisposition,
) -> artisan_capnp::ReceiptDisposition {
    match value {
        ReceiptDisposition::Accepted => artisan_capnp::ReceiptDisposition::Accepted,
        ReceiptDisposition::Duplicate => artisan_capnp::ReceiptDisposition::Duplicate,
    }
}

pub(crate) const fn encode_stop_run_disposition(
    value: StopRunDisposition,
) -> artisan_capnp::StopRunDisposition {
    match value {
        StopRunDisposition::Requested => artisan_capnp::StopRunDisposition::Requested,
        StopRunDisposition::AlreadyRequested => artisan_capnp::StopRunDisposition::AlreadyRequested,
        StopRunDisposition::NotActive => artisan_capnp::StopRunDisposition::NotActive,
    }
}

pub(crate) const fn encode_place_kind(value: PlaceKind) -> artisan_capnp::PlaceKind {
    match value {
        PlaceKind::Home => artisan_capnp::PlaceKind::Home,
        PlaceKind::Desktop => artisan_capnp::PlaceKind::Desktop,
        PlaceKind::Documents => artisan_capnp::PlaceKind::Documents,
        PlaceKind::Downloads => artisan_capnp::PlaceKind::Downloads,
        PlaceKind::Music => artisan_capnp::PlaceKind::Music,
        PlaceKind::Pictures => artisan_capnp::PlaceKind::Pictures,
        PlaceKind::Videos => artisan_capnp::PlaceKind::Videos,
    }
}

pub(crate) const fn encode_directory_kind(
    value: DirectoryKind,
) -> artisan_capnp::DirectoryEntryKind {
    match value {
        DirectoryKind::Root => artisan_capnp::DirectoryEntryKind::Root,
        DirectoryKind::Directory => artisan_capnp::DirectoryEntryKind::Directory,
    }
}

pub(crate) const fn encode_lifecycle_state(value: LifecycleState) -> artisan_capnp::LifecycleState {
    match value {
        LifecycleState::Ready => artisan_capnp::LifecycleState::Ready,
        LifecycleState::Busy => artisan_capnp::LifecycleState::Busy,
        LifecycleState::Draining => artisan_capnp::LifecycleState::Draining,
    }
}

pub(crate) const fn encode_lifecycle_stop_disposition(
    value: LifecycleStopDisposition,
) -> artisan_capnp::LifecycleStopDisposition {
    match value {
        LifecycleStopDisposition::Accepted => artisan_capnp::LifecycleStopDisposition::Accepted,
        LifecycleStopDisposition::Duplicate => artisan_capnp::LifecycleStopDisposition::Duplicate,
        LifecycleStopDisposition::AlreadyStopping => {
            artisan_capnp::LifecycleStopDisposition::AlreadyStopping
        }
    }
}

pub(crate) const fn encode_error_code(value: ErrorCode) -> artisan_capnp::ErrorCode {
    match value {
        ErrorCode::UnsupportedVersion => artisan_capnp::ErrorCode::UnsupportedVersion,
        ErrorCode::InvalidInput => artisan_capnp::ErrorCode::InvalidInput,
        ErrorCode::DirectoryUnknown => artisan_capnp::ErrorCode::DirectoryUnknown,
        ErrorCode::ProjectUnknown => artisan_capnp::ErrorCode::ProjectUnknown,
        ErrorCode::ThreadUnknown => artisan_capnp::ErrorCode::ThreadUnknown,
        ErrorCode::Internal => artisan_capnp::ErrorCode::Internal,
        ErrorCode::IdempotencyConflict => artisan_capnp::ErrorCode::IdempotencyConflict,
        ErrorCode::UnsupportedFeature => artisan_capnp::ErrorCode::UnsupportedFeature,
        ErrorCode::LifecycleBusy => artisan_capnp::ErrorCode::LifecycleBusy,
        ErrorCode::EngineConfigConflict => artisan_capnp::ErrorCode::EngineConfigConflict,
    }
}

pub(crate) fn decode_rich_link_page_metadata(
    value: artisan_capnp::rich_link_page_metadata::Reader<'_>,
) -> Result<ResponsePayload, ProtocolDecodeError> {
    Ok(ResponsePayload::RichLink(RichLinkPageMetadata::new(
        read_text(value.get_requested_url(), "response.richLink.requestedUrl")?,
        read_text(value.get_page_name(), "response.richLink.pageName")?,
        value.get_cache_expires_at_ms(),
    )?))
}

pub(crate) fn decode_event(
    value: artisan_capnp::event::Reader<'_>,
) -> Result<ServerEvent, ProtocolDecodeError> {
    let cursor = EventCursor::new(value.get_cursor())?;
    let event = match value.which()? {
        event::Which::ProjectAttached(project) => Event::ProjectAttached(ProjectAttached {
            project: decode_project(project?)?,
        }),
        event::Which::ThreadCreated(thread) => Event::ThreadCreated(ThreadCreated {
            thread: decode_thread(thread?)?,
        }),
        event::Which::FirstMessageQueued(queued) => {
            let queued = queued?;
            Event::FirstMessageQueued(FirstMessageQueued {
                message: QueuedMessage {
                    request_id: parse_request_id(
                        read_text(
                            queued.get_request_id(),
                            "event.firstMessageQueued.requestId",
                        )?,
                        "event.firstMessageQueued.requestId",
                    )?,
                    message_id: parse_message_id(
                        read_text(
                            queued.get_message_id(),
                            "event.firstMessageQueued.messageId",
                        )?,
                        "event.firstMessageQueued.messageId",
                    )?,
                    thread_id: parse_thread_id(
                        read_text(queued.get_thread_id(), "event.firstMessageQueued.threadId")?,
                        "event.firstMessageQueued.threadId",
                    )?,
                    body: MessageBody::parse(read_text(
                        queued.get_body(),
                        "event.firstMessageQueued.body",
                    )?)
                    .map_err(|source| ProtocolDecodeError::MessageBody { source })?,
                },
            })
        }
        event::Which::EngineObservation(value) => {
            let value = value?;
            let thread_id = parse_thread_id(
                read_text(value.get_thread_id(), "event.engineObservation.threadId")?,
                "event.engineObservation.threadId",
            )?;
            let attribution = decode_engine_observation_attribution(value.get_attribution())?;
            Event::EngineObservation(EngineObservationEvent {
                thread_id,
                observation: decode_engine_observation(value.get_observation()?)?,
                attribution,
            })
        }
    };
    Ok(ServerEvent { cursor, event })
}

/// Decodes the additive optional observation attribution.
///
/// Old frames never set the union and decode as `None`, preserving the frozen
/// v1 bytes. When present, run/turn ids must parse, the commit time must be
/// positive, and the delivery sequence must be positive; any malformed or
/// nonpositive value is a typed rejection, never a silent default.
pub(crate) fn decode_engine_observation_attribution(
    value: artisan_capnp::engine_observation_event::attribution::Reader<'_>,
) -> Result<Option<EngineObservationAttribution>, ProtocolDecodeError> {
    use artisan_capnp::engine_observation_event::attribution::Which;
    match value.which()? {
        Which::NoAttribution(()) => Ok(None),
        Which::Attribution(attribution) => {
            let attribution = attribution?;
            let run_id = parse_run_id(
                read_text(
                    attribution.get_run_id(),
                    "event.engineObservation.attribution.runId",
                )?,
                "event.engineObservation.attribution.runId",
            )?;
            let turn_id = parse_turn_id(
                read_text(
                    attribution.get_turn_id(),
                    "event.engineObservation.attribution.turnId",
                )?,
                "event.engineObservation.attribution.turnId",
            )?;
            let committed_at_millis = attribution.get_committed_at_millis();
            if committed_at_millis <= 0 {
                return Err(ProtocolDecodeError::Observation {
                    source: artisan_domain::ObservationError::OutOfRange {
                        field: "committed_at",
                    },
                });
            }
            let delivery_sequence = attribution.get_delivery_sequence();
            if delivery_sequence == 0 {
                return Err(ProtocolDecodeError::Observation {
                    source: artisan_domain::ObservationError::OutOfRange {
                        field: "delivery_sequence",
                    },
                });
            }
            Ok(Some(EngineObservationAttribution {
                run_id,
                turn_id,
                committed_at: UnixMillis::from_millis(committed_at_millis),
                delivery_sequence,
            }))
        }
    }
}

pub(crate) fn decode_protocol_error(
    value: artisan_capnp::protocol_error::Reader<'_>,
) -> Result<ProtocolFailure, ProtocolDecodeError> {
    let request_id = match value.which()? {
        protocol_error::Which::Correlated(request_id) => Some(parse_request_id(
            read_text(request_id, "protocolError.correlated")?,
            "protocolError.correlated",
        )?),
        protocol_error::Which::Uncorrelated(()) => None,
    };
    Ok(ProtocolFailure {
        code: decode_error_code(value.get_code()?),
        detail: ErrorDetail::parse(read_text(value.get_message(), "protocolError.message")?)?,
        retryable: value.get_retryable(),
        request_id,
    })
}

pub(crate) fn decode_directory_listing(
    value: artisan_capnp::directory_listing::Reader<'_>,
) -> Result<DirectoryListing, ProtocolDecodeError> {
    let places = value.get_places()?;
    let places_count = places.len() as usize;
    if places_count > DIRECTORY_LISTING_MAX_PLACES {
        return Err(ProtocolDecodeError::DirectoryListing {
            source: DirectoryListingError::TooManyPlaces {
                count: places_count,
                maximum: DIRECTORY_LISTING_MAX_PLACES,
            },
        });
    }

    let entries = value.get_entries()?;
    let entries_count = entries.len() as usize;
    if entries_count > DIRECTORY_LISTING_MAX_ENTRIES {
        return Err(ProtocolDecodeError::DirectoryListing {
            source: DirectoryListingError::TooManyEntries {
                count: entries_count,
                maximum: DIRECTORY_LISTING_MAX_ENTRIES,
            },
        });
    }

    let parent = match value.get_parent().which()? {
        directory_listing::parent::Which::NoParent(()) => None,
        directory_listing::parent::Which::Parent(parent) => Some(parse_directory_id(
            read_text(parent, "directoryListing.parent")?,
            "directoryListing.parent",
        )?),
    };

    let places = places
        .iter()
        .map(|place| {
            Ok(DirectoryPlace {
                kind: decode_place_kind(place.get_kind()?),
                directory_id: parse_directory_id(
                    read_text(
                        place.get_directory_id(),
                        "directoryListing.places.directoryId",
                    )?,
                    "directoryListing.places.directoryId",
                )?,
                display_name: DisplayName::parse(read_text(
                    place.get_display_name(),
                    "directoryListing.places.displayName",
                )?)
                .map_err(|source| ProtocolDecodeError::DisplayName {
                    field: "directoryListing.places.displayName",
                    source,
                })?,
            })
        })
        .collect::<Result<Vec<_>, ProtocolDecodeError>>()?;

    let entries = entries
        .iter()
        .map(|entry| {
            Ok(DirectoryEntry {
                directory_id: parse_directory_id(
                    read_text(
                        entry.get_directory_id(),
                        "directoryListing.entries.directoryId",
                    )?,
                    "directoryListing.entries.directoryId",
                )?,
                display_name: DisplayName::parse(read_text(
                    entry.get_display_name(),
                    "directoryListing.entries.displayName",
                )?)
                .map_err(|source| ProtocolDecodeError::DisplayName {
                    field: "directoryListing.entries.displayName",
                    source,
                })?,
                kind: decode_directory_kind(entry.get_kind()?),
                has_children: entry.get_has_children(),
            })
        })
        .collect::<Result<Vec<_>, ProtocolDecodeError>>()?;

    DirectoryListing::new(places, entries, parent)
        .map_err(|source| ProtocolDecodeError::DirectoryListing { source })
}

pub(crate) fn decode_project(
    value: artisan_capnp::project::Reader<'_>,
) -> Result<ProjectSummary, ProtocolDecodeError> {
    Ok(ProjectSummary {
        project_id: parse_project_id(
            read_text(value.get_project_id(), "project.projectId")?,
            "project.projectId",
        )?,
        display_name: DisplayName::parse(read_text(
            value.get_display_name(),
            "project.displayName",
        )?)
        .map_err(|source| ProtocolDecodeError::DisplayName {
            field: "project.displayName",
            source,
        })?,
        root_path: RootPath::parse(read_text(value.get_root_path(), "project.rootPath")?)
            .map_err(|source| ProtocolDecodeError::RootPath { source })?,
        attached_at: UnixMillis::from_millis(value.get_attached_at_millis()),
    })
}

pub(crate) fn decode_thread(
    value: artisan_capnp::thread_summary::Reader<'_>,
) -> Result<ThreadSummary, ProtocolDecodeError> {
    Ok(ThreadSummary {
        has_active_work: value.get_has_active_work(),
        last_message_at: value
            .get_has_last_message()
            .then(|| UnixMillis::from_millis(value.get_last_message_at_millis())),
        thread_id: parse_thread_id(
            read_text(value.get_thread_id(), "thread.threadId")?,
            "thread.threadId",
        )?,
        project_id: parse_project_id(
            read_text(value.get_project_id(), "thread.projectId")?,
            "thread.projectId",
        )?,
        title: ThreadTitle::parse(read_text(value.get_title(), "thread.title")?)
            .map_err(|source| ProtocolDecodeError::ThreadTitle { source })?,
        created_at: UnixMillis::from_millis(value.get_created_at_millis()),
        updated_at: UnixMillis::from_millis(value.get_updated_at_millis()),
    })
}

pub(crate) const fn encode_run_status(status: RunLiveStatus) -> artisan_capnp::RunStatus {
    match status {
        RunLiveStatus::Queued => artisan_capnp::RunStatus::Queued,
        RunLiveStatus::Running => artisan_capnp::RunStatus::Running,
        RunLiveStatus::Waiting => artisan_capnp::RunStatus::Waiting,
    }
}

/// Strict run-status decode: `unknown` is a typed failure, never a
/// tolerated state. The current backend always emits a live status with
/// its engine; native QUIC is a same-version build.
pub(crate) fn decode_run_status(
    value: artisan_capnp::RunStatus,
) -> Result<RunLiveStatus, ProtocolDecodeError> {
    match value {
        artisan_capnp::RunStatus::Queued => Ok(RunLiveStatus::Queued),
        artisan_capnp::RunStatus::Running => Ok(RunLiveStatus::Running),
        artisan_capnp::RunStatus::Waiting => Ok(RunLiveStatus::Waiting),
        artisan_capnp::RunStatus::Unknown => {
            Err(ProtocolDecodeError::UnknownDiscriminant { value: 0 })
        }
    }
}

pub(crate) const fn decode_disposition(
    value: artisan_capnp::ReceiptDisposition,
) -> ReceiptDisposition {
    match value {
        artisan_capnp::ReceiptDisposition::Accepted => ReceiptDisposition::Accepted,
        artisan_capnp::ReceiptDisposition::Duplicate => ReceiptDisposition::Duplicate,
    }
}

pub(crate) const fn decode_stop_run_disposition(
    value: artisan_capnp::StopRunDisposition,
) -> StopRunDisposition {
    match value {
        artisan_capnp::StopRunDisposition::Requested => StopRunDisposition::Requested,
        artisan_capnp::StopRunDisposition::AlreadyRequested => StopRunDisposition::AlreadyRequested,
        artisan_capnp::StopRunDisposition::NotActive => StopRunDisposition::NotActive,
    }
}

pub(crate) const fn decode_place_kind(value: artisan_capnp::PlaceKind) -> PlaceKind {
    match value {
        artisan_capnp::PlaceKind::Home => PlaceKind::Home,
        artisan_capnp::PlaceKind::Desktop => PlaceKind::Desktop,
        artisan_capnp::PlaceKind::Documents => PlaceKind::Documents,
        artisan_capnp::PlaceKind::Downloads => PlaceKind::Downloads,
        artisan_capnp::PlaceKind::Music => PlaceKind::Music,
        artisan_capnp::PlaceKind::Pictures => PlaceKind::Pictures,
        artisan_capnp::PlaceKind::Videos => PlaceKind::Videos,
    }
}

pub(crate) const fn decode_directory_kind(
    value: artisan_capnp::DirectoryEntryKind,
) -> DirectoryKind {
    match value {
        artisan_capnp::DirectoryEntryKind::Root => DirectoryKind::Root,
        artisan_capnp::DirectoryEntryKind::Directory => DirectoryKind::Directory,
    }
}

pub(crate) const fn decode_lifecycle_state(value: artisan_capnp::LifecycleState) -> LifecycleState {
    match value {
        artisan_capnp::LifecycleState::Ready => LifecycleState::Ready,
        artisan_capnp::LifecycleState::Busy => LifecycleState::Busy,
        artisan_capnp::LifecycleState::Draining => LifecycleState::Draining,
    }
}

pub(crate) const fn decode_lifecycle_stop_disposition(
    value: artisan_capnp::LifecycleStopDisposition,
) -> LifecycleStopDisposition {
    match value {
        artisan_capnp::LifecycleStopDisposition::Accepted => LifecycleStopDisposition::Accepted,
        artisan_capnp::LifecycleStopDisposition::Duplicate => LifecycleStopDisposition::Duplicate,
        artisan_capnp::LifecycleStopDisposition::AlreadyStopping => {
            LifecycleStopDisposition::AlreadyStopping
        }
    }
}

pub(crate) const fn decode_error_code(value: artisan_capnp::ErrorCode) -> ErrorCode {
    match value {
        artisan_capnp::ErrorCode::UnsupportedVersion => ErrorCode::UnsupportedVersion,
        artisan_capnp::ErrorCode::InvalidInput => ErrorCode::InvalidInput,
        artisan_capnp::ErrorCode::DirectoryUnknown => ErrorCode::DirectoryUnknown,
        artisan_capnp::ErrorCode::ProjectUnknown => ErrorCode::ProjectUnknown,
        artisan_capnp::ErrorCode::ThreadUnknown => ErrorCode::ThreadUnknown,
        artisan_capnp::ErrorCode::Internal => ErrorCode::Internal,
        artisan_capnp::ErrorCode::IdempotencyConflict => ErrorCode::IdempotencyConflict,
        artisan_capnp::ErrorCode::UnsupportedFeature => ErrorCode::UnsupportedFeature,
        artisan_capnp::ErrorCode::LifecycleBusy => ErrorCode::LifecycleBusy,
        artisan_capnp::ErrorCode::EngineConfigConflict => ErrorCode::EngineConfigConflict,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{ProtocolVersion, ResolveRichLinkRequest, RichLinkPageMetadata};

    fn envelope(body: WireEnvelopeBody) -> WireEnvelope {
        WireEnvelope {
            protocol_version: ProtocolVersion::V1,
            frame_id: FrameId::parse("frame-1").expect("frame id is valid"),
            sent_at: UnixMillis::from_millis(1_700_000_000_000),
            body,
        }
    }

    #[test]
    fn resolve_rich_link_request_round_trips() {
        let request = ResolveRichLinkRequest::new("https://example.com/docs?q=1#frag")
            .expect("request is valid");
        let wire = envelope(WireEnvelopeBody::Request(ClientRequest::ResolveRichLink(
            request,
        )));
        let encoded = encode_envelope(&wire).expect("request encodes");
        let decoded = decode_envelope(&encoded).expect("request decodes");
        assert!(decoded.body == wire.body);
    }

    #[test]
    fn rich_link_response_round_trips() {
        let metadata = RichLinkPageMetadata::new(
            "https://example.com/docs",
            "Example — Docs",
            1_700_000_060_000,
        )
        .expect("metadata is valid");
        let wire = envelope(WireEnvelopeBody::Response(ServerResponse {
            request_id: RequestId::parse("frame-1").expect("request id is valid"),
            payload: ResponsePayload::RichLink(metadata),
        }));
        let encoded = encode_envelope(&wire).expect("response encodes");
        let decoded = decode_envelope(&encoded).expect("response decodes");
        assert!(decoded.body == wire.body);
    }

    #[test]
    fn decode_rejects_non_http_rich_link_url() {
        let mut message = Builder::new(HeapAllocator::new());
        {
            let mut root = message.init_root::<artisan_capnp::envelope::Builder>();
            root.set_protocol_version(1);
            root.set_message_id("frame-1");
            root.set_sent_at_millis(0);
            root.reborrow()
                .init_body()
                .init_request()
                .init_resolve_rich_link()
                .set_url("file:///etc/passwd");
        }
        let bytes = serialize::write_message_to_words(&message);
        assert!(matches!(
            decode_envelope(&bytes),
            Err(ProtocolDecodeError::ProtocolValue { .. })
        ));
    }
}
