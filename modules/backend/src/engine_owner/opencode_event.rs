//! Stateful, bounded normalization of real `OpenCode2` session envelopes.
//!
//! This leaf deliberately sits beside, rather than inside, `event.rs`.  The
//! latter is the legacy fixture decoder and its `run_id`/`sequence`/
//! `delta|state` contract remains unchanged until the owner wiring packet
//! switches production streams over.  `OpenCode2` itself supplies an envelope
//! shaped like `{ "type", "data", "durable": { "seq" } }`; its Artisan run
//! identity is therefore always supplied by the owner that creates this
//! adapter.

use std::collections::{HashMap, HashSet};

use artisan_domain::RunId;
use serde_json::{Map, Value};
use thiserror::Error;

use crate::engine_owner::consts::MAX_PROVIDER_ID_BYTES;
use crate::engine_owner::framing::SseEvent;
use crate::engine_owner::observation::{
    EngineObservation, TerminalObservation, TerminalState, chunk_text,
};

/// Maximum complete provider envelope accepted by this normalization leaf.
///
/// The SSE framer supplies a larger transport ceiling.  This smaller
/// application ceiling ensures that the temporary `serde_json::Value` tree is
/// finite even when the provider sends unrelated fields.  Provider text is
/// never truncated: a value over this bound is rejected instead.
pub(crate) const OPENCODE2_EVENT_MAX_BYTES: usize = 256 * 1024;

/// Maximum UTF-8 bytes retained for one assistant text part.
pub(crate) const OPENCODE2_TEXT_PART_MAX_BYTES: usize = 1024 * 1024;

/// Maximum assistant parts retained by one configured turn.
pub(crate) const OPENCODE2_MAX_TEXT_PARTS: usize = 128;

/// Maximum source identities retained by one configured turn.
pub(crate) const OPENCODE2_MAX_DEDUP_IDENTITIES: usize = 1024;

const MAX_EVENT_TYPE_BYTES: usize = 128;
const MAX_PROVIDER_SESSION_BYTES: usize = 256;
const MAX_SOURCE_ID_BYTES: usize = 1024;
const MAX_REASON_BYTES: usize = 1024;
const MAX_ERROR_REF_BYTES: usize = 256;

type JsonObject = Map<String, Value>;

/// Provider event-kind bits returned even when an event has no observation.
///
/// In particular, `usage` lets the owner route the original envelope to the
/// separate bounded usage parser without making usage look like a terminal or
/// text observation.
#[expect(
    clippy::struct_excessive_bools,
    reason = "the flag word mirrors the provider event-kind vocabulary bit-for-bit; a bitset type would obscure the public field reads"
)]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct OpenCodeEventKindFlags {
    pub(crate) error: bool,
    pub(crate) interaction: bool,
    pub(crate) lifecycle: bool,
    pub(crate) log_synced: bool,
    pub(crate) reasoning: bool,
    pub(crate) terminal: bool,
    pub(crate) text_delta: bool,
    pub(crate) text_ended: bool,
    pub(crate) tool: bool,
    pub(crate) usage: bool,
}

/// What the owner must do with a complete `session.text.ended` part.
///
/// `Confirmed` means every byte already arrived as live deltas, so the owner
/// may only mark the part ended.  `Replace` is deliberately not an
/// `EngineObservation::TextDelta`: the full provider text did not equal the
/// text already streamed (or no live text existed), and appending it would
/// duplicate or lose content.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum OpenCodeTextReconciliation {
    Confirmed {
        run_id: RunId,
        session_id: String,
        part_id: String,
        source_event_id: String,
    },
    Replace {
        run_id: RunId,
        session_id: String,
        part_id: String,
        source_event_id: String,
        text: String,
    },
}

/// Bounded output for one complete provider envelope.
///
/// `provider_cursor` is the actual `OpenCode` `durable.seq` (or the actual
/// top-level `seq` on `log.synced`) from this envelope.  It is not the
/// sequence used to satisfy the older `EngineObservation` shape when a live
/// event has no durable cursor.  In that case the adapter allocates a
/// monotonic local sequence starting at one solely for the observation's
/// existing `sequence` field; it never reports that local value as a provider
/// cursor.
#[derive(Debug, PartialEq)]
pub(crate) struct OpenCodeEventResult {
    pub(crate) observations: Vec<EngineObservation>,
    pub(crate) provider_cursor: Option<u64>,
    pub(crate) recognized_event_kind: OpenCodeEventKindFlags,
    pub(crate) text_reconciliation: Option<OpenCodeTextReconciliation>,
}

/// Payload-free failure while normalizing one bounded `OpenCode` envelope.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Error)]
pub(crate) enum OpenCodeEventError {
    #[error("provider event exceeds the bounded adapter input limit")]
    EventTooLarge,
    #[error("provider event is not valid json")]
    InvalidJson,
    #[error("provider event envelope is not an object")]
    InvalidEnvelope,
    #[error("provider event type is not valid")]
    InvalidEventType,
    #[error("provider event data is not an object")]
    InvalidData,
    #[error("provider event session id is missing")]
    MissingSessionId,
    #[error("provider event session id is not valid")]
    InvalidSessionId,
    #[error("provider event session does not match the active session")]
    SessionMismatch,
    #[error("provider event durable cursor is not a bounded integer")]
    InvalidProviderCursor,
    #[error("provider event source identity is not valid")]
    InvalidEventIdentity,
    #[error("provider event source identity is missing")]
    MissingEventIdentity,
    #[error("provider event source identity exceeds the bounded limit")]
    EventIdentityTooLong,
    #[error("provider event assistant message id is not valid")]
    InvalidAssistantMessageId,
    #[error("provider event text part identity is missing")]
    MissingTextPartIdentity,
    #[error("provider event text part identity is not valid")]
    InvalidTextPartIdentity,
    #[error("provider event text is not valid")]
    InvalidText,
    #[error("provider event text exceeds the bounded part limit")]
    TextTooLong,
    #[error("provider text delta arrived after its part ended")]
    TextDeltaAfterPartEnded,
    #[error("provider event has too many text parts")]
    TooManyTextParts,
    #[error("provider event deduplication memory is exhausted")]
    DeduplicationMemoryExhausted,
    #[error("provider event local sequence is exhausted")]
    LocalSequenceExhausted,
    #[error("provider event reason is not valid")]
    InvalidReason,
    #[error("provider event error reference is not valid")]
    InvalidErrorRef,
    #[error("provider event arrived after the terminal event")]
    EventAfterTerminal,
    #[error("provider event has a conflicting terminal state")]
    TerminalConflict,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum OpenCodeEventKind {
    Error,
    ExecutionFailed,
    ExecutionInterrupted,
    ExecutionStarted,
    ExecutionSucceeded,
    Form,
    Interaction,
    Lifecycle,
    LogSynced,
    Reasoning,
    StepFailed,
    StepStarted,
    StepEnded,
    TextDelta,
    TextEnded,
    Tool,
    Unknown,
}

impl OpenCodeEventKind {
    fn flags(self) -> OpenCodeEventKindFlags {
        match self {
            Self::Error => OpenCodeEventKindFlags {
                error: true,
                ..Default::default()
            },
            Self::ExecutionFailed | Self::ExecutionInterrupted | Self::ExecutionSucceeded => {
                OpenCodeEventKindFlags {
                    lifecycle: true,
                    terminal: true,
                    ..Default::default()
                }
            }
            Self::ExecutionStarted
            | Self::Lifecycle
            | Self::StepFailed
            | Self::StepStarted
            | Self::StepEnded => OpenCodeEventKindFlags {
                lifecycle: true,
                usage: matches!(self, Self::StepFailed | Self::StepEnded),
                ..Default::default()
            },
            Self::Form | Self::Interaction => OpenCodeEventKindFlags {
                interaction: true,
                ..Default::default()
            },
            Self::LogSynced => OpenCodeEventKindFlags {
                log_synced: true,
                ..Default::default()
            },
            Self::Reasoning => OpenCodeEventKindFlags {
                reasoning: true,
                ..Default::default()
            },
            Self::TextDelta => OpenCodeEventKindFlags {
                text_delta: true,
                ..Default::default()
            },
            Self::TextEnded => OpenCodeEventKindFlags {
                text_ended: true,
                ..Default::default()
            },
            Self::Tool => OpenCodeEventKindFlags {
                tool: true,
                ..Default::default()
            },
            Self::Unknown => OpenCodeEventKindFlags::default(),
        }
    }

    const fn requires_session(self) -> bool {
        !matches!(self, Self::LogSynced | Self::Unknown)
    }

    const fn requires_source_identity(self) -> bool {
        matches!(
            self,
            Self::ExecutionFailed
                | Self::ExecutionInterrupted
                | Self::ExecutionSucceeded
                | Self::TextDelta
                | Self::TextEnded
        )
    }

    const fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::ExecutionFailed | Self::ExecutionInterrupted | Self::ExecutionSucceeded
        )
    }
}

fn terminal_state(
    kind: OpenCodeEventKind,
    data: &JsonObject,
) -> Result<Option<TerminalState>, OpenCodeEventError> {
    let state = match kind {
        OpenCodeEventKind::ExecutionFailed => Some(TerminalState::Failed),
        OpenCodeEventKind::ExecutionSucceeded => Some(TerminalState::Completed),
        OpenCodeEventKind::ExecutionInterrupted => {
            let reason = optional_bounded_text(data, "reason", MAX_REASON_BYTES)
                .map_err(|()| OpenCodeEventError::InvalidReason)?;
            Some(if reason.as_deref() == Some("user") {
                TerminalState::Cancelled
            } else {
                TerminalState::Interrupted
            })
        }
        _ => None,
    };
    Ok(state)
}

/*
 * Keep this mapping separate from `OpenCodeEventKind::flags`: the provider's
 * user interruption is a cancellation in the existing observation model,
 * while every other interruption remains distinct.
 */
fn terminal_state_for_kind(
    kind: OpenCodeEventKind,
    data: &JsonObject,
) -> Result<TerminalState, OpenCodeEventError> {
    terminal_state(kind, data)?.ok_or(OpenCodeEventError::TerminalConflict)
}

#[derive(Debug)]
struct TextPartState {
    text: String,
    ended: bool,
}

/// Stateful adapter for one immutable Artisan run/provider session pair.
///
/// Create one instance per configured turn and retain it across every SSE
/// batch, including live and durable-replay batches.  It does not retain raw
/// JSON or provider envelopes: only bounded source identities and bounded
/// text for currently observed parts survive between calls.
pub(crate) struct OpenCodeEventAdapter {
    run_id: RunId,
    session_id: String,
    parts: HashMap<String, TextPartState>,
    seen_source_events: HashSet<String>,
    terminal: Option<TerminalState>,
    next_local_sequence: u64,
    last_durable_event_cursor: Option<u64>,
    provider_cursor: Option<u64>,
}

impl OpenCodeEventAdapter {
    /// Creates an adapter bound to the owner-supplied run and provider session.
    pub(crate) fn new(run_id: RunId, session_id: String) -> Result<Self, OpenCodeEventError> {
        validate_identifier(&session_id, MAX_PROVIDER_SESSION_BYTES)
            .map_err(|()| OpenCodeEventError::InvalidSessionId)?;
        Ok(Self {
            run_id,
            session_id,
            parts: HashMap::new(),
            seen_source_events: HashSet::new(),
            terminal: None,
            next_local_sequence: 1,
            last_durable_event_cursor: None,
            provider_cursor: None,
        })
    }

    /// Returns the greatest provider cursor observed by this adapter.
    #[allow(dead_code)]
    #[must_use]
    pub(crate) fn provider_cursor(&self) -> Option<u64> {
        self.provider_cursor
    }

    /// Normalizes exactly one complete SSE event.
    ///
    /// Unknown event types and recognized non-text events are successful
    /// no-op results with flags/cursor metadata.  Only text events and actual
    /// execution terminal events require a source identity, because those are
    /// the events this leaf must deduplicate or expose as observations.
    #[expect(
        clippy::too_many_lines,
        reason = "one event-normalization dispatch over the provider vocabulary; extraction would thread the adapter state"
    )]
    pub(crate) fn normalize(
        &mut self,
        event: &SseEvent,
    ) -> Result<OpenCodeEventResult, OpenCodeEventError> {
        if event.data().len() > OPENCODE2_EVENT_MAX_BYTES {
            return Err(OpenCodeEventError::EventTooLarge);
        }
        let envelope: Value =
            serde_json::from_str(event.data()).map_err(|_| OpenCodeEventError::InvalidJson)?;
        let envelope = envelope
            .as_object()
            .ok_or(OpenCodeEventError::InvalidEnvelope)?;
        let event_type = envelope
            .get("type")
            .and_then(Value::as_str)
            .ok_or(OpenCodeEventError::InvalidEventType)?;
        validate_identifier(event_type, MAX_EVENT_TYPE_BYTES)
            .map_err(|()| OpenCodeEventError::InvalidEventType)?;

        if event_type == "session.updated" {
            let metadata = envelope.get("data").or_else(|| envelope.get("properties"));
            let metadata = metadata.and_then(|value| value.get("info").or(Some(value)));
            let title = metadata.and_then(|value| {
                let session = value
                    .get("sessionID")
                    .or_else(|| value.get("id"))?
                    .as_str()?;
                if session != self.session_id {
                    return None;
                }
                artisan_domain::ThreadTitle::parse(value.get("title")?.as_str()?.to_owned()).ok()
            });
            let observations = title
                .map(|title| EngineObservation::SummaryTitle {
                    run_id: self.run_id.clone(),
                    title,
                })
                .into_iter()
                .collect();
            return Ok(result(
                None,
                OpenCodeEventKindFlags::default(),
                observations,
                None,
            ));
        }
        let provider_cursor = provider_cursor(envelope, event_type)?;
        let kind = classify_event(event_type);
        if matches!(kind, OpenCodeEventKind::Unknown) {
            self.remember_provider_cursor(provider_cursor);
            return Ok(result(provider_cursor, kind.flags(), Vec::new(), None));
        }
        if matches!(kind, OpenCodeEventKind::LogSynced) {
            self.remember_provider_cursor(provider_cursor);
            return Ok(result(provider_cursor, kind.flags(), Vec::new(), None));
        }

        let data = envelope
            .get("data")
            .and_then(Value::as_object)
            .ok_or(OpenCodeEventError::InvalidData)?;
        if kind.requires_session() {
            self.validate_event_session(data, matches!(kind, OpenCodeEventKind::Form))?;
        }
        let flags = kind.flags();
        // A durable log is ordered by its source cursor. Its watermark gives
        // constant-memory replay suppression without a lifetime event-count cap.
        if let (Some(cursor), Some(processed)) = (provider_cursor, self.last_durable_event_cursor)
            && cursor <= processed
        {
            return Ok(result(provider_cursor, flags, Vec::new(), None));
        }
        let incoming_terminal = if kind.is_terminal() {
            Some(terminal_state_for_kind(kind, data)?)
        } else {
            None
        };

        let source_event_id = if kind.requires_source_identity()
            || has_source_identity(envelope, event, provider_cursor)
        {
            Some(self.source_event_id(envelope, event, event_type, provider_cursor)?)
        } else {
            None
        };
        if let Some(source_event_id) = source_event_id.as_deref() {
            if let Some(cursor) = provider_cursor {
                // Durable acknowledgement retires the live identity while
                // retaining suppression through the monotonic watermark.
                if self.seen_source_events.remove(source_event_id) {
                    self.last_durable_event_cursor = Some(cursor);
                    self.remember_provider_cursor(Some(cursor));
                    return Ok(result(provider_cursor, flags, Vec::new(), None));
                }
            } else if self.remember_source_event(source_event_id)? {
                return Ok(result(provider_cursor, flags, Vec::new(), None));
            }
        }

        if let Some(existing) = self.terminal {
            if let Some(incoming) = incoming_terminal {
                if existing != incoming {
                    return Err(OpenCodeEventError::TerminalConflict);
                }
                return Ok(result(provider_cursor, flags, Vec::new(), None));
            }
            if kind.requires_source_identity() {
                return Err(OpenCodeEventError::EventAfterTerminal);
            }
            return Ok(result(provider_cursor, flags, Vec::new(), None));
        }

        // The text kinds are covered by `requires_source_identity()` and the
        // terminal kinds by `is_terminal()`, so these typed errors are
        // defensive against classification drift, not expected provider input.
        let normalized = match kind {
            OpenCodeEventKind::TextDelta => self.normalize_text_delta(
                data,
                &source_event_id.ok_or(OpenCodeEventError::MissingEventIdentity)?,
                provider_cursor,
                flags,
            ),
            OpenCodeEventKind::TextEnded => self.normalize_text_ended(
                data,
                source_event_id.ok_or(OpenCodeEventError::MissingEventIdentity)?,
                provider_cursor,
                flags,
            ),
            OpenCodeEventKind::ExecutionFailed
            | OpenCodeEventKind::ExecutionInterrupted
            | OpenCodeEventKind::ExecutionSucceeded => self.normalize_terminal(
                data,
                incoming_terminal.ok_or(OpenCodeEventError::TerminalConflict)?,
                provider_cursor,
                flags,
            ),
            _ => Ok(result(provider_cursor, flags, Vec::new(), None)),
        };
        if normalized.is_ok() {
            self.remember_provider_cursor(provider_cursor);
            if let Some(cursor) = provider_cursor {
                self.last_durable_event_cursor = Some(cursor);
            }
        }
        normalized
    }

    fn normalize_text_delta(
        &mut self,
        data: &JsonObject,
        source_event_id: &str,
        provider_cursor: Option<u64>,
        flags: OpenCodeEventKindFlags,
    ) -> Result<OpenCodeEventResult, OpenCodeEventError> {
        let part_id = text_part_id(data)?;
        let delta = data
            .get("delta")
            .and_then(Value::as_str)
            .ok_or(OpenCodeEventError::InvalidText)?;
        if self.parts.get(&part_id).is_some_and(|part| part.ended) {
            return Err(OpenCodeEventError::TextDeltaAfterPartEnded);
        }
        let current_length = self.parts.get(&part_id).map_or(0, |part| part.text.len());
        let next_length = current_length
            .checked_add(delta.len())
            .ok_or(OpenCodeEventError::TextTooLong)?;
        if next_length > OPENCODE2_TEXT_PART_MAX_BYTES {
            return Err(OpenCodeEventError::TextTooLong);
        }
        if current_length == 0 && !self.parts.contains_key(&part_id) {
            self.insert_text_part(&part_id)?;
        }
        let sequence = if delta.is_empty() {
            None
        } else {
            Some(self.observation_sequence(provider_cursor)?)
        };
        if let Some(part) = self.parts.get_mut(&part_id) {
            part.text.push_str(delta);
        }
        let observations = sequence.map_or_else(Vec::new, |sequence| {
            chunk_text(&self.run_id, sequence, source_event_id, delta)
                .into_iter()
                .map(|delta| delta.with_part_id(part_id.clone()))
                .map(EngineObservation::TextDelta)
                .collect()
        });
        Ok(result(provider_cursor, flags, observations, None))
    }

    fn normalize_text_ended(
        &mut self,
        data: &JsonObject,
        source_event_id: String,
        provider_cursor: Option<u64>,
        flags: OpenCodeEventKindFlags,
    ) -> Result<OpenCodeEventResult, OpenCodeEventError> {
        let part_id = text_part_id(data)?;
        let text = data
            .get("text")
            .and_then(Value::as_str)
            .ok_or(OpenCodeEventError::InvalidText)?;
        if text.len() > OPENCODE2_TEXT_PART_MAX_BYTES {
            return Err(OpenCodeEventError::TextTooLong);
        }

        let Some(part) = self.parts.get_mut(&part_id) else {
            self.insert_text_part(&part_id)?;
            if let Some(part) = self.parts.get_mut(&part_id) {
                text.clone_into(&mut part.text);
                part.ended = true;
            }
            return Ok(result(
                provider_cursor,
                flags,
                Vec::new(),
                Some(OpenCodeTextReconciliation::Replace {
                    run_id: self.run_id.clone(),
                    session_id: self.session_id.clone(),
                    part_id,
                    source_event_id,
                    text: text.to_owned(),
                }),
            ));
        };

        if part.ended && part.text == text {
            return Ok(result(provider_cursor, flags, Vec::new(), None));
        }
        let replacement = part.text != text;
        text.clone_into(&mut part.text);
        part.ended = true;
        let reconciliation = if replacement {
            OpenCodeTextReconciliation::Replace {
                run_id: self.run_id.clone(),
                session_id: self.session_id.clone(),
                part_id,
                source_event_id,
                text: text.to_owned(),
            }
        } else {
            OpenCodeTextReconciliation::Confirmed {
                run_id: self.run_id.clone(),
                session_id: self.session_id.clone(),
                part_id,
                source_event_id,
            }
        };
        Ok(result(
            provider_cursor,
            flags,
            Vec::new(),
            Some(reconciliation),
        ))
    }

    fn normalize_terminal(
        &mut self,
        data: &JsonObject,
        state: TerminalState,
        provider_cursor: Option<u64>,
        flags: OpenCodeEventKindFlags,
    ) -> Result<OpenCodeEventResult, OpenCodeEventError> {
        let reason = optional_bounded_text(data, "reason", MAX_REASON_BYTES)
            .map_err(|()| OpenCodeEventError::InvalidReason)?;
        let error_ref = optional_bounded_text(data, "error_ref", MAX_ERROR_REF_BYTES)
            .map_err(|()| OpenCodeEventError::InvalidErrorRef)?;
        let sequence = self.observation_sequence(provider_cursor)?;
        self.terminal = Some(state);
        self.parts.clear();
        let terminal =
            TerminalObservation::new(self.run_id.clone(), sequence, state, reason, error_ref);
        Ok(result(
            provider_cursor,
            flags,
            vec![EngineObservation::Terminal(terminal)],
            None,
        ))
    }

    fn validate_event_session(
        &self,
        data: &JsonObject,
        is_form: bool,
    ) -> Result<(), OpenCodeEventError> {
        let direct = match data.get("sessionID") {
            None | Some(Value::Null) => None,
            Some(value) => Some(value.as_str().ok_or(OpenCodeEventError::InvalidSessionId)?),
        };
        let nested = if is_form {
            match data.get("form") {
                None | Some(Value::Null) => None,
                Some(value) => {
                    let form = value.as_object().ok_or(OpenCodeEventError::InvalidData)?;
                    match form.get("sessionID") {
                        None | Some(Value::Null) => None,
                        Some(value) => {
                            Some(value.as_str().ok_or(OpenCodeEventError::InvalidSessionId)?)
                        }
                    }
                }
            }
        } else {
            None
        };
        if let (Some(direct), Some(nested)) = (direct, nested)
            && direct != nested
        {
            return Err(OpenCodeEventError::SessionMismatch);
        }
        let session = direct
            .or(nested)
            .ok_or(OpenCodeEventError::MissingSessionId)?;
        validate_identifier(session, MAX_PROVIDER_SESSION_BYTES)
            .map_err(|()| OpenCodeEventError::InvalidSessionId)?;
        if session != self.session_id {
            return Err(OpenCodeEventError::SessionMismatch);
        }
        Ok(())
    }

    fn source_event_id(
        &self,
        envelope: &JsonObject,
        event: &SseEvent,
        event_type: &str,
        provider_cursor: Option<u64>,
    ) -> Result<String, OpenCodeEventError> {
        let raw = match envelope.get("id") {
            None | Some(Value::Null) => None,
            Some(value) => {
                let id = value
                    .as_str()
                    .ok_or(OpenCodeEventError::InvalidEventIdentity)?;
                if id.is_empty() {
                    None
                } else {
                    validate_source_id(id)?;
                    Some(format!("envelope:{}:{}:{id}", self.session_id, event_type))
                }
            }
        };
        let raw = if let Some(raw) = raw {
            raw
        } else if let Some(id) = event.id().filter(|id| !id.is_empty()) {
            validate_source_id(id)?;
            format!("sse:{}:{}:{id}", self.session_id, event_type)
        } else if let Some(provider_cursor) = provider_cursor {
            format!(
                "durable:{}:{}:{provider_cursor}",
                self.session_id, event_type
            )
        } else {
            return Err(OpenCodeEventError::MissingEventIdentity);
        };
        if raw.len() > MAX_SOURCE_ID_BYTES {
            return Err(OpenCodeEventError::EventIdentityTooLong);
        }
        Ok(raw)
    }

    fn remember_source_event(&mut self, source_event_id: &str) -> Result<bool, OpenCodeEventError> {
        if self.seen_source_events.contains(source_event_id) {
            return Ok(true);
        }
        if self.seen_source_events.len() >= OPENCODE2_MAX_DEDUP_IDENTITIES {
            return Err(OpenCodeEventError::DeduplicationMemoryExhausted);
        }
        self.seen_source_events.insert(source_event_id.to_owned());
        Ok(false)
    }

    fn insert_text_part(&mut self, part_id: &str) -> Result<(), OpenCodeEventError> {
        if self.parts.len() >= OPENCODE2_MAX_TEXT_PARTS {
            return Err(OpenCodeEventError::TooManyTextParts);
        }
        self.parts.insert(
            part_id.to_owned(),
            TextPartState {
                text: String::new(),
                ended: false,
            },
        );
        Ok(())
    }

    fn observation_sequence(
        &mut self,
        provider_cursor: Option<u64>,
    ) -> Result<u64, OpenCodeEventError> {
        if let Some(provider_cursor) = provider_cursor {
            return Ok(provider_cursor);
        }
        let sequence = self.next_local_sequence;
        self.next_local_sequence = sequence
            .checked_add(1)
            .ok_or(OpenCodeEventError::LocalSequenceExhausted)?;
        Ok(sequence)
    }

    fn remember_provider_cursor(&mut self, provider_cursor: Option<u64>) {
        if let Some(provider_cursor) = provider_cursor {
            self.provider_cursor = Some(
                self.provider_cursor
                    .map_or(provider_cursor, |current| current.max(provider_cursor)),
            );
        }
    }
}

fn result(
    provider_cursor: Option<u64>,
    recognized_event_kind: OpenCodeEventKindFlags,
    observations: Vec<EngineObservation>,
    text_reconciliation: Option<OpenCodeTextReconciliation>,
) -> OpenCodeEventResult {
    OpenCodeEventResult {
        observations,
        provider_cursor,
        recognized_event_kind,
        text_reconciliation,
    }
}

fn classify_event(event_type: &str) -> OpenCodeEventKind {
    match event_type {
        "log.synced" => OpenCodeEventKind::LogSynced,
        "session.execution.failed" => OpenCodeEventKind::ExecutionFailed,
        "session.execution.interrupted" => OpenCodeEventKind::ExecutionInterrupted,
        "session.execution.started" => OpenCodeEventKind::ExecutionStarted,
        "session.execution.succeeded" => OpenCodeEventKind::ExecutionSucceeded,
        "session.reasoning.delta" | "session.reasoning.ended" => OpenCodeEventKind::Reasoning,
        "session.step.failed" => OpenCodeEventKind::StepFailed,
        "session.step.started" => OpenCodeEventKind::StepStarted,
        "session.step.ended" => OpenCodeEventKind::StepEnded,
        "session.text.delta" => OpenCodeEventKind::TextDelta,
        "session.text.ended" => OpenCodeEventKind::TextEnded,
        "session.tool.input.started"
        | "session.tool.called"
        | "session.tool.progress"
        | "session.tool.success"
        | "session.tool.failed" => OpenCodeEventKind::Tool,
        "session.retry.scheduled"
        | "session.compaction.started"
        | "session.compaction.ended"
        | "session.shell.started"
        | "session.shell.ended"
        | "session.status"
        | "session.idle" => OpenCodeEventKind::Lifecycle,
        "session.error" | "session.execution.error" | "error" => OpenCodeEventKind::Error,
        "form.created" | "session.form.created" => OpenCodeEventKind::Form,
        "permission.asked" | "permission.replied" => OpenCodeEventKind::Interaction,
        _ => OpenCodeEventKind::Unknown,
    }
}

fn provider_cursor(
    envelope: &JsonObject,
    event_type: &str,
) -> Result<Option<u64>, OpenCodeEventError> {
    let durable_cursor = match envelope.get("durable") {
        None | Some(Value::Null) => None,
        Some(value) => {
            let durable = value
                .as_object()
                .ok_or(OpenCodeEventError::InvalidProviderCursor)?;
            match durable.get("seq") {
                None | Some(Value::Null) => None,
                Some(value) => Some(
                    value
                        .as_u64()
                        .ok_or(OpenCodeEventError::InvalidProviderCursor)?,
                ),
            }
        }
    };
    if durable_cursor.is_some() || event_type != "log.synced" {
        return Ok(durable_cursor);
    }
    match envelope.get("seq") {
        None | Some(Value::Null) => Ok(None),
        Some(value) => Ok(Some(
            value
                .as_u64()
                .ok_or(OpenCodeEventError::InvalidProviderCursor)?,
        )),
    }
}

fn has_source_identity(
    envelope: &JsonObject,
    event: &SseEvent,
    provider_cursor: Option<u64>,
) -> bool {
    provider_cursor.is_some()
        || envelope
            .get("id")
            .and_then(Value::as_str)
            .is_some_and(|id| !id.is_empty())
        || event.id().is_some_and(|id| !id.is_empty())
}

fn text_part_id(data: &JsonObject) -> Result<String, OpenCodeEventError> {
    let assistant_message_id = data
        .get("assistantMessageID")
        .and_then(Value::as_str)
        .ok_or(OpenCodeEventError::InvalidAssistantMessageId)?;
    validate_identifier(assistant_message_id, MAX_PROVIDER_ID_BYTES)
        .map_err(|()| OpenCodeEventError::InvalidAssistantMessageId)?;

    let part = if let Some(value) = data.get("ordinal") {
        let ordinal = value
            .as_u64()
            .ok_or(OpenCodeEventError::InvalidTextPartIdentity)?;
        format!("{assistant_message_id}:ordinal:{ordinal}")
    } else if let Some(value) = data.get("partID") {
        let part_id = value
            .as_str()
            .ok_or(OpenCodeEventError::InvalidTextPartIdentity)?;
        validate_identifier(part_id, MAX_PROVIDER_ID_BYTES)
            .map_err(|()| OpenCodeEventError::InvalidTextPartIdentity)?;
        format!("{assistant_message_id}:part:{part_id}")
    } else {
        return Err(OpenCodeEventError::MissingTextPartIdentity);
    };
    if part.len() > MAX_SOURCE_ID_BYTES {
        return Err(OpenCodeEventError::InvalidTextPartIdentity);
    }
    Ok(part)
}

fn optional_bounded_text(
    data: &JsonObject,
    key: &str,
    maximum: usize,
) -> Result<Option<String>, ()> {
    match data.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(value) => {
            let text = value.as_str().ok_or(())?;
            if text.len() > maximum {
                return Err(());
            }
            Ok(Some(text.to_owned()))
        }
    }
}

fn validate_identifier(value: &str, maximum: usize) -> Result<(), ()> {
    if value.is_empty()
        || value.len() > maximum
        || value
            .chars()
            .any(|character| character.is_whitespace() || character.is_control())
    {
        return Err(());
    }
    Ok(())
}

fn validate_source_id(value: &str) -> Result<(), OpenCodeEventError> {
    if value.is_empty() || value.len() > MAX_SOURCE_ID_BYTES || value.chars().any(char::is_control)
    {
        return Err(if value.len() > MAX_SOURCE_ID_BYTES {
            OpenCodeEventError::EventIdentityTooLong
        } else {
            OpenCodeEventError::InvalidEventIdentity
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine_owner::framing::SseFramer;
    use crate::engine_owner::observation::TextDelta;

    fn new_adapter() -> OpenCodeEventAdapter {
        OpenCodeEventAdapter::new(
            RunId::parse("run-opencode-event-1").expect("valid run id"),
            "provider-session-1".to_owned(),
        )
        .expect("valid adapter")
    }

    #[test]
    fn generated_title_is_scoped_to_its_provider_session() {
        let mut adapter =
            OpenCodeEventAdapter::new(RunId::parse("title-run").unwrap(), "root".into()).unwrap();
        for (session, expected) in [("child", 0), ("root", 1)] {
            let frame = event(
                &format!(
                    r#"{{"type":"session.updated","data":{{"id":"{session}","title":"List project files"}}}}"#
                ),
                None,
                None,
            );
            let result = adapter.normalize(&frame).unwrap();
            assert_eq!(result.observations.len(), expected);
        }
    }

    fn event(payload: &str, envelope_id: Option<&str>, sse_id: Option<&str>) -> SseEvent {
        let mut frame = String::new();
        if let Some(sse_id) = sse_id {
            frame.push_str("id: ");
            frame.push_str(sse_id);
            frame.push('\n');
        }
        frame.push_str("data: ");
        frame.push_str(payload);
        frame.push_str("\n\n");
        let test_bound = OPENCODE2_EVENT_MAX_BYTES.saturating_mul(2);
        let mut framer = SseFramer::new(test_bound, test_bound).expect("valid test framer");
        let event = framer
            .feed(frame.as_bytes())
            .expect("valid test frame")
            .pop()
            .expect("one event");
        if let Some(envelope_id) = envelope_id {
            let value: Value = serde_json::from_str(payload).expect("typed test envelope");
            assert_eq!(value.get("id").and_then(Value::as_str), Some(envelope_id));
        }
        event
    }

    fn typed_event(
        event_type: &str,
        data: &str,
        durable: Option<u64>,
        envelope_id: Option<&str>,
    ) -> String {
        let id = envelope_id.map_or_else(String::new, |id| format!(",\"id\":\"{id}\""));
        let durable =
            durable.map_or_else(String::new, |seq| format!(",\"durable\":{{\"seq\":{seq}}}"));
        format!("{{\"type\":\"{event_type}\",\"data\":{data}{durable}{id}}}")
    }

    fn text_delta_from(observation: &EngineObservation) -> &TextDelta {
        match observation {
            EngineObservation::TextDelta(delta) => delta,
            EngineObservation::TextSnapshot(_)
            | EngineObservation::Usage(_)
            | EngineObservation::Activity(_)
            | EngineObservation::Subagent(_)
            | EngineObservation::SubagentTranscript(_) => {
                panic!("unexpected production observation in fixture")
            }
            EngineObservation::Terminal(_) | EngineObservation::SummaryTitle { .. } => {
                panic!("expected text delta")
            }
        }
    }

    #[test]
    fn actual_typed_delta_and_ended_reconcile_without_duplicate_text() {
        let mut adapter = new_adapter();
        let delta_payload = typed_event(
            "session.text.delta",
            r#"{"sessionID":"provider-session-1","assistantMessageID":"assistant-1","ordinal":0,"delta":"hello 🌍"}"#,
            Some(10),
            Some("envelope-delta-1"),
        );
        let delta = adapter
            .normalize(&event(&delta_payload, Some("envelope-delta-1"), None))
            .expect("actual delta envelope");
        assert_eq!(delta.provider_cursor, Some(10));
        assert!(delta.recognized_event_kind.text_delta);
        assert_eq!(delta.observations.len(), 1);
        let text = text_delta_from(&delta.observations[0]);
        assert_eq!(text.sequence(), 10);
        assert_eq!(text.delta(), "hello 🌍");

        let ended_payload = typed_event(
            "session.text.ended",
            r#"{"sessionID":"provider-session-1","assistantMessageID":"assistant-1","ordinal":0,"text":"hello 🌍"}"#,
            Some(11),
            Some("envelope-ended-1"),
        );
        let ended = adapter
            .normalize(&event(&ended_payload, Some("envelope-ended-1"), None))
            .expect("actual ended envelope");
        assert_eq!(ended.provider_cursor, Some(11));
        assert!(ended.recognized_event_kind.text_ended);
        assert!(matches!(
            ended.text_reconciliation,
            Some(OpenCodeTextReconciliation::Confirmed { .. })
        ));
        assert!(ended.observations.is_empty());

        let replayed_ended = adapter
            .normalize(&event(&ended_payload, None, None))
            .expect("replayed ended envelope");
        assert!(replayed_ended.observations.is_empty());
        assert!(replayed_ended.text_reconciliation.is_none());

        let terminal_payload = typed_event(
            "session.execution.succeeded",
            r#"{"sessionID":"provider-session-1"}"#,
            Some(12),
            Some("execution-succeeded-1"),
        );
        let terminal = adapter
            .normalize(&event(&terminal_payload, None, None))
            .expect("execution success envelope");
        assert!(terminal.recognized_event_kind.terminal);
        assert!(matches!(
            terminal.observations.as_slice(),
            [EngineObservation::Terminal(observation)]
                if observation.state() == TerminalState::Completed
        ));

        let replayed_delta = adapter
            .normalize(&event(&delta_payload, None, None))
            .expect("replayed delta after terminal");
        assert!(replayed_delta.observations.is_empty());
    }

    #[test]
    fn ended_without_live_text_is_an_explicit_replacement() {
        let mut adapter = new_adapter();
        let payload = typed_event(
            "session.text.ended",
            r#"{"sessionID":"provider-session-1","assistantMessageID":"assistant-2","ordinal":1,"text":"replayed full text"}"#,
            Some(20),
            Some("ended-replay-1"),
        );
        let result = adapter
            .normalize(&event(&payload, None, None))
            .expect("ended-only envelope");
        assert!(result.observations.is_empty());
        assert!(matches!(
            result.text_reconciliation,
            Some(OpenCodeTextReconciliation::Replace { ref text, .. })
                if text == "replayed full text"
        ));

        let replay = adapter
            .normalize(&event(&payload, None, None))
            .expect("duplicate ended-only envelope");
        assert!(replay.text_reconciliation.is_none());

        let correction = typed_event(
            "session.text.ended",
            r#"{"sessionID":"provider-session-1","assistantMessageID":"assistant-2","ordinal":1,"text":"corrected full text"}"#,
            Some(21),
            Some("ended-correction-1"),
        );
        let corrected = adapter
            .normalize(&event(&correction, None, None))
            .expect("changed ended text");
        assert!(matches!(
            corrected.text_reconciliation,
            Some(OpenCodeTextReconciliation::Replace { ref text, .. })
                if text == "corrected full text"
        ));
    }

    #[test]
    fn unicode_boundaries_and_multiple_assistant_parts_keep_chunk_ids_stable() {
        let mut adapter = new_adapter();
        let delta_text = format!("{}💡", "a".repeat(4095));
        let payload = typed_event(
            "session.text.delta",
            &format!(
                "{{\"sessionID\":\"provider-session-1\",\"assistantMessageID\":\"assistant-3\",\"ordinal\":0,\"delta\":{}}}",
                serde_json::to_string(&delta_text).expect("text json")
            ),
            None,
            Some("unicode-delta-1"),
        );
        let result = adapter
            .normalize(&event(&payload, None, None))
            .expect("unicode delta");
        assert_eq!(result.observations.len(), 2);
        let first = text_delta_from(&result.observations[0]);
        let second = text_delta_from(&result.observations[1]);
        assert_eq!(first.delta().len(), 4095);
        assert_eq!(second.delta(), "💡");
        assert_eq!(first.sequence(), second.sequence());
        assert!(first.chunk_id().ends_with(":1:0"));
        assert!(second.chunk_id().ends_with(":1:1"));

        let second_part = typed_event(
            "session.text.delta",
            r#"{"sessionID":"provider-session-1","assistantMessageID":"assistant-3","ordinal":1,"delta":"second part"}"#,
            None,
            Some("unicode-delta-2"),
        );
        let second_result = adapter
            .normalize(&event(&second_part, None, None))
            .expect("second assistant part");
        assert_eq!(second_result.observations.len(), 1);
        assert_eq!(
            text_delta_from(&second_result.observations[0]).delta(),
            "second part"
        );
        assert!(text_delta_from(&second_result.observations[0]).sequence() > first.sequence());
    }

    #[test]
    fn nontext_lifecycle_tool_reasoning_usage_and_error_events_do_not_finish_run() {
        let mut adapter = new_adapter();
        let events = [
            (
                "session.execution.started",
                r#"{"sessionID":"provider-session-1"}"#,
                Some(1),
            ),
            (
                "session.step.started",
                r#"{"sessionID":"provider-session-1","assistantMessageID":"assistant-4"}"#,
                Some(2),
            ),
            (
                "session.reasoning.delta",
                r#"{"sessionID":"provider-session-1","assistantMessageID":"assistant-4","ordinal":0,"delta":"thinking"}"#,
                Some(3),
            ),
            (
                "session.tool.success",
                r#"{"sessionID":"provider-session-1","id":"tool-1","name":"read"}"#,
                Some(4),
            ),
            (
                "session.step.ended",
                r#"{"sessionID":"provider-session-1","assistantMessageID":"assistant-4","tokens":{"input":4,"output":2}}"#,
                Some(5),
            ),
            (
                "session.error",
                r#"{"sessionID":"provider-session-1","error":{"message":"provider detail"}}"#,
                Some(6),
            ),
        ];
        for (index, (event_type, data, cursor)) in events.into_iter().enumerate() {
            let payload = typed_event(event_type, data, cursor, Some(&format!("nontext-{index}")));
            let result = adapter
                .normalize(&event(&payload, None, None))
                .expect("nontext provider event");
            assert!(result.observations.is_empty());
            assert!(result.text_reconciliation.is_none());
        }
        assert_eq!(adapter.provider_cursor(), Some(6));
        assert!(adapter.terminal.is_none());
    }

    #[test]
    fn form_session_is_read_from_nested_form_and_wrong_sessions_are_rejected() {
        let mut adapter = new_adapter();
        let form = typed_event(
            "form.created",
            r#"{"form":{"sessionID":"provider-session-1","id":"form-1"}}"#,
            None,
            Some("form-1"),
        );
        let result = adapter
            .normalize(&event(&form, None, None))
            .expect("nested form session");
        assert!(result.recognized_event_kind.interaction);
        assert!(result.observations.is_empty());

        let wrong_text = typed_event(
            "session.text.delta",
            r#"{"sessionID":"other-session","assistantMessageID":"assistant-5","ordinal":0,"delta":"wrong"}"#,
            Some(30),
            Some("wrong-session-1"),
        );
        assert_eq!(
            adapter.normalize(&event(&wrong_text, None, None)),
            Err(OpenCodeEventError::SessionMismatch)
        );
        assert_eq!(adapter.provider_cursor(), None);

        let wrong_form = typed_event(
            "form.created",
            r#"{"form":{"sessionID":"other-session","id":"form-2"}}"#,
            None,
            Some("form-2"),
        );
        assert_eq!(
            adapter.normalize(&event(&wrong_form, None, None)),
            Err(OpenCodeEventError::SessionMismatch)
        );
    }

    #[test]
    fn live_events_use_nonzero_local_sequences_and_sse_identity() {
        let mut adapter = new_adapter();
        let first_payload = typed_event(
            "session.text.delta",
            r#"{"sessionID":"provider-session-1","assistantMessageID":"assistant-6","ordinal":0,"delta":"one"}"#,
            None,
            Some("envelope-live-1"),
        );
        let first = adapter
            .normalize(&event(&first_payload, None, None))
            .expect("envelope-identified live event");
        let first_sequence = text_delta_from(&first.observations[0]).sequence();
        assert_eq!(first.provider_cursor, None);
        assert_eq!(first_sequence, 1);

        let replay_payload = typed_event(
            "session.text.delta",
            r#"{"sessionID":"provider-session-1","assistantMessageID":"assistant-6","ordinal":0,"delta":"one"}"#,
            Some(7),
            Some("envelope-live-1"),
        );
        let replay = adapter
            .normalize(&event(&replay_payload, Some("envelope-live-1"), None))
            .expect("durable replay of live event");
        assert_eq!(replay.provider_cursor, Some(7));
        assert!(replay.observations.is_empty());

        let second_payload = typed_event(
            "session.text.delta",
            r#"{"sessionID":"provider-session-1","assistantMessageID":"assistant-6","ordinal":0,"delta":"two"}"#,
            None,
            None,
        );
        let second = adapter
            .normalize(&event(&second_payload, None, Some("sse-2")))
            .expect("sse-identified second live event");
        assert_eq!(text_delta_from(&second.observations[0]).sequence(), 2);
        assert!(second.provider_cursor.is_none());
    }

    #[test]
    fn log_synced_is_only_a_cursor_marker_and_terminal_mapping_is_exact() {
        let mut adapter = new_adapter();
        let synced = r#"{"type":"log.synced","seq":41}"#;
        let synced = adapter
            .normalize(&event(synced, None, None))
            .expect("sync marker");
        assert!(synced.recognized_event_kind.log_synced);
        assert_eq!(synced.provider_cursor, Some(41));
        assert!(synced.observations.is_empty());

        let interrupted_user = typed_event(
            "session.execution.interrupted",
            r#"{"sessionID":"provider-session-1","reason":"user"}"#,
            Some(42),
            Some("interrupt-user-1"),
        );
        let interrupted_user = adapter
            .normalize(&event(&interrupted_user, None, None))
            .expect("user interruption");
        assert!(matches!(
            interrupted_user.observations.as_slice(),
            [EngineObservation::Terminal(observation)]
                if observation.state() == TerminalState::Cancelled
        ));

        let mut other = new_adapter();
        let interrupted_other = typed_event(
            "session.execution.interrupted",
            r#"{"sessionID":"provider-session-1","reason":"timeout"}"#,
            Some(43),
            Some("interrupt-other-1"),
        );
        let interrupted_other = other
            .normalize(&event(&interrupted_other, None, None))
            .expect("non-user interruption");
        assert!(matches!(
            interrupted_other.observations.as_slice(),
            [EngineObservation::Terminal(observation)]
                if observation.state() == TerminalState::Interrupted
        ));

        let mut failed = new_adapter();
        let failed_payload = typed_event(
            "session.execution.failed",
            r#"{"sessionID":"provider-session-1","error_ref":"provider-failed"}"#,
            Some(44),
            Some("failed-1"),
        );
        let failed = failed
            .normalize(&event(&failed_payload, None, None))
            .expect("failed execution");
        assert!(matches!(
            failed.observations.as_slice(),
            [EngineObservation::Terminal(observation)]
                if observation.state() == TerminalState::Failed
        ));
    }

    #[test]
    fn malformed_overbound_and_late_events_are_rejected_without_fake_completion() {
        let mut adapter = new_adapter();
        let invalid = event("not-json", None, None);
        assert_eq!(
            adapter.normalize(&invalid),
            Err(OpenCodeEventError::InvalidJson)
        );

        let huge = format!(
            "{{\"type\":\"unknown\",\"data\":\"{}\"}}",
            "x".repeat(OPENCODE2_EVENT_MAX_BYTES)
        );
        let huge_event = event(&huge, None, None);
        assert_eq!(
            adapter.normalize(&huge_event),
            Err(OpenCodeEventError::EventTooLarge)
        );

        let missing_identity = typed_event(
            "session.text.delta",
            r#"{"sessionID":"provider-session-1","assistantMessageID":"assistant-7","ordinal":0,"delta":"text"}"#,
            None,
            None,
        );
        assert_eq!(
            adapter.normalize(&event(&missing_identity, None, None)),
            Err(OpenCodeEventError::MissingEventIdentity)
        );

        let success = typed_event(
            "session.execution.succeeded",
            r#"{"sessionID":"provider-session-1"}"#,
            Some(50),
            Some("success-1"),
        );
        adapter
            .normalize(&event(&success, None, None))
            .expect("terminal success");
        let late_text = typed_event(
            "session.text.delta",
            r#"{"sessionID":"provider-session-1","assistantMessageID":"assistant-7","ordinal":0,"delta":"late"}"#,
            Some(51),
            Some("late-text-1"),
        );
        assert_eq!(
            adapter.normalize(&event(&late_text, None, None)),
            Err(OpenCodeEventError::EventAfterTerminal)
        );
    }
    #[test]
    fn durable_streams_outlive_the_live_identity_cache_without_duplicate_replay() {
        let mut adapter = new_adapter();
        let data = r#"{"sessionID":"provider-session-1","assistantMessageID":"long-message","ordinal":0,"delta":"x"}"#;
        for sequence in 1..=2048 {
            let payload = typed_event("session.text.delta", data, Some(sequence), None);
            let result = adapter
                .normalize(&event(&payload, None, None))
                .expect("long durable stream");
            assert_eq!(result.observations.len(), 1);
        }
        assert!(adapter.seen_source_events.is_empty());
        let replay = typed_event("session.text.delta", data, Some(1), None);
        assert!(
            adapter
                .normalize(&event(&replay, None, None))
                .expect("old replay")
                .observations
                .is_empty()
        );
        assert_eq!(adapter.provider_cursor(), Some(2048));
    }
}
