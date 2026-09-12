//! Strict decode helpers, checkpoint decoding, and bind/engine validation.

use super::encode::encode_bytes;
#[allow(clippy::wildcard_imports)]
use super::*;

/// Decodes one persisted observation checkpoint and proves canonicality.
///
/// The caller supplies the stored checkpoint version and blob (for example
/// from the `run_checkpoints` row written by [`Repository::commit_run_batch`])
/// and receives the engine tag, binding version, and validated observations.
/// Unknown engines, tags, and provider values reject with typed errors.
///
/// # Errors
///
/// Returns [`ObservationCommitError`] for version, format, engine, sequence,
/// bound, shape, and canonicality violations.
pub fn decode_observation_checkpoint(
    version: i64,
    bytes: &[u8],
) -> Result<DecodedObservationBatch, ObservationCommitError> {
    if version != OBSERVATION_CHECKPOINT_VERSION {
        return Err(ObservationCommitError::VersionMismatch);
    }
    let value: Value =
        serde_json::from_slice(bytes).map_err(|_| ObservationCommitError::Malformed)?;
    let envelope = value.as_object().ok_or(ObservationCommitError::Malformed)?;
    require_keys(
        envelope,
        &[
            "format",
            "version",
            "engine",
            "binding_version",
            "observations",
        ],
    )?;
    if get_str(envelope, "format")? != OBSERVATION_FORMAT_TAG {
        return Err(ObservationCommitError::FormatMismatch);
    }
    if get_i64(envelope, "version")? != OBSERVATION_CHECKPOINT_VERSION {
        return Err(ObservationCommitError::VersionMismatch);
    }
    let engine = EngineId::parse(get_str(envelope, "engine")?)
        .map_err(|_| ObservationCommitError::UnknownEngine)?;
    let binding_version = get_i64(envelope, "binding_version")?;
    if binding_version <= 0 {
        return Err(ObservationCommitError::InvalidObservation(
            ObservationError::OutOfRange {
                field: "binding_version",
            },
        ));
    }
    let raw = envelope
        .get("observations")
        .and_then(Value::as_array)
        .ok_or(ObservationCommitError::Malformed)?;
    if raw.is_empty() {
        return Err(ObservationCommitError::EmptyBatch);
    }
    if raw.len() > OBSERVATION_BATCH_MAX_OBSERVATIONS {
        return Err(ObservationCommitError::TooMany {
            count: raw.len(),
            maximum: OBSERVATION_BATCH_MAX_OBSERVATIONS,
        });
    }
    let mut observations = Vec::with_capacity(raw.len());
    for entry in raw {
        let object = entry.as_object().ok_or(ObservationCommitError::Malformed)?;
        observations.push(decode_observation(object)?);
    }
    let canonical = encode_bytes(engine, binding_version, &observations)?;
    if canonical.as_slice() != bytes {
        return Err(ObservationCommitError::NonCanonical);
    }
    Ok(DecodedObservationBatch {
        engine,
        binding_version,
        observations,
    })
}

/// Requires an observation batch to agree with its run bind.
///
/// The binding version must equal the bound receipt's version: observations
/// commit only under the bind they were produced for, never across a rebind.
///
/// # Errors
///
/// Returns [`ObservationCommitError::BindMismatch`] on any version divergence.
pub fn validate_observation_bind(
    binding_version: i64,
    bound: &BoundRunReceipt,
) -> Result<(), ObservationCommitError> {
    if binding_version != bound.binding_version {
        return Err(ObservationCommitError::BindMismatch);
    }
    Ok(())
}

/// Requires a decoded batch to continue the committed engine history.
///
/// The engine tag of a newly decoded batch must equal the expected engine so
/// a run's durable observation history never mixes engine vocabularies.
///
/// # Errors
///
/// Returns [`ObservationCommitError::EngineMismatch`] on any tag divergence.
pub fn validate_observation_engine(
    engine: EngineId,
    batch: &DecodedObservationBatch,
) -> Result<(), ObservationCommitError> {
    if batch.engine != engine {
        return Err(ObservationCommitError::EngineMismatch);
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Strict decoding (exact key sets per tag, typed provider values)
// ---------------------------------------------------------------------------

fn require_keys(
    object: &Map<String, Value>,
    expected: &[&str],
) -> Result<(), ObservationCommitError> {
    if object.len() != expected.len() || expected.iter().any(|key| !object.contains_key(*key)) {
        return Err(ObservationCommitError::Malformed);
    }
    Ok(())
}

fn get_str<'a>(
    object: &'a Map<String, Value>,
    key: &str,
) -> Result<&'a str, ObservationCommitError> {
    object
        .get(key)
        .and_then(Value::as_str)
        .ok_or(ObservationCommitError::Malformed)
}

fn get_opt_str<'a>(
    object: &'a Map<String, Value>,
    key: &str,
) -> Result<Option<&'a str>, ObservationCommitError> {
    match object.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(value) => value
            .as_str()
            .ok_or(ObservationCommitError::Malformed)
            .map(Some),
    }
}

fn get_bool(object: &Map<String, Value>, key: &str) -> Result<bool, ObservationCommitError> {
    object
        .get(key)
        .and_then(Value::as_bool)
        .ok_or(ObservationCommitError::Malformed)
}

fn get_i64(object: &Map<String, Value>, key: &str) -> Result<i64, ObservationCommitError> {
    object
        .get(key)
        .and_then(Value::as_i64)
        .ok_or(ObservationCommitError::Malformed)
}

fn get_u64(object: &Map<String, Value>, key: &str) -> Result<u64, ObservationCommitError> {
    object
        .get(key)
        .and_then(Value::as_u64)
        .ok_or(ObservationCommitError::Malformed)
}

fn get_opt_u64(
    object: &Map<String, Value>,
    key: &str,
) -> Result<Option<u64>, ObservationCommitError> {
    match object.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(value) => value
            .as_u64()
            .ok_or(ObservationCommitError::Malformed)
            .map(Some),
    }
}

fn get_opt_i32(
    object: &Map<String, Value>,
    key: &str,
) -> Result<Option<i32>, ObservationCommitError> {
    match object.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(value) => {
            let raw = value.as_i64().ok_or(ObservationCommitError::Malformed)?;
            i32::try_from(raw)
                .map(Some)
                .map_err(|_| ObservationCommitError::Malformed)
        }
    }
}

fn get_opt_f64(
    object: &Map<String, Value>,
    key: &str,
) -> Result<Option<f64>, ObservationCommitError> {
    match object.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(value) => value
            .as_f64()
            .ok_or(ObservationCommitError::Malformed)
            .map(Some),
    }
}

fn get_opt_object<'a>(
    object: &'a Map<String, Value>,
    key: &str,
) -> Result<Option<&'a Map<String, Value>>, ObservationCommitError> {
    match object.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(value) => value
            .as_object()
            .ok_or(ObservationCommitError::Malformed)
            .map(Some),
    }
}

fn get_opt_string_array(
    object: &Map<String, Value>,
    key: &str,
) -> Result<Option<Vec<String>>, ObservationCommitError> {
    match object.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(value) => {
            let raw = value.as_array().ok_or(ObservationCommitError::Malformed)?;
            let mut out = Vec::with_capacity(raw.len());
            for entry in raw {
                out.push(
                    entry
                        .as_str()
                        .ok_or(ObservationCommitError::Malformed)?
                        .to_owned(),
                );
            }
            Ok(Some(out))
        }
    }
}

fn get_opt_id(
    object: &Map<String, Value>,
    key: &str,
) -> Result<Option<ObservationId>, ObservationCommitError> {
    match get_opt_str(object, key)? {
        None => Ok(None),
        Some(text) => ObservationId::parse(text.to_owned())
            .map(Some)
            .map_err(ObservationError::Identifier)
            .map_err(ObservationCommitError::InvalidObservation),
    }
}

fn header(
    object: &Map<String, Value>,
) -> Result<(ObservationId, ObservationSequence), ObservationCommitError> {
    let id = ObservationId::parse(get_str(object, "id")?.to_owned())
        .map_err(ObservationError::Identifier)?;
    let sequence = ObservationSequence::new(get_u64(object, "sequence")?)?;
    Ok((id, sequence))
}

fn decode_error_ref(object: &Map<String, Value>) -> Result<EngineErrorRef, ObservationCommitError> {
    require_keys(
        object,
        &[
            "artisan_code",
            "provider_code",
            "detail",
            "affected_model_id",
            "limit_id",
            "limit_label",
            "limit_scope",
            "resets_at",
        ],
    )?;
    let artisan_code = ArtisanCode::parse(get_str(object, "artisan_code")?.to_owned())?;
    let limit_scope = match get_opt_str(object, "limit_scope")? {
        None => None,
        Some(scope) => Some(LimitScope::parse(scope)?),
    };
    EngineErrorRef::new(EngineErrorRefInput {
        artisan_code,
        provider_code: get_opt_str(object, "provider_code")?.map(str::to_owned),
        detail: get_opt_str(object, "detail")?.map(str::to_owned),
        affected_model_id: get_opt_str(object, "affected_model_id")?.map(str::to_owned),
        limit_id: get_opt_str(object, "limit_id")?.map(str::to_owned),
        limit_label: get_opt_str(object, "limit_label")?.map(str::to_owned),
        limit_scope,
        resets_at: get_opt_str(object, "resets_at")?.map(str::to_owned),
    })
    .map_err(ObservationCommitError::InvalidObservation)
}

fn decode_opt_error_ref(
    object: &Map<String, Value>,
) -> Result<Option<EngineErrorRef>, ObservationCommitError> {
    match get_opt_object(object, "error_ref")? {
        None => Ok(None),
        Some(nested) => decode_error_ref(nested).map(Some),
    }
}

fn decode_approval_request(
    object: &Map<String, Value>,
) -> Result<ApprovalRequest, ObservationCommitError> {
    require_keys(object, &["kind", "command", "cwd", "reason"])?;
    let kind = ApprovalKind::parse(get_str(object, "kind")?)?;
    match kind {
        ApprovalKind::Command => ApprovalRequest::command(
            get_str(object, "command")?.to_owned(),
            get_opt_str(object, "cwd")?.map(str::to_owned),
            get_opt_str(object, "reason")?.map(str::to_owned),
        ),
        ApprovalKind::FileChange => {
            if get_opt_str(object, "command")?.is_some() || get_opt_str(object, "cwd")?.is_some() {
                return Err(ObservationCommitError::Malformed);
            }
            ApprovalRequest::file_change(get_opt_str(object, "reason")?.map(str::to_owned))
        }
        ApprovalKind::Action => {
            if get_opt_str(object, "command")?.is_some() || get_opt_str(object, "cwd")?.is_some() {
                return Err(ObservationCommitError::Malformed);
            }
            ApprovalRequest::action(get_opt_str(object, "reason")?.map(str::to_owned))
        }
    }
    .map_err(ObservationCommitError::InvalidObservation)
}

fn decode_question_options(
    object: &Map<String, Value>,
) -> Result<Option<Vec<QuestionOption>>, ObservationCommitError> {
    match object.get("options") {
        None | Some(Value::Null) => Ok(None),
        Some(value) => {
            let raw = value.as_array().ok_or(ObservationCommitError::Malformed)?;
            let mut options = Vec::with_capacity(raw.len());
            for entry in raw {
                let option = entry.as_object().ok_or(ObservationCommitError::Malformed)?;
                require_keys(option, &["label", "description"])?;
                options.push(
                    QuestionOption::new(
                        get_str(option, "label")?.to_owned(),
                        get_opt_str(option, "description")?.map(str::to_owned),
                    )
                    .map_err(ObservationCommitError::InvalidObservation)?,
                );
            }
            Ok(Some(options))
        }
    }
}

fn decode_plan_entries(
    object: &Map<String, Value>,
) -> Result<Vec<PlanEntry>, ObservationCommitError> {
    let raw = object
        .get("entries")
        .and_then(Value::as_array)
        .ok_or(ObservationCommitError::Malformed)?;
    let mut entries = Vec::with_capacity(raw.len());
    for entry in raw {
        let item = entry.as_object().ok_or(ObservationCommitError::Malformed)?;
        require_keys(item, &["id", "status", "text"])?;
        entries.push(
            PlanEntry::new(
                ObservationId::parse(get_str(item, "id")?.to_owned())
                    .map_err(ObservationError::Identifier)
                    .map_err(ObservationCommitError::InvalidObservation)?,
                PlanEntryStatus::parse(get_str(item, "status")?)
                    .map_err(ObservationCommitError::InvalidObservation)?,
                get_str(item, "text")?.to_owned(),
            )
            .map_err(ObservationCommitError::InvalidObservation)?,
        );
    }
    Ok(entries)
}

#[expect(
    clippy::too_many_lines,
    reason = "one linear match arm per wire tag documents the wire-to-domain table; extraction \
              would hide that mapping behind jumps"
)]
fn decode_transcript_content(
    object: &Map<String, Value>,
) -> Result<TranscriptContent, ObservationCommitError> {
    let tag = get_str(object, "tag")?;
    match tag {
        "agent_message_delta" => {
            require_keys(object, &["tag", "item_id", "phase", "delta"])?;
            TranscriptAgentMessageDelta::new(
                ObservationId::parse(get_str(object, "item_id")?.to_owned())
                    .map_err(ObservationError::Identifier)?,
                MessagePhase::parse(get_str(object, "phase")?)?,
                get_str(object, "delta")?.to_owned(),
            )
            .map(TranscriptContent::AgentMessageDelta)
            .map_err(ObservationCommitError::InvalidObservation)
        }
        "agent_message_completed" => {
            require_keys(object, &["tag", "item_id", "phase", "message"])?;
            TranscriptAgentMessageCompleted::new(
                ObservationId::parse(get_str(object, "item_id")?.to_owned())
                    .map_err(ObservationError::Identifier)?,
                MessagePhase::parse(get_str(object, "phase")?)?,
                get_str(object, "message")?.to_owned(),
            )
            .map(TranscriptContent::AgentMessageCompleted)
            .map_err(ObservationCommitError::InvalidObservation)
        }
        "reasoning_summary_delta" => {
            require_keys(object, &["tag", "item_id", "summary_index", "delta"])?;
            TranscriptReasoningSummaryDelta::new(
                ObservationId::parse(get_str(object, "item_id")?.to_owned())
                    .map_err(ObservationError::Identifier)?,
                get_u64(object, "summary_index")?,
                get_str(object, "delta")?.to_owned(),
            )
            .map(TranscriptContent::ReasoningSummaryDelta)
            .map_err(ObservationCommitError::InvalidObservation)
        }
        "reasoning_summary_completed" => {
            require_keys(object, &["tag", "item_id", "text"])?;
            TranscriptReasoningSummaryCompleted::new(
                ObservationId::parse(get_str(object, "item_id")?.to_owned())
                    .map_err(ObservationError::Identifier)?,
                get_opt_str(object, "text")?.map(str::to_owned),
            )
            .map(TranscriptContent::ReasoningSummaryCompleted)
            .map_err(ObservationCommitError::InvalidObservation)
        }
        "terminal_activity" => {
            require_keys(
                object,
                &[
                    "tag",
                    "activity_id",
                    "channel",
                    "command",
                    "exit_code",
                    "output",
                    "state",
                ],
            )?;
            let channel = match get_opt_str(object, "channel")? {
                None => None,
                Some(channel) => Some(TerminalChannel::parse(channel)?),
            };
            TranscriptTerminalActivity::new(
                ObservationId::parse(get_str(object, "activity_id")?.to_owned())
                    .map_err(ObservationError::Identifier)?,
                channel,
                get_opt_str(object, "command")?.map(str::to_owned),
                get_opt_i32(object, "exit_code")?,
                get_opt_str(object, "output")?.map(str::to_owned),
                TerminalActivityState::parse(get_str(object, "state")?)?,
            )
            .map(TranscriptContent::TerminalActivity)
            .map_err(ObservationCommitError::InvalidObservation)
        }
        "tool" => {
            require_keys(object, &["tag", "tool_id", "tool_name", "action", "detail"])?;
            TranscriptTool::new(
                ObservationId::parse(get_str(object, "tool_id")?.to_owned())
                    .map_err(ObservationError::Identifier)?,
                get_str(object, "tool_name")?.to_owned(),
                ToolAction::parse(get_str(object, "action")?)?,
                get_opt_str(object, "detail")?.map(str::to_owned),
            )
            .map(TranscriptContent::Tool)
            .map_err(ObservationCommitError::InvalidObservation)
        }
        "file" => {
            require_keys(
                object,
                &["tag", "path", "action", "lines_added", "lines_deleted"],
            )?;
            TranscriptFile::new(
                get_str(object, "path")?.to_owned(),
                FileAction::parse(get_str(object, "action")?)?,
                get_opt_u64(object, "lines_added")?,
                get_opt_u64(object, "lines_deleted")?,
            )
            .map(TranscriptContent::File)
            .map_err(ObservationCommitError::InvalidObservation)
        }
        "search" => {
            require_keys(
                object,
                &[
                    "tag",
                    "query",
                    "result_count",
                    "scope",
                    "search_id",
                    "state",
                ],
            )?;
            let scope = match get_opt_str(object, "scope")? {
                None => None,
                Some(scope) => Some(SearchScope::parse(scope)?),
            };
            TranscriptSearch::new(
                get_str(object, "query")?.to_owned(),
                get_opt_u64(object, "result_count")?,
                scope,
                get_opt_id(object, "search_id")?,
                SearchState::parse(get_str(object, "state")?)?,
            )
            .map(TranscriptContent::Search)
            .map_err(ObservationCommitError::InvalidObservation)
        }
        _ => Err(ObservationCommitError::UnknownObservation),
    }
}

#[allow(clippy::too_many_lines)]
fn decode_observation(object: &Map<String, Value>) -> Result<Observation, ObservationCommitError> {
    let tag = get_str(object, "tag")?;
    match tag {
        "agent_message_delta" => {
            require_keys(
                object,
                &[
                    "tag", "id", "sequence", "item_id", "phase", "delta", "turn_id",
                ],
            )?;
            let (id, sequence) = header(object)?;
            AgentMessageDeltaObservation::new(
                id,
                sequence,
                ObservationId::parse(get_str(object, "item_id")?.to_owned())
                    .map_err(ObservationError::Identifier)?,
                MessagePhase::parse(get_str(object, "phase")?)?,
                get_str(object, "delta")?.to_owned(),
                ObservationId::parse(get_str(object, "turn_id")?.to_owned())
                    .map_err(ObservationError::Identifier)?,
            )
            .map(Observation::AgentMessageDelta)
            .map_err(ObservationCommitError::InvalidObservation)
        }
        "agent_message_completed" => {
            require_keys(
                object,
                &[
                    "tag", "id", "sequence", "item_id", "phase", "message", "turn_id",
                ],
            )?;
            let (id, sequence) = header(object)?;
            AgentMessageCompletedObservation::new(
                id,
                sequence,
                ObservationId::parse(get_str(object, "item_id")?.to_owned())
                    .map_err(ObservationError::Identifier)?,
                MessagePhase::parse(get_str(object, "phase")?)?,
                get_str(object, "message")?.to_owned(),
                ObservationId::parse(get_str(object, "turn_id")?.to_owned())
                    .map_err(ObservationError::Identifier)?,
            )
            .map(Observation::AgentMessageCompleted)
            .map_err(ObservationCommitError::InvalidObservation)
        }
        "approval" => {
            require_keys(
                object,
                &[
                    "tag",
                    "id",
                    "sequence",
                    "approval_id",
                    "state",
                    "description",
                    "request",
                    "approved",
                ],
            )?;
            let (id, sequence) = header(object)?;
            let approval_id = ObservationId::parse(get_str(object, "approval_id")?.to_owned())
                .map_err(ObservationError::Identifier)?;
            let state = ApprovalState::parse(get_str(object, "state")?)?;
            let description = get_str(object, "description")?.to_owned();
            let request_object = object
                .get("request")
                .and_then(Value::as_object)
                .ok_or(ObservationCommitError::Malformed)?;
            let request = decode_approval_request(request_object)?;
            let approved = match object.get("approved") {
                None | Some(Value::Null) => None,
                Some(value) => Some(value.as_bool().ok_or(ObservationCommitError::Malformed)?),
            };
            match (state, approved) {
                (ApprovalState::Requested, None) => {
                    ApprovalObservation::requested(id, sequence, approval_id, description, request)
                }
                (ApprovalState::Resolved, Some(decision)) => ApprovalObservation::resolved(
                    id,
                    sequence,
                    approval_id,
                    description,
                    request,
                    decision,
                ),
                (ApprovalState::Requested, Some(_)) => {
                    Err(ObservationError::UnexpectedField { field: "approved" })
                }
                (ApprovalState::Resolved, None) => {
                    Err(ObservationError::MissingField { field: "approved" })
                }
            }
            .map(Observation::Approval)
            .map_err(ObservationCommitError::InvalidObservation)
        }
        "compaction" => {
            require_keys(
                object,
                &[
                    "tag",
                    "id",
                    "sequence",
                    "state",
                    "compaction_id",
                    "duration_ms",
                    "summary",
                ],
            )?;
            let (id, sequence) = header(object)?;
            CompactionObservation::new(
                id,
                sequence,
                CompactionState::parse(get_str(object, "state")?)?,
                get_opt_id(object, "compaction_id")?,
                get_opt_u64(object, "duration_ms")?,
                get_opt_str(object, "summary")?.map(str::to_owned),
            )
            .map(Observation::Compaction)
            .map_err(ObservationCommitError::InvalidObservation)
        }
        "file" => {
            require_keys(
                object,
                &[
                    "tag",
                    "id",
                    "sequence",
                    "path",
                    "action",
                    "lines_added",
                    "lines_deleted",
                ],
            )?;
            let (id, sequence) = header(object)?;
            FileObservation::new(
                id,
                sequence,
                get_str(object, "path")?.to_owned(),
                FileAction::parse(get_str(object, "action")?)?,
                get_opt_u64(object, "lines_added")?,
                get_opt_u64(object, "lines_deleted")?,
            )
            .map(Observation::File)
            .map_err(ObservationCommitError::InvalidObservation)
        }
        "native_action" => {
            require_keys(
                object,
                &[
                    "tag",
                    "id",
                    "sequence",
                    "action",
                    "detail",
                    "diagnostic",
                    "error_ref",
                ],
            )?;
            let (id, sequence) = header(object)?;
            NativeActionObservation::new(
                id,
                sequence,
                get_str(object, "action")?.to_owned(),
                get_opt_str(object, "detail")?.map(str::to_owned),
                get_bool(object, "diagnostic")?,
                decode_opt_error_ref(object)?,
            )
            .map(Observation::NativeAction)
            .map_err(ObservationCommitError::InvalidObservation)
        }
        "plan" => {
            require_keys(object, &["tag", "id", "sequence", "entries", "turn_id"])?;
            let (id, sequence) = header(object)?;
            PlanObservation::new(
                id,
                sequence,
                decode_plan_entries(object)?,
                get_opt_id(object, "turn_id")?,
            )
            .map(Observation::Plan)
            .map_err(ObservationCommitError::InvalidObservation)
        }
        "process_diagnostic" => {
            require_keys(
                object,
                &["tag", "id", "sequence", "level", "message", "error_ref"],
            )?;
            let (id, sequence) = header(object)?;
            ProcessDiagnosticObservation::new(
                id,
                sequence,
                DiagnosticLevel::parse(get_str(object, "level")?)?,
                get_str(object, "message")?.to_owned(),
                decode_opt_error_ref(object)?,
            )
            .map(Observation::ProcessDiagnostic)
            .map_err(ObservationCommitError::InvalidObservation)
        }
        "protocol_diagnostic" => {
            require_keys(object, &["tag", "id", "sequence", "level", "message"])?;
            let (id, sequence) = header(object)?;
            ProtocolDiagnosticObservation::new(
                id,
                sequence,
                DiagnosticLevel::parse(get_str(object, "level")?)?,
                get_str(object, "message")?.to_owned(),
            )
            .map(Observation::ProtocolDiagnostic)
            .map_err(ObservationCommitError::InvalidObservation)
        }
        "question" => {
            require_keys(
                object,
                &[
                    "tag",
                    "id",
                    "sequence",
                    "question_id",
                    "state",
                    "text",
                    "header",
                    "multi_select",
                    "options",
                    "answers",
                ],
            )?;
            let (id, sequence) = header(object)?;
            let input = QuestionInput {
                question_id: ObservationId::parse(get_str(object, "question_id")?.to_owned())
                    .map_err(ObservationError::Identifier)?,
                text: get_str(object, "text")?.to_owned(),
                header: get_opt_str(object, "header")?.map(str::to_owned),
                multi_select: get_bool(object, "multi_select")?,
                options: decode_question_options(object)?,
            };
            let state = QuestionState::parse(get_str(object, "state")?)?;
            let answers = get_opt_string_array(object, "answers")?;
            match (state, answers) {
                (QuestionState::Requested, None) => {
                    QuestionObservation::requested(id, sequence, input)
                }
                (QuestionState::Resolved, Some(resolved)) => {
                    QuestionObservation::resolved(id, sequence, input, resolved)
                }
                (QuestionState::Requested, Some(_)) => {
                    Err(ObservationError::UnexpectedField { field: "answers" })
                }
                (QuestionState::Resolved, None) => {
                    Err(ObservationError::MissingField { field: "answers" })
                }
            }
            .map(Observation::Question)
            .map_err(ObservationCommitError::InvalidObservation)
        }
        "reasoning_summary_completed" => {
            require_keys(
                object,
                &["tag", "id", "sequence", "item_id", "text", "turn_id"],
            )?;
            let (id, sequence) = header(object)?;
            ReasoningSummaryCompletedObservation::new(
                id,
                sequence,
                ObservationId::parse(get_str(object, "item_id")?.to_owned())
                    .map_err(ObservationError::Identifier)?,
                get_opt_str(object, "text")?.map(str::to_owned),
                ObservationId::parse(get_str(object, "turn_id")?.to_owned())
                    .map_err(ObservationError::Identifier)?,
            )
            .map(Observation::ReasoningSummaryCompleted)
            .map_err(ObservationCommitError::InvalidObservation)
        }
        "reasoning_summary_delta" => {
            require_keys(
                object,
                &[
                    "tag",
                    "id",
                    "sequence",
                    "item_id",
                    "summary_index",
                    "delta",
                    "thinking_tokens",
                    "turn_id",
                ],
            )?;
            let (id, sequence) = header(object)?;
            ReasoningSummaryDeltaObservation::new(
                id,
                sequence,
                ObservationId::parse(get_str(object, "item_id")?.to_owned())
                    .map_err(ObservationError::Identifier)?,
                get_u64(object, "summary_index")?,
                get_str(object, "delta")?.to_owned(),
                get_opt_u64(object, "thinking_tokens")?,
                ObservationId::parse(get_str(object, "turn_id")?.to_owned())
                    .map_err(ObservationError::Identifier)?,
            )
            .map(Observation::ReasoningSummaryDelta)
            .map_err(ObservationCommitError::InvalidObservation)
        }
        "retry" => {
            require_keys(
                object,
                &[
                    "tag",
                    "id",
                    "sequence",
                    "turn_id",
                    "attempt_state",
                    "will_retry",
                    "message",
                ],
            )?;
            let (id, sequence) = header(object)?;
            RetryObservation::new(
                id,
                sequence,
                ObservationId::parse(get_str(object, "turn_id")?.to_owned())
                    .map_err(ObservationError::Identifier)?,
                RetryAttemptState::parse(get_str(object, "attempt_state")?)?,
                get_bool(object, "will_retry")?,
                get_str(object, "message")?.to_owned(),
            )
            .map(Observation::Retry)
            .map_err(ObservationCommitError::InvalidObservation)
        }
        "run_state" => {
            require_keys(object, &["tag", "id", "sequence", "state"])?;
            let (id, sequence) = header(object)?;
            Ok(Observation::RunState(RunStateObservation::new(
                id,
                sequence,
                RunState::parse(get_str(object, "state")?)?,
            )))
        }
        "run_terminal" => {
            require_keys(
                object,
                &[
                    "tag",
                    "id",
                    "sequence",
                    "state",
                    "error_ref",
                    "summary_title",
                ],
            )?;
            let (id, sequence) = header(object)?;
            RunTerminalObservation::new(
                id,
                sequence,
                RunTerminalState::parse(get_str(object, "state")?)?,
                decode_opt_error_ref(object)?,
                get_opt_str(object, "summary_title")?.map(str::to_owned),
            )
            .map(Observation::RunTerminal)
            .map_err(ObservationCommitError::InvalidObservation)
        }
        "search" => {
            require_keys(
                object,
                &[
                    "tag",
                    "id",
                    "sequence",
                    "query",
                    "scope",
                    "search_id",
                    "state",
                    "result_count",
                ],
            )?;
            let (id, sequence) = header(object)?;
            let scope = match get_opt_str(object, "scope")? {
                None => None,
                Some(scope) => Some(SearchScope::parse(scope)?),
            };
            SearchObservation::new(
                id,
                sequence,
                get_str(object, "query")?.to_owned(),
                scope,
                get_opt_id(object, "search_id")?,
                SearchState::parse(get_str(object, "state")?)?,
                get_opt_u64(object, "result_count")?,
            )
            .map(Observation::Search)
            .map_err(ObservationCommitError::InvalidObservation)
        }
        "subagent" => {
            require_keys(
                object,
                &[
                    "tag",
                    "id",
                    "sequence",
                    "agent_native_thread_id",
                    "parent_native_thread_id",
                    "state",
                    "activity",
                    "agent_path",
                    "turn_id",
                ],
            )?;
            let (id, sequence) = header(object)?;
            SubagentObservation::new(
                id,
                sequence,
                SubagentInput {
                    agent_native_thread_id: ObservationId::parse(
                        get_str(object, "agent_native_thread_id")?.to_owned(),
                    )
                    .map_err(ObservationError::Identifier)?,
                    parent_native_thread_id: ObservationId::parse(
                        get_str(object, "parent_native_thread_id")?.to_owned(),
                    )
                    .map_err(ObservationError::Identifier)?,
                    state: SubagentState::parse(get_str(object, "state")?)?,
                    activity: get_opt_str(object, "activity")?.map(str::to_owned),
                    agent_path: get_opt_str(object, "agent_path")?.map(str::to_owned),
                    turn_id: get_opt_id(object, "turn_id")?,
                },
            )
            .map(Observation::Subagent)
            .map_err(ObservationCommitError::InvalidObservation)
        }
        "subagent_transcript" => {
            require_keys(
                object,
                &[
                    "tag",
                    "id",
                    "sequence",
                    "agent_native_thread_id",
                    "parent_native_thread_id",
                    "content",
                ],
            )?;
            let (id, sequence) = header(object)?;
            let content_object = object
                .get("content")
                .and_then(Value::as_object)
                .ok_or(ObservationCommitError::Malformed)?;
            Ok(Observation::SubagentTranscript(
                SubagentTranscriptObservation::new(
                    id,
                    sequence,
                    ObservationId::parse(get_str(object, "agent_native_thread_id")?.to_owned())
                        .map_err(ObservationError::Identifier)?,
                    ObservationId::parse(get_str(object, "parent_native_thread_id")?.to_owned())
                        .map_err(ObservationError::Identifier)?,
                    decode_transcript_content(content_object)?,
                ),
            ))
        }
        "terminal_activity" => {
            require_keys(
                object,
                &[
                    "tag",
                    "id",
                    "sequence",
                    "activity_id",
                    "channel",
                    "command",
                    "shell",
                    "output",
                    "exit_code",
                    "state",
                ],
            )?;
            let (id, sequence) = header(object)?;
            let channel = match get_opt_str(object, "channel")? {
                None => None,
                Some(channel) => Some(TerminalChannel::parse(channel)?),
            };
            TerminalActivityObservation::new(
                id,
                sequence,
                TerminalActivityInput {
                    activity_id: ObservationId::parse(get_str(object, "activity_id")?.to_owned())
                        .map_err(ObservationError::Identifier)?,
                    channel,
                    command: get_opt_str(object, "command")?.map(str::to_owned),
                    shell: get_opt_str(object, "shell")?.map(str::to_owned),
                    output: get_opt_str(object, "output")?.map(str::to_owned),
                    exit_code: get_opt_i32(object, "exit_code")?,
                    state: TerminalActivityState::parse(get_str(object, "state")?)?,
                },
            )
            .map(Observation::TerminalActivity)
            .map_err(ObservationCommitError::InvalidObservation)
        }
        "tool" => {
            require_keys(
                object,
                &[
                    "tag",
                    "id",
                    "sequence",
                    "tool_id",
                    "tool_name",
                    "action",
                    "detail",
                ],
            )?;
            let (id, sequence) = header(object)?;
            ToolObservation::new(
                id,
                sequence,
                ObservationId::parse(get_str(object, "tool_id")?.to_owned())
                    .map_err(ObservationError::Identifier)?,
                get_str(object, "tool_name")?.to_owned(),
                ToolAction::parse(get_str(object, "action")?)?,
                get_opt_str(object, "detail")?.map(str::to_owned),
            )
            .map(Observation::Tool)
            .map_err(ObservationCommitError::InvalidObservation)
        }
        "turn_state" => {
            require_keys(object, &["tag", "id", "sequence", "turn_id", "state"])?;
            let (id, sequence) = header(object)?;
            Ok(Observation::TurnState(TurnStateObservation::new(
                id,
                sequence,
                ObservationId::parse(get_str(object, "turn_id")?.to_owned())
                    .map_err(ObservationError::Identifier)?,
                TurnState::parse(get_str(object, "state")?)?,
            )))
        }
        "usage" => {
            require_keys(
                object,
                &[
                    "tag",
                    "id",
                    "sequence",
                    "basis",
                    "input_tokens",
                    "cached_input_tokens",
                    "output_tokens",
                    "context_tokens",
                    "context_window_tokens",
                    "cost_usd",
                    "provider_route_id",
                    "turn_id",
                ],
            )?;
            let (id, sequence) = header(object)?;
            UsageObservation::new(
                id,
                sequence,
                UsageInput {
                    basis: UsageBasis::parse(get_str(object, "basis")?)?,
                    input_tokens: get_opt_u64(object, "input_tokens")?,
                    cached_input_tokens: get_opt_u64(object, "cached_input_tokens")?,
                    output_tokens: get_opt_u64(object, "output_tokens")?,
                    context_tokens: get_opt_u64(object, "context_tokens")?,
                    context_window_tokens: get_opt_u64(object, "context_window_tokens")?,
                    cost_usd: get_opt_f64(object, "cost_usd")?,
                    provider_route_id: get_opt_id(object, "provider_route_id")?,
                    turn_id: get_opt_id(object, "turn_id")?,
                },
            )
            .map(Observation::Usage)
            .map_err(ObservationCommitError::InvalidObservation)
        }
        _ => Err(ObservationCommitError::UnknownObservation),
    }
}
