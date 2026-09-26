//! Envelope body and request/response dispatch codec.
//!
//! Owns `WireEnvelopeBody` encode/decode plus the request and response union
//! dispatch. Family payload codecs live in the sibling modules; this module
//! routes each arm and enforces envelope correlation at the body boundary.

#[allow(clippy::wildcard_imports)]
use super::*;

pub(crate) fn encode_body(
    mut root: capnp_envelope::Builder<'_>,
    body: &WireEnvelopeBody,
) -> Result<(), ProtocolEncodeError> {
    match body {
        WireEnvelopeBody::Hello(value) => {
            let mut hello = root.reborrow().init_body().init_hello();
            let mut versions = hello.reborrow().init_supported_versions(list_length(
                "hello.supportedVersions",
                value.supported_versions.versions().len(),
            )?);
            for (index, version) in value.supported_versions.versions().iter().enumerate() {
                versions.set(list_index("hello.supportedVersions", index)?, version.get());
            }
            match &value.credential {
                HelloCredential::Initial(capability) => {
                    hello
                        .reborrow()
                        .init_credential()
                        .set_initial(capability.expose_for_wire());
                }
                HelloCredential::Reconnect(capability) => {
                    hello
                        .reborrow()
                        .init_credential()
                        .set_reconnect(capability.expose_for_wire());
                }
            }
            hello.set_supports_lifecycle_control(value.supports_lifecycle_control);
        }
        WireEnvelopeBody::Welcome(value) => {
            let mut welcome = root.reborrow().init_body().init_welcome();
            welcome.set_negotiated_version(value.negotiated_version.get());
            welcome.set_connection_id(value.connection_id.as_str());
            welcome.set_reconnect_capability(value.reconnect_capability.expose_for_wire());
            welcome.set_lifecycle_control_supported(value.lifecycle_control_supported);
        }
        WireEnvelopeBody::Request(value) => {
            encode_request(root.reborrow().init_body().init_request(), value)?;
        }
        WireEnvelopeBody::Response(value) => {
            encode_response(root.reborrow().init_body().init_response(), value)?;
        }
        WireEnvelopeBody::Event(value) => {
            encode_event(root.reborrow().init_body().init_event(), value)?;
        }
        WireEnvelopeBody::ProtocolError(value) => {
            encode_protocol_error(root.reborrow().init_body().init_protocol_error(), value);
        }
        WireEnvelopeBody::PatchBatch(value) => {
            encode_patch_batch(root.reborrow().init_body().init_patch_batch(), value)?;
        }
    }
    Ok(())
}

#[expect(
    clippy::too_many_lines,
    reason = "central client-request union dispatcher; complex arms delegate to per-family codecs"
)]
pub(crate) fn encode_request(
    mut builder: artisan_capnp::request::Builder<'_>,
    value: &ClientRequest,
) -> Result<(), ProtocolEncodeError> {
    match value {
        ClientRequest::Query(Query::ListDirectories(query)) => {
            let mut scope = builder.reborrow().init_list_directories().init_scope();
            if let Some(parent) = &query.parent {
                scope.set_parent(parent.as_str());
            } else {
                scope.set_no_parent(());
            }
        }
        ClientRequest::Query(Query::ListProjectThreads(query)) => {
            builder
                .reborrow()
                .init_list_project_threads()
                .set_project_id(query.project_id.as_str());
        }
        ClientRequest::Query(Query::ListAttachedProjects(_)) => {
            builder.reborrow().init_list_attached_projects();
        }
        ClientRequest::Command(Command::AttachProject(command)) => {
            builder
                .reborrow()
                .init_attach_project()
                .set_directory_id(command.directory_id.as_str());
        }
        ClientRequest::Command(Command::CreateThread(command)) => {
            let mut create = builder.reborrow().init_create_project_thread();
            create.set_project_id(command.project_id.as_str());
            create.set_title(command.title.as_str());
        }
        ClientRequest::Command(Command::QueueFirstMessage(command)) => {
            let mut queue = builder.reborrow().init_queue_first_message();
            queue.set_thread_id(command.thread_id.as_str());
            queue.set_body(command.body.as_str());
        }
        ClientRequest::Command(Command::QueueMessage(command)) => {
            encode_queue_message(builder.reborrow().init_queue_message(), command)?;
        }
        ClientRequest::Command(Command::StopRun(command)) => {
            let mut stop = builder.reborrow().init_stop_run();
            stop.set_thread_id(command.thread_id().as_str());
            stop.set_run_id(command.run_id().as_str());
        }
        ClientRequest::Command(Command::RespondApproval(command)) => {
            let mut respond = builder.reborrow().init_respond_approval();
            respond.set_thread_id(command.thread_id().as_str());
            respond.set_run_id(command.run_id().as_str());
            respond.set_approval_id(command.approval_id().as_str());
            respond.set_approved(command.approved());
        }
        ClientRequest::Command(Command::RespondQuestion(command)) => {
            let mut respond = builder.reborrow().init_respond_question();
            respond.set_thread_id(command.thread_id().as_str());
            respond.set_run_id(command.run_id().as_str());
            respond.set_question_id(command.question_id().as_str());
            let mut answers = respond.reborrow().init_answers(list_length(
                "request.respondQuestion.answers",
                command.answers().len(),
            )?);
            for (index, answer) in command.answers().iter().enumerate() {
                answers.set(
                    list_index("request.respondQuestion.answers", index)?,
                    answer.as_str(),
                );
            }
        }
        ClientRequest::Command(Command::SetModelFavorite(command)) => {
            let mut favorite = builder.reborrow().init_set_model_favorite();
            favorite.set_thread_id(command.thread_id().as_str());
            favorite.set_profile_id(command.profile_id().as_str());
            favorite.set_catalog_revision(command.catalog_revision().as_str());
            favorite.set_model_id(command.model_id().as_str());
            favorite.set_favorite(command.favorite());
        }
        ClientRequest::Command(Command::SetThreadEngineConfig(command)) => {
            encode_set_thread_engine_config(builder.reborrow(), command.as_ref());
        }
        ClientRequest::Conversation(ConversationRequest::Query(query)) => {
            encode_conversation_query_request(builder, query);
        }
        ClientRequest::Conversation(ConversationRequest::Subscribe(subscribe)) => {
            let mut encoded = builder.reborrow().init_conversation_subscribe();
            encoded.set_thread_id(subscribe.thread_id.as_str());
            let mut start = encoded.init_start();
            if let Some(after) = subscribe.after {
                start.set_resume_after(after.get());
            } else {
                start.set_fresh(());
            }
        }
        ClientRequest::Conversation(ConversationRequest::Unsubscribe(unsubscribe)) => {
            builder
                .reborrow()
                .init_conversation_unsubscribe()
                .set_thread_id(unsubscribe.thread_id.as_str());
        }
        ClientRequest::ValidateDirectory(path) => {
            builder.reborrow().set_validate_directory(path.as_str());
        }
        ClientRequest::PickDirectory => {
            builder.reborrow().set_pick_directory(());
        }
        ClientRequest::Lifecycle(LifecycleRequest::Status) => {
            builder.reborrow().init_lifecycle_control().init_status();
        }
        ClientRequest::Lifecycle(LifecycleRequest::Stop { require_idle }) => {
            builder
                .reborrow()
                .init_lifecycle_control()
                .init_stop()
                .set_require_idle(*require_idle);
        }
        ClientRequest::Query(Query::ReadThreadEngineSettings(query)) => {
            builder
                .reborrow()
                .init_read_thread_engine_settings()
                .set_thread_id(query.thread_id().as_str());
        }
        ClientRequest::Query(Query::ListRegisteredEngineProfiles(_)) => {
            builder.reborrow().init_list_registered_engine_profiles();
        }
        ClientRequest::Query(Query::ReadMessageImage(query)) => {
            let mut encoded = builder.reborrow().init_read_message_image();
            encoded.set_thread_id(query.thread_id().as_str());
            encoded.set_message_id(query.message_id().as_str());
            encoded.set_index(query.index());
        }
        ClientRequest::Query(Query::ReadActiveRun(query)) => {
            builder
                .reborrow()
                .init_read_active_run()
                .set_thread_id(query.thread_id().as_str());
        }
        ClientRequest::Query(Query::ReadComposerCatalog(query)) => {
            let mut catalog = builder.reborrow().init_read_composer_catalog();
            catalog.set_thread_id(query.thread_id().as_str());
            catalog.set_profile_id(query.profile_id().as_str());
        }
        ClientRequest::Query(Query::ReadModelFavorites(_)) => {
            builder.reborrow().set_read_model_favorites(());
        }

        ClientRequest::Query(Query::ReadAccountUsage(query)) => {
            let mut encoded = builder.reborrow().init_read_account_usage();
            match query.engine_id() {
                Some(engine_id) => encoded.reborrow().init_scope().set_one(engine_id),
                None => encoded.reborrow().init_scope().set_all(()),
            }
            encoded.set_force(query.force());
        }

        ClientRequest::Query(Query::ListQueuedMessages(query)) => {
            crate::composer_state_codec::encode_list_queued_messages_request(
                builder.reborrow().init_list_queued_messages(),
                query,
            )
            .map_err(|_| ProtocolEncodeError::ComposerState)?;
        }
        ClientRequest::Command(Command::WithdrawQueuedMessage(_))
        | ClientRequest::Query(Query::ReadRecalledMessage(_) | Query::ReadRunUsage(_)) => {
            encode_queue_state_request(builder, value);
        }
        ClientRequest::Query(Query::ListFailedMessages(query)) => {
            crate::composer_state_codec::encode_list_failed_messages_request(
                builder.reborrow().init_list_failed_messages(),
                query,
            )
            .map_err(|_| ProtocolEncodeError::ComposerState)?;
        }
        ClientRequest::Command(
            Command::SaveComposerDraft(_)
            | Command::UploadComposerAttachment(_)
            | Command::QueueStoredMessage(_),
        )
        | ClientRequest::Query(Query::ReadComposerDraft(_) | Query::ReadComposerAttachment(_)) => {
            encode_composer_draft_request(builder, value)?;
        }
        ClientRequest::Command(
            Command::RetryFailedMessage(_)
            | Command::RecoverFailedMessage(_)
            | Command::SubmitComposerDraft(_),
        ) => {
            encode_message_submission_request(builder, value)?;
        }
        ClientRequest::Query(
            Query::ReadHostCatalog(_)
            | Query::ResolveModelSelection(_)
            | Query::ResolveEngineConfiguration(_),
        ) => {
            encode_forge_decision_request(builder, value)?;
        }
        ClientRequest::Query(Query::ReadUserPreferences(_))
        | ClientRequest::Command(
            Command::RecordNavigation(_) | Command::ImportLegacyPreferences(_),
        ) => encode_user_preferences_request(builder, value)?,
        ClientRequest::Query(Query::ReadRecentThreads(_)) => {
            encode_recent_threads_request(builder);
        }
        ClientRequest::ResolveRichLink(request) => {
            builder
                .reborrow()
                .init_resolve_rich_link()
                .set_url(request.url());
        }
        ClientRequest::QueryProjectRepository(query) => {
            encode_project_repository_query(
                builder.reborrow().init_query_project_repository(),
                query,
            )?;
        }
    }
    Ok(())
}

pub(crate) fn encode_response(
    mut builder: artisan_capnp::response::Builder<'_>,
    value: &ServerResponse,
) -> Result<(), ProtocolEncodeError> {
    builder.set_request_id(value.request_id.as_str());
    encode_response_payload(builder, &value.payload, &value.request_id)
}

#[expect(
    clippy::too_many_lines,
    reason = "central server-response union dispatcher; arms delegate to per-family codecs"
)]
pub(crate) fn encode_response_payload(
    mut builder: artisan_capnp::response::Builder<'_>,
    payload: &ResponsePayload,
    outer_request_id: &RequestId,
) -> Result<(), ProtocolEncodeError> {
    match payload {
        ResponsePayload::QueuedMessages(value) => {
            crate::composer_state_codec::encode_queued_message_listing(
                builder.reborrow().init_queued_messages(),
                value,
            )
            .map_err(|_| ProtocolEncodeError::ComposerState)?;
        }
        ResponsePayload::MessageWithdrawn(value) => {
            crate::composer_state_codec::encode_queued_message_withdrawal_result(
                builder.reborrow().init_message_withdrawn(),
                outer_request_id,
                value,
            )
            .map_err(|_| ProtocolEncodeError::ComposerState)?;
        }
        ResponsePayload::RecalledMessage(value) => {
            crate::composer_state_codec::encode_recalled_message_result(
                builder.reborrow().init_recalled_message(),
                value,
            )
            .map_err(|_| ProtocolEncodeError::ComposerState)?;
        }
        ResponsePayload::RunUsage(value) => crate::composer_state_codec::encode_run_usage_result(
            builder.reborrow().init_run_usage(),
            value,
        )
        .map_err(|_| ProtocolEncodeError::ComposerState)?,
        ResponsePayload::FailedMessages(value) => {
            crate::composer_state_codec::encode_failed_message_listing(
                builder.reborrow().init_failed_messages(),
                value,
            )
            .map_err(|_| ProtocolEncodeError::ComposerState)?;
        }
        ResponsePayload::ComposerDraftSaved(_)
        | ResponsePayload::ComposerDraft(_)
        | ResponsePayload::ComposerAttachmentUploaded(_)
        | ResponsePayload::ComposerAttachment(_) => {
            encode_composer_draft_response(builder, payload, outer_request_id)?;
        }
        ResponsePayload::FailedMessageRetried(_)
        | ResponsePayload::FailedMessageRecovered(_)
        | ResponsePayload::ComposerDraftSubmitted(_) => {
            encode_message_submission_response(builder, payload, outer_request_id)?;
        }
        ResponsePayload::HostCatalog(_)
        | ResponsePayload::ModelSelectionResolved(_)
        | ResponsePayload::EngineConfigurationResolved(_) => {
            encode_forge_decision_response(builder, payload)?;
        }
        ResponsePayload::UserPreferences(_) | ResponsePayload::LegacyPreferencesImported(_) => {
            encode_user_preferences_response(builder, payload)?;
        }
        ResponsePayload::RecentThreads(listing) => {
            encode_recent_threads(builder.reborrow().init_recent_threads(), listing)?;
        }
        ResponsePayload::AccountUsage(snapshot) => {
            encode_engine_usage_snapshot(builder.reborrow().init_account_usage(), snapshot)?;
        }
        ResponsePayload::DirectoryListing(listing) => {
            encode_directory_listing(builder.reborrow().init_directory_list(), listing)?;
        }
        ResponsePayload::ProjectListing(listing) => {
            encode_project_listing_response(builder.reborrow(), listing)?;
        }
        ResponsePayload::AttachedProject {
            project,
            disposition,
        } => {
            let mut result = builder.reborrow().init_attached_project();
            encode_project(result.reborrow().init_project(), project);
            result.set_disposition(encode_disposition(*disposition));
        }
        ResponsePayload::ThreadListing(listing) => {
            let mut threads = builder
                .reborrow()
                .init_thread_list()
                .init_threads(list_length(
                    "response.threadList.threads",
                    listing.threads().len(),
                )?);
            for (index, thread) in listing.threads().iter().enumerate() {
                encode_thread(
                    threads
                        .reborrow()
                        .get(list_index("response.threadList.threads", index)?),
                    thread,
                );
            }
        }
        ResponsePayload::CreatedThread {
            thread,
            disposition,
        } => {
            let mut result = builder.reborrow().init_created_thread();
            encode_thread(result.reborrow().init_thread(), thread);
            result.set_disposition(encode_disposition(*disposition));
        }
        ResponsePayload::FirstMessageQueued(receipt) => {
            let mut result = builder.reborrow().init_queued_receipt();
            result.set_request_id(receipt.request_id.as_str());
            result.set_message_id(receipt.message_id.as_str());
            result.set_thread_id(receipt.thread_id.as_str());
            result.set_disposition(encode_disposition(receipt.disposition));
            result.set_state(artisan_capnp::QueuedState::Queued);
        }
        ResponsePayload::MessageQueued(receipt) => {
            let mut result = builder.reborrow().init_queued_message_receipt();
            result.set_request_id(receipt.request_id.as_str());
            result.set_message_id(receipt.message_id.as_str());
            result.set_thread_id(receipt.thread_id.as_str());
            result.set_disposition(encode_disposition(receipt.disposition));
            result.set_state(artisan_capnp::QueuedState::Queued);
        }
        ResponsePayload::MessageImage(result) => {
            let mut encoded = builder.reborrow().init_message_image();
            encode_image_attachment_ref(encoded.reborrow().init_reference(), &result.reference);
            encoded.set_bytes(&result.bytes);
        }
        ResponsePayload::RunStopped(receipt) => {
            let mut encoded = builder.reborrow().init_stop_run_receipt();
            encoded.set_request_id(receipt.request_id.as_str());
            encoded.set_thread_id(receipt.thread_id.as_str());
            encoded.set_run_id(receipt.run_id.as_str());
            encoded.set_disposition(encode_stop_run_disposition(receipt.disposition));
        }
        ResponsePayload::ApprovalResponse(receipt) => {
            encode_respond_approval_receipt(builder.reborrow().init_approval_response(), receipt);
        }
        ResponsePayload::QuestionResponse(receipt) => {
            encode_respond_question_receipt(builder.reborrow().init_question_response(), receipt)?;
        }
        ResponsePayload::ActiveRun(result) => {
            let mut encoded = builder.reborrow().init_active_run();
            match result {
                ActiveRunResult::NoActive { thread_id } => {
                    encoded.set_thread_id(thread_id.as_str());
                    encoded.init_state().set_no_active(());
                }
                ActiveRunResult::Active {
                    thread_id,
                    run_id,
                    status,
                    engine_id,
                } => {
                    encoded.set_thread_id(thread_id.as_str());
                    encoded.reborrow().init_state().set_active(run_id.as_str());
                    encoded.set_run_status(encode_run_status(*status));
                    encoded.set_run_engine_id(engine_id.as_str());
                }
            }
        }
        ResponsePayload::ComposerCatalog(result) => {
            result.validate_scope()?;
            let mut encoded = builder.reborrow().init_composer_catalog();
            encoded.set_thread_id(result.thread_id.as_str());
            encoded.set_profile_id(result.profile_id.as_str());
            encoded.set_snapshot_data(result.snapshot.as_bytes());
        }
        ResponsePayload::ModelFavorites(snapshot) => {
            encode_model_favorites_snapshot(builder.reborrow().init_model_favorites(), snapshot)?;
        }
        ResponsePayload::ModelFavoriteSet(receipt) => {
            let mut encoded = builder.reborrow().init_model_favorite_set();
            encoded.set_request_id(receipt.request_id.as_str());
            encoded.set_model_id(receipt.model_id.as_str());
            encoded.set_favorite(receipt.favorite);
            encoded.set_disposition(encode_disposition(receipt.disposition));
            encode_model_favorites_snapshot(encoded.init_snapshot(), &receipt.snapshot)?;
        }
        ResponsePayload::ConversationSnapshot(snapshot) => {
            encode_conversation_snapshot(
                builder.reborrow().init_conversation_snapshot(),
                snapshot,
            )?;
        }
        ResponsePayload::ConversationSubscriptionStarted(started) => {
            let encoded = builder.reborrow().init_conversation_subscription_started();
            match started {
                ConversationSubscriptionStarted::Fresh(start) => {
                    encode_conversation_snapshot(encoded.init_fresh(), start.snapshot())?;
                }
                ConversationSubscriptionStarted::Resumed { thread_id, cursor } => {
                    let mut point = encoded.init_resumed();
                    point.set_thread_id(thread_id.as_str());
                    point.set_cursor(cursor.get());
                }
            }
        }
        ResponsePayload::ConversationSubscriptionStopped(stopped) => {
            builder
                .reborrow()
                .init_conversation_subscription_stopped()
                .set_thread_id(stopped.thread_id.as_str());
        }
        ResponsePayload::DirectoryPicked(outcome) => {
            encode_directory_picked(builder.reborrow().init_directory_picked(), outcome);
        }
        ResponsePayload::Lifecycle(value) => {
            encode_lifecycle_response(builder.reborrow().init_lifecycle_control(), value)?;
        }
        ResponsePayload::ThreadEngineConfigSet(result) => {
            encode_thread_engine_config_result(builder.reborrow(), result);
        }
        ResponsePayload::ThreadEngineSettings(result) => {
            encode_thread_engine_settings_result(builder.reborrow(), result);
        }
        ResponsePayload::RegisteredEngineProfiles(result) => {
            encode_registered_engine_profiles_result(
                builder.reborrow().init_registered_engine_profiles(),
                result,
            )?;
        }
        ResponsePayload::RichLink(result) => {
            encode_rich_link_page_metadata(builder.reborrow().init_rich_link(), result)?;
        }
        ResponsePayload::ProjectRepository(result) => {
            encode_project_repository_query_result(
                builder.reborrow().init_project_repository(),
                result,
            )?;
        }
    }
    Ok(())
}

pub(crate) fn decode_body(
    root: capnp_envelope::Reader<'_>,
    frame_id: &FrameId,
) -> Result<WireEnvelopeBody, ProtocolDecodeError> {
    match root.get_body().which()? {
        capnp_envelope::body::Which::Hello(value) => {
            Ok(WireEnvelopeBody::Hello(decode_hello(value?)?))
        }
        capnp_envelope::body::Which::Welcome(value) => {
            Ok(WireEnvelopeBody::Welcome(decode_welcome(value?)?))
        }
        capnp_envelope::body::Which::Request(value) => {
            Ok(WireEnvelopeBody::Request(decode_request(value?, frame_id)?))
        }
        capnp_envelope::body::Which::Response(value) => {
            Ok(WireEnvelopeBody::Response(decode_response(value?)?))
        }
        capnp_envelope::body::Which::Event(value) => {
            Ok(WireEnvelopeBody::Event(decode_event(value?)?))
        }
        capnp_envelope::body::Which::ProtocolError(value) => Ok(WireEnvelopeBody::ProtocolError(
            decode_protocol_error(value?)?,
        )),
        capnp_envelope::body::Which::PatchBatch(value) => {
            Ok(WireEnvelopeBody::PatchBatch(decode_patch_batch(value?)?))
        }
    }
}

pub(crate) fn decode_hello(
    value: artisan_capnp::hello::Reader<'_>,
) -> Result<Hello, ProtocolDecodeError> {
    let versions = value.get_supported_versions()?;
    let version_count = versions.len() as usize;
    if version_count > crate::HELLO_VERSION_MAX_ENTRIES {
        return Err(VersionOfferError::TooMany {
            count: version_count,
            maximum: crate::HELLO_VERSION_MAX_ENTRIES,
        }
        .into());
    }
    let supported_versions = VersionOffer::new(versions.iter().collect())?;
    let credential = match value.get_credential().which()? {
        artisan_capnp::hello::credential::Which::Initial(capability) => {
            HelloCredential::Initial(LocalCapability::try_from_slice(capability?)?)
        }
        artisan_capnp::hello::credential::Which::Reconnect(capability) => {
            HelloCredential::Reconnect(ReconnectCapability::try_from_slice(capability?)?)
        }
    };
    Ok(Hello {
        supported_versions,
        credential,
        supports_lifecycle_control: value.get_supports_lifecycle_control(),
    })
}

pub(crate) fn decode_welcome(
    value: artisan_capnp::welcome::Reader<'_>,
) -> Result<Welcome, ProtocolDecodeError> {
    Ok(Welcome {
        negotiated_version: ProtocolVersion::new(value.get_negotiated_version())?,
        connection_id: ConnectionId::parse(read_text(
            value.get_connection_id(),
            "welcome.connectionId",
        )?)?,
        reconnect_capability: ReconnectCapability::try_from_slice(
            value.get_reconnect_capability()?,
        )?,
        lifecycle_control_supported: value.get_lifecycle_control_supported(),
    })
}

#[expect(
    clippy::too_many_lines,
    reason = "central client-request union dispatcher mirroring `encode_request`"
)]
pub(crate) fn decode_request(
    value: artisan_capnp::request::Reader<'_>,
    frame_id: &FrameId,
) -> Result<ClientRequest, ProtocolDecodeError> {
    let request_id =
        frame_id
            .to_request_id()
            .map_err(|source| ProtocolDecodeError::Identifier {
                field: "envelope.messageId",
                source,
            })?;
    match value.which()? {
        request::Which::ListDirectories(query) => {
            let parent = match query?.get_scope().which()? {
                list_directories_request::scope::Which::NoParent(()) => None,
                list_directories_request::scope::Which::Parent(value) => Some(parse_directory_id(
                    read_text(value, "request.listDirectories.parent")?,
                    "request.listDirectories.parent",
                )?),
            };
            Ok(ClientRequest::Query(Query::ListDirectories(
                ListDirectories { parent },
            )))
        }
        request::Which::AttachProject(command) => {
            let command = command?;
            Ok(ClientRequest::Command(Command::AttachProject(
                AttachProject {
                    request_id,
                    directory_id: parse_directory_id(
                        read_text(
                            command.get_directory_id(),
                            "request.attachProject.directoryId",
                        )?,
                        "request.attachProject.directoryId",
                    )?,
                },
            )))
        }
        request::Which::ListProjectThreads(query) => {
            let query = query?;
            Ok(ClientRequest::Query(Query::ListProjectThreads(
                ListProjectThreads {
                    project_id: parse_project_id(
                        read_text(
                            query.get_project_id(),
                            "request.listProjectThreads.projectId",
                        )?,
                        "request.listProjectThreads.projectId",
                    )?,
                },
            )))
        }
        request::Which::ListAttachedProjects(query) => {
            query?;
            Ok(ClientRequest::Query(Query::ListAttachedProjects(
                ListAttachedProjects,
            )))
        }
        request::Which::CreateProjectThread(command) => {
            let command = command?;
            Ok(ClientRequest::Command(Command::CreateThread(
                CreateThread {
                    request_id,
                    project_id: parse_project_id(
                        read_text(
                            command.get_project_id(),
                            "request.createProjectThread.projectId",
                        )?,
                        "request.createProjectThread.projectId",
                    )?,
                    title: ThreadTitle::parse(read_text(
                        command.get_title(),
                        "request.createProjectThread.title",
                    )?)
                    .map_err(|source| ProtocolDecodeError::ThreadTitle { source })?,
                },
            )))
        }
        request::Which::QueueFirstMessage(command) => {
            decode_queue_first_message(command?, request_id)
        }
        request::Which::QueueMessage(command) => decode_queue_message(command?, request_id),
        request::Which::StopRun(command) => decode_stop_run(command?, request_id),
        request::Which::RespondApproval(command) => decode_respond_approval(command?, request_id),
        request::Which::RespondQuestion(command) => decode_respond_question(command?, request_id),
        request::Which::SetThreadEngineConfig(command) => {
            decode_set_thread_engine_config(command?, request_id)
        }
        request::Which::ConversationQuery(query) => decode_conversation_query_request(query?),
        request::Which::ConversationSubscribe(subscribe) => {
            decode_conversation_subscribe_request(subscribe?)
        }
        request::Which::ConversationUnsubscribe(unsubscribe) => {
            decode_conversation_unsubscribe_request(unsubscribe?)
        }
        request::Which::PickDirectory(()) => Ok(ClientRequest::PickDirectory),
        request::Which::ValidateDirectory(path) => Ok(ClientRequest::ValidateDirectory(
            artisan_domain::RootPath::parse(read_text(path, "request.validateDirectory")?)?,
        )),
        request::Which::LifecycleControl(lifecycle) => decode_lifecycle_request(lifecycle?),
        request::Which::ReadThreadEngineSettings(query) => {
            decode_read_thread_engine_settings(query?)
        }
        request::Which::ListRegisteredEngineProfiles(query) => {
            query?;
            Ok(ClientRequest::Query(Query::ListRegisteredEngineProfiles(
                artisan_domain::commands::ListRegisteredEngineProfiles,
            )))
        }
        request::Which::ReadMessageImage(query) => decode_read_message_image_request(query?),
        request::Which::ReadActiveRun(query) => {
            let query = query?;
            Ok(ClientRequest::Query(Query::ReadActiveRun(
                ReadActiveRun::new(parse_thread_id(
                    read_text(query.get_thread_id(), "request.readActiveRun.threadId")?,
                    "request.readActiveRun.threadId",
                )?),
            )))
        }
        request::Which::ReadComposerCatalog(query) => decode_read_composer_catalog_request(query?),
        request::Which::ListQueuedMessages(value) => {
            Ok(ClientRequest::Query(Query::ListQueuedMessages(
                crate::composer_state_codec::decode_list_queued_messages_request(value?)?,
            )))
        }
        request::Which::WithdrawQueuedMessage(value) => {
            Ok(ClientRequest::Command(Command::WithdrawQueuedMessage(
                crate::composer_state_codec::decode_withdraw_queued_message_request(
                    value?, request_id,
                )?,
            )))
        }
        request::Which::ReadRecalledMessage(value) => {
            Ok(ClientRequest::Query(Query::ReadRecalledMessage(
                crate::composer_state_codec::decode_read_recalled_message_request(value?)?,
            )))
        }
        request::Which::ReadRunUsage(value) => Ok(ClientRequest::Query(Query::ReadRunUsage(
            crate::composer_state_codec::decode_read_run_usage_request(value?)?,
        ))),
        request::Which::ListFailedMessages(value) => {
            Ok(ClientRequest::Query(Query::ListFailedMessages(
                crate::composer_state_codec::decode_list_failed_messages_request(value?)?,
            )))
        }
        request::Which::ReadAccountUsage(value) => decode_read_account_usage(value?),
        request::Which::ResolveRichLink(query) => {
            let query = query?;
            Ok(ClientRequest::ResolveRichLink(ResolveRichLinkRequest::new(
                read_text(query.get_url(), "request.resolveRichLink.url")?,
            )?))
        }
        request::Which::QueryProjectRepository(query) => Ok(ClientRequest::QueryProjectRepository(
            decode_project_repository_query(query?)?,
        )),
        request::Which::ReadModelFavorites(()) => Ok(ClientRequest::Query(
            Query::ReadModelFavorites(ReadModelFavorites),
        )),
        request::Which::SetModelFavorite(command) => {
            decode_set_model_favorite(command?, request_id)
        }
        request::Which::SaveComposerDraft(_)
        | request::Which::ReadComposerDraft(_)
        | request::Which::UploadComposerAttachment(_)
        | request::Which::ReadComposerAttachment(_)
        | request::Which::QueueStoredMessage(_) => decode_composer_draft_request(value, request_id),
        request::Which::RetryFailedMessage(_)
        | request::Which::RecoverFailedMessage(_)
        | request::Which::SubmitComposerDraft(_) => {
            decode_message_submission_request(value, request_id)
        }
        request::Which::ReadHostCatalog(())
        | request::Which::ResolveModelSelection(_)
        | request::Which::ResolveEngineConfiguration(_) => decode_forge_decision_request(value),
        request::Which::ReadUserPreferences(())
        | request::Which::RecordNavigation(_)
        | request::Which::ImportLegacyPreferences(_) => {
            decode_user_preferences_request(value, &request_id)
        }
        request::Which::ReadRecentThreads(()) => Ok(decode_recent_threads_request()),
    }
}

#[expect(
    clippy::too_many_lines,
    reason = "central server-response union dispatcher mirroring `encode_response_payload`"
)]
pub(crate) fn decode_response(
    value: artisan_capnp::response::Reader<'_>,
) -> Result<ServerResponse, ProtocolDecodeError> {
    let request_id = parse_request_id(
        read_text(value.get_request_id(), "response.requestId")?,
        "response.requestId",
    )?;
    let payload = match value.which()? {
        response::Which::DirectoryList(listing) => {
            ResponsePayload::DirectoryListing(decode_directory_listing(listing?)?)
        }
        response::Which::ProjectList(listing) => {
            ResponsePayload::ProjectListing(decode_project_list(listing?)?)
        }
        response::Which::AttachedProject(result) => {
            let result = result?;
            ResponsePayload::AttachedProject {
                project: decode_project(result.get_project()?)?,
                disposition: decode_disposition(result.get_disposition()?),
            }
        }
        response::Which::ThreadList(listing) => {
            let threads = listing?.get_threads()?;
            let thread_count = threads.len() as usize;
            if thread_count > THREAD_LISTING_MAX_THREADS {
                return Err(ProtocolDecodeError::ThreadListing {
                    source: ThreadListingError::TooManyThreads {
                        count: thread_count,
                        maximum: THREAD_LISTING_MAX_THREADS,
                    },
                });
            }
            let decoded = threads
                .iter()
                .map(decode_thread)
                .collect::<Result<Vec<_>, _>>()?;
            ResponsePayload::ThreadListing(
                ThreadListing::new(decoded)
                    .map_err(|source| ProtocolDecodeError::ThreadListing { source })?,
            )
        }
        response::Which::CreatedThread(result) => {
            let result = result?;
            ResponsePayload::CreatedThread {
                thread: decode_thread(result.get_thread()?)?,
                disposition: decode_disposition(result.get_disposition()?),
            }
        }
        response::Which::QueuedReceipt(receipt) => decode_queued_receipt(receipt?, &request_id)?,
        response::Which::QueuedMessageReceipt(receipt) => {
            decode_queue_message_receipt(receipt?, &request_id)?
        }
        response::Which::MessageImage(result) => decode_message_image(result?)?,
        response::Which::StopRunReceipt(receipt) => decode_stop_run_receipt(receipt?, &request_id)?,
        response::Which::ApprovalResponse(receipt) => {
            decode_respond_approval_receipt(receipt?, &request_id)?
        }
        response::Which::QuestionResponse(receipt) => {
            decode_respond_question_receipt(receipt?, &request_id)?
        }
        response::Which::ActiveRun(result) => decode_active_run_result(result?)?,
        response::Which::QueuedMessages(value) => ResponsePayload::QueuedMessages(
            crate::composer_state_codec::decode_queued_message_listing(value?)?,
        ),
        response::Which::MessageWithdrawn(value) => ResponsePayload::MessageWithdrawn(
            crate::composer_state_codec::decode_queued_message_withdrawal_result(
                value?,
                &request_id,
            )?,
        ),
        response::Which::RecalledMessage(value) => ResponsePayload::RecalledMessage(
            crate::composer_state_codec::decode_recalled_message_result(value?)?,
        ),
        response::Which::RunUsage(value) => ResponsePayload::RunUsage(
            crate::composer_state_codec::decode_run_usage_result(value?)?,
        ),
        response::Which::FailedMessages(value) => ResponsePayload::FailedMessages(
            crate::composer_state_codec::decode_failed_message_listing(value?)?,
        ),
        response::Which::AccountUsage(value) => decode_engine_usage_snapshot(value?)?,
        response::Which::ComposerCatalog(result) => decode_composer_catalog(result?)?,
        response::Which::ModelFavorites(snapshot) => ResponsePayload::ModelFavorites(
            decode_model_favorites_snapshot(snapshot?, "response.modelFavorites.modelIds")?,
        ),
        response::Which::ModelFavoriteSet(receipt) => {
            decode_model_favorite_set(receipt?, &request_id)?
        }
        response::Which::ConversationSnapshot(snapshot) => {
            ResponsePayload::ConversationSnapshot(decode_conversation_snapshot(snapshot?)?)
        }
        response::Which::ConversationSubscriptionStarted(started) => {
            ResponsePayload::ConversationSubscriptionStarted(
                decode_conversation_subscription_started(started?)?,
            )
        }
        response::Which::ConversationSubscriptionStopped(stopped) => {
            ResponsePayload::ConversationSubscriptionStopped(
                decode_conversation_subscription_stopped(stopped?)?,
            )
        }
        response::Which::DirectoryPicked(picked) => decode_directory_picked(picked?)?,
        response::Which::LifecycleControl(lifecycle) => {
            ResponsePayload::Lifecycle(decode_lifecycle_response(lifecycle?)?)
        }
        response::Which::ThreadEngineConfigSet(result) => {
            decode_thread_engine_config_set(result?, &request_id)?
        }
        response::Which::ThreadEngineSettings(result) => {
            decode_thread_engine_settings_result(result?)?
        }
        response::Which::RegisteredEngineProfiles(result) => {
            decode_registered_engine_profiles_result(result?)?
        }
        response::Which::RichLink(result) => decode_rich_link_page_metadata(result?)?,
        response::Which::ComposerDraftSaved(_)
        | response::Which::ComposerDraft(_)
        | response::Which::ComposerAttachmentUploaded(_)
        | response::Which::ComposerAttachment(_) => {
            decode_composer_draft_response(value, &request_id)?
        }
        response::Which::FailedMessageRetried(_)
        | response::Which::FailedMessageRecovered(_)
        | response::Which::ComposerDraftSubmitted(_) => {
            decode_message_submission_response(value, &request_id)?
        }
        response::Which::HostCatalog(_)
        | response::Which::ModelSelectionResolved(_)
        | response::Which::EngineConfigurationResolved(_) => decode_forge_decision_response(value)?,
        response::Which::UserPreferences(_) | response::Which::LegacyPreferencesImported(_) => {
            decode_user_preferences_response(value)?
        }
        response::Which::ProjectRepository(result) => {
            decode_project_repository_query_result(result?)?
        }
        response::Which::RecentThreads(listing) => {
            ResponsePayload::RecentThreads(decode_recent_threads(listing?)?)
        }
    };
    Ok(ServerResponse {
        request_id,
        payload,
    })
}
