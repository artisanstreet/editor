//! Portable handoff for a run that cannot resume the thread's provider
//! session.
//!
//! Ported from the Electron backend's thread continuation service and
//! compactor. A provider session is scoped to one engine and profile, so a
//! thread that moves to another engine, or whose last session ended without
//! a resumable binding, starts a fresh provider session. That session's
//! first prompt carries a checkpoint of the conversation so far: the newest
//! messages verbatim, and everything older summarized by a compaction model
//! into a fixed template, the way `OpenCode` compacts a session. When no
//! compaction model can run, a mechanical summary names the original
//! objective and how much was left out, and the switch still proceeds.

#[path = "handoff/checkpoint.rs"]
mod checkpoint;

use std::path::Path;
use std::time::Duration;

use artisan_database::Repository;
use artisan_domain::{
    EngineId, EngineModelId, EngineSelection, ErrorChain, RootPath, RunId, ThreadId, TurnId,
};
use artisan_native_engine::{VerifiedClaudeLaunch, VerifiedCodexLaunch};
use artisan_transport::CancelHandle;

use crate::engine_owner::EngineHandoff;
use crate::engine_owner::handoff::{summarize_with_claude, summarize_with_codex};

use super::ResolvedLaunch;
use super::claim::{resolve_claude_launch, resolve_codex_launch};
use super::diagnostics::StepError;

/// Bounds every compaction attempt of one handoff together, so a stuck
/// provider cannot hold the switch.
const COMPACTION_BUDGET: Duration = Duration::from_mins(5);

/// The newest run that reached its provider, whose session the new run
/// cannot resume.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct HandoffSource {
    pub(super) run_id: RunId,
}

/// Everything one handoff reads.
pub(super) struct HandoffRequest<'a> {
    pub(super) repository: &'a Repository,
    pub(super) database_path: &'a Path,
    pub(super) thread_id: &'a ThreadId,
    /// The turn being launched; its own request is not history.
    pub(super) turn_id: &'a TurnId,
    pub(super) run_id: &'a RunId,
    pub(super) source: &'a HandoffSource,
    pub(super) project_root: &'a RootPath,
    /// The launch the new run starts on, tried first as the compactor: it
    /// is verified to run now, while the source engine may be the one the
    /// user is leaving (signed out, or out of quota).
    pub(super) target_launch: &'a ResolvedLaunch,
    pub(super) target_selection: &'a EngineSelection,
    /// The run's stop request and the dispatcher's own shutdown signals.
    /// Any of them ends compaction early; the mechanical summary is used
    /// instead and the stop then settles through the normal start path.
    pub(super) stops: [&'a CancelHandle; 3],
}

/// Builds the history a fresh provider session starts from.
///
/// Returns `Ok(None)` when the thread holds no settled messages, so there
/// is nothing to hand over.
///
/// # Errors
///
/// Returns the failed step when the transcript cannot be read. Compaction
/// failures never fail the handoff.
pub(super) async fn prepare_handoff(
    request: HandoffRequest<'_>,
) -> Result<Option<EngineHandoff>, StepError> {
    let transcript = request
        .repository
        .read_handoff_transcript(
            request.thread_id,
            request.turn_id,
            checkpoint::TRANSCRIPT_READ_BYTES,
            checkpoint::ENTRY_MAX_BYTES,
        )
        .await
        .map_err(|error| {
            StepError::failed("reading the thread's history for the handoff", error)
        })?;
    if transcript.entries.is_empty() {
        return Ok(None);
    }
    let split = checkpoint::split(&transcript.entries);
    let unread = transcript
        .total_entries
        .saturating_sub(transcript.entries.len() as u64);
    let omitted = unread + split.head.len() as u64;
    let kept = split.tail.len();
    let summary = if omitted == 0 {
        eprintln!(
            "native run handoff (thread {} run {}): all {kept} messages carried verbatim",
            request.thread_id, request.run_id
        );
        checkpoint::mechanical_summary(None, 0)
    } else {
        let prompt = checkpoint::compaction_prompt(split.head, unread);
        let compacted = tokio::select! {
            biased;
            () = stopped(request.stops) => None,
            compacted = tokio::time::timeout(COMPACTION_BUDGET, compact(&request, &prompt)) => {
                compacted.unwrap_or_else(|_| {
                    eprintln!(
                        "handoff compaction gave up (thread {} run {}): no summary within {} s; handing over the mechanical summary",
                        request.thread_id,
                        request.run_id,
                        COMPACTION_BUDGET.as_secs()
                    );
                    None
                })
            }
        };
        if let Some((summary, engine)) = compacted {
            eprintln!(
                "native run handoff (thread {} run {}): {omitted} earlier messages summarized by {engine:?}, {kept} carried verbatim",
                request.thread_id, request.run_id
            );
            summary
        } else {
            eprintln!(
                "native run handoff (thread {} run {}): mechanical summary, {omitted} earlier messages left out, {kept} carried verbatim",
                request.thread_id, request.run_id
            );
            checkpoint::mechanical_summary(transcript.first_user_message.as_deref(), omitted)
        }
    };
    Ok(Some(EngineHandoff::new(checkpoint::render_context(
        &summary,
        &split.tail,
    ))))
}

/// Resolves once any of the stop signals fires.
async fn stopped([run, dispatcher, process]: [&CancelHandle; 3]) {
    tokio::select! {
        () = run.wait() => {}
        () = dispatcher.wait() => {}
        () = process.wait() => {}
    }
}

/// Runs the compaction prompt on the first compactor that answers: the
/// target engine, then the source engine. Returns the summary and the
/// engine that wrote it.
async fn compact(request: &HandoffRequest<'_>, prompt: &str) -> Option<(String, EngineId)> {
    let target_engine = request.target_selection.engine_id();
    if let Some(compactor) = target_compactor(request.target_launch, request.target_selection)
        && let Some(summary) = attempt(request, &compactor, prompt).await
    {
        return Some(summary);
    }
    let source = match request
        .repository
        .read_run_engine_selection(request.thread_id, &request.source.run_id)
        .await
    {
        Ok(Some(selection)) if selection.engine_id() != target_engine => selection,
        Ok(_) => return None,
        Err(error) => {
            eprintln!(
                "handoff compaction skipped the source engine (thread {} run {}): {}",
                request.thread_id,
                request.run_id,
                ErrorChain(&StepError::failed(
                    "reading the source run's engine selection",
                    error
                ))
            );
            return None;
        }
    };
    match &source {
        EngineSelection::Claude(selection) => {
            let launch = resolve_source(
                request,
                resolve_claude_launch(request.database_path, selection.profile_id()).await,
            )?;
            let compactor = Compactor::Claude {
                launch: &launch,
                model: selection.model_id().map(EngineModelId::as_str),
            };
            attempt(request, &compactor, prompt).await
        }
        EngineSelection::Codex(selection) => {
            let launch = resolve_source(
                request,
                resolve_codex_launch(request.database_path, selection.profile_id()).await,
            )?;
            let compactor = Compactor::Codex {
                launch: &launch,
                model: selection.model_id().map(EngineModelId::as_str),
            };
            attempt(request, &compactor, prompt).await
        }
        EngineSelection::OpenCode2(_) | EngineSelection::Grok(_) | EngineSelection::Cursor(_) => {
            None
        }
    }
}

fn resolve_source<T>(
    request: &HandoffRequest<'_>,
    resolved: Result<T, super::claim::LaunchProbeFailure>,
) -> Option<T> {
    resolved
        .inspect_err(|failure| {
            eprintln!(
                "handoff compaction skipped the source engine (thread {} run {}): {}",
                request.thread_id,
                request.run_id,
                failure.reason()
            );
        })
        .ok()
}

/// An engine that can run one tool-less summary request.
enum Compactor<'a> {
    Claude {
        launch: &'a VerifiedClaudeLaunch,
        model: Option<&'a str>,
    },
    Codex {
        launch: &'a VerifiedCodexLaunch,
        model: Option<&'a str>,
    },
}

impl Compactor<'_> {
    const fn engine(&self) -> EngineId {
        match self {
            Self::Claude { .. } => EngineId::Claude,
            Self::Codex { .. } => EngineId::Codex,
        }
    }
}

fn target_compactor<'a>(
    launch: &'a ResolvedLaunch,
    selection: &'a EngineSelection,
) -> Option<Compactor<'a>> {
    match (launch, selection) {
        (ResolvedLaunch::Claude(launch), EngineSelection::Claude(selection)) => {
            Some(Compactor::Claude {
                launch,
                model: selection.model_id().map(EngineModelId::as_str),
            })
        }
        (ResolvedLaunch::Codex(launch), EngineSelection::Codex(selection)) => {
            Some(Compactor::Codex {
                launch,
                model: selection.model_id().map(EngineModelId::as_str),
            })
        }
        _ => None,
    }
}

async fn attempt(
    request: &HandoffRequest<'_>,
    compactor: &Compactor<'_>,
    prompt: &str,
) -> Option<(String, EngineId)> {
    let reply = match compactor {
        Compactor::Claude { launch, model } => {
            summarize_with_claude(launch, request.project_root, *model, prompt).await
        }
        Compactor::Codex { launch, model } => {
            summarize_with_codex(launch, request.project_root, *model, prompt).await
        }
    };
    let engine = compactor.engine();
    match reply.map(|reply| checkpoint::accept_summary(&reply)) {
        Ok(Some(summary)) => Some((summary, engine)),
        Ok(None) => {
            eprintln!(
                "handoff compaction by {engine:?} returned no usable summary (thread {} run {})",
                request.thread_id, request.run_id
            );
            None
        }
        Err(reason) => {
            eprintln!(
                "handoff compaction by {engine:?} failed (thread {} run {}): {reason}",
                request.thread_id, request.run_id
            );
            None
        }
    }
}
