#![allow(clippy::module_name_repetitions)]
#![allow(dead_code)]

use std::ffi::OsString;

use artisan_domain::{CursorPermissionMode, CursorSelection, CursorSpeed, FilesystemAccess};
use thiserror::Error;

use super::super::acp::{AcpDefinition, CURSOR_ACP, ImageMode, LaunchArgs, cursor_build_args};

/// Sentinel version carried by [`CursorLaunch`] until the probe/authority
/// packet lands. Preflight, catalog, and turn paths reject the cursor launch
/// before any version gate reads it; the value never authorizes execution.
pub(crate) const CURSOR_C1_UNPROBED_VERSION: &str = "0.0.0-cursor-c1-unprobed";

/// Maximum accepted cursor session identity bytes (mirrors the ACP row bound
/// the transport enforces for `session/load`).
pub(crate) const CURSOR_MAX_SESSION_ID_BYTES: usize = 256;

/// Payload-free failure of the cursor definition boundary.
///
/// Transport mistakes (spawn, handshake, prompt, stream, stall, cancel,
/// shutdown, deadline, interruption, exit) surface as the owner
/// [`EngineOperationError`](super::super::operation::EngineOperationError) at the
/// dispatch arm; only typed-boundary mistakes originate here.
#[derive(Clone, Debug, Eq, PartialEq, Error)]
pub(crate) enum CursorTurnError {
    /// The durable selection or a provider frame violates the cursor adapter
    /// contract. The turn fails closed instead of seating a wrong session.
    #[error("cursor turn misconfigured")]
    Configuration,
    /// A stdio write over the ACP transport failed.
    #[error("cursor stdio write failed")]
    StreamFailed,
    /// A steer was issued while a prompt round is active. A waiting ACP
    /// session accepts a follow-up; an active prompt cannot be steered in
    /// place, mirroring `EngineUnsupportedCommandError` for `steer`.
    #[error("cursor steer unsupported while a prompt is active")]
    UnsupportedCommand,
}

/// Typed cursor settings derived from the durable selection.
///
/// Mirrors `CursorAcpArgs`/`ResolveCursorModel` in
/// `modules/engines/src/cursor/engine.ts` through the shared [`LaunchArgs`]
/// shape: the model stays optional (the adapter omits `--model` when no model
/// is selected), a read-only canonical policy maps to `--mode ask` at
/// argument-building time, and only the `force` permission mode maps to a CLI
/// flag. Effort/speed suffix resolution lives in the shared
/// [`cursor_build_args`] row builder; the durable selection keeps the raw
/// choices.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct CursorSettings {
    profile_id: String,
    model: Option<String>,
    reasoning_effort: Option<String>,
    speed_fast: bool,
    permission_force: bool,
    write_access: bool,
}

impl CursorSettings {
    /// Derives typed ACP settings from the durable selection.
    ///
    /// The cursor adapter imposes no construction-time permission rejections
    /// beyond the typed options: read-only maps to ask mode, never to a
    /// refusal.
    pub(crate) fn from_selection(selection: &CursorSelection) -> Self {
        Self {
            profile_id: selection.profile_id().as_str().to_owned(),
            model: selection.model_id().map(|model| model.as_str().to_owned()),
            reasoning_effort: selection
                .reasoning_effort()
                .map(|effort| effort.as_str().to_owned()),
            speed_fast: selection.speed() == Some(CursorSpeed::Fast),
            permission_force: selection.permission_mode() == Some(CursorPermissionMode::Force),
            write_access: selection.permission().filesystem() != FilesystemAccess::None,
        }
    }

    /// Returns the managed profile identity.
    #[must_use]
    pub(crate) fn profile_id(&self) -> &str {
        &self.profile_id
    }

    /// Projects these settings onto the shared ACP row args.
    #[must_use]
    pub(crate) fn launch_args(&self) -> LaunchArgs {
        LaunchArgs {
            model: self.model.clone(),
            reasoning_effort: self.reasoning_effort.clone(),
            speed_fast: self.speed_fast,
            permission: if self.permission_force {
                Some("force".to_owned())
            } else {
                None
            },
            write_access: self.write_access,
        }
    }

    /// Builds the exact cursor stdio argv tail for one turn: optional
    /// resolved `--model`, ask mode on read-only, `--force` mapping, then
    /// `acp`.
    #[must_use]
    pub(crate) fn build_args(&self) -> Vec<OsString> {
        cursor_build_args(&self.launch_args())
    }

    /// Returns the shared cursor ACP definition row: platform executable,
    /// native image blocks, `status` auth probe, `cursor_login` selection.
    #[must_use]
    pub(crate) fn definition() -> &'static AcpDefinition {
        &CURSOR_ACP
    }

    /// Returns how this row carries image payloads: native image blocks,
    /// never embedded resources and never dropped.
    #[must_use]
    pub(crate) const fn image_mode() -> ImageMode {
        ImageMode::Image
    }
}

/// Finite C1 cursor launch capability.
///
/// Carries the managed profile identity plus the unprobed version sentinel
/// until the probe/authority packet lands. The dispatcher never constructs
/// this from a live probe in C1, and the dispatch arm fails closed before
/// spawning; the value exists so admission, binding (`cursor`, format 1), and
/// quarantine plumbing prove their cursor arms without claiming a runnable
/// engine.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct CursorLaunch {
    profile_id: String,
    version: String,
}

impl CursorLaunch {
    /// Creates an unprobed launch for exactly one managed profile.
    #[must_use]
    pub(crate) fn unprobed(profile_id: String) -> Self {
        Self {
            profile_id,
            version: CURSOR_C1_UNPROBED_VERSION.to_owned(),
        }
    }

    /// Returns the managed profile identity.
    #[must_use]
    pub(crate) fn profile_id(&self) -> &str {
        &self.profile_id
    }

    /// Returns the version string (the C1 sentinel until probed).
    #[must_use]
    pub(crate) fn version(&self) -> &str {
        &self.version
    }
}

/// Rejects a steer issued while a prompt round is active with the typed
/// unsupported-command failure, mirroring the shared ACP `Send` gate: a
/// waiting session accepts a follow-up, an active prompt cannot be steered in
/// place.
///
/// # Errors
///
/// Returns [`CursorTurnError::UnsupportedCommand`] when `prompt_active` is
/// set.
pub(crate) const fn check_cursor_steer(prompt_active: bool) -> Result<(), CursorTurnError> {
    if prompt_active {
        return Err(CursorTurnError::UnsupportedCommand);
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// C3: native continuation gate, resume, usage, and teardown contract
// ---------------------------------------------------------------------------

/// Minimum cursor CLI for native continuation: the certified agent release.
///
/// Mirrors `cursor_certified_version` in
/// `modules/engines/src/toolchain/distribution.ts`. The gate re-checks this
/// recorded constant against the probed launch version so a stale capability
/// can never authorize a resume the installed CLI no longer honors. Cursor
/// versions are dated (`YYYY.M.D-suffix`); comparison uses their numeric
/// core.
pub(crate) const CURSOR_CONTINUATION_MINIMUM_CLI_VERSION: &str = "2026.08.11-e8db854";

/// Returns whether cursor teardown must terminate the whole process group.
///
/// Always true: the cursor turn spawns through
/// [`spawn_acp_child`](super::super::acp::spawn_acp_child) with whole-group custody
/// (Job Object on Windows), so teardown kills cursor grandchildren that
/// still hold pipes instead of orphaning them. Unobserved reaps surface as
/// [`AcpShutdown::Retained`](super::super::acp::AcpShutdown) for owner quarantine.
pub(crate) const fn cursor_requires_group_termination() -> bool {
    true
}

/// Compares two dated cursor CLI spellings by their numeric core.
///
/// A leading agent name and any trailing `-suffix` are ignored, so
/// `agent 2026.9.6-stable.1` compares by `[2026, 9, 6]`. Returns `None` when
/// either side has no parseable dated triple; callers fail closed on `None`.
pub(crate) fn compare_cursor_cli_versions(left: &str, right: &str) -> Option<std::cmp::Ordering> {
    Some(parse_cursor_dated_triple(left)?.cmp(&parse_cursor_dated_triple(right)?))
}

/// Returns whether a probed CLI version meets a minimum floor.
pub(crate) fn cursor_cli_meets_minimum(version: &str, minimum: &str) -> bool {
    matches!(
        compare_cursor_cli_versions(version, minimum),
        Some(std::cmp::Ordering::Equal | std::cmp::Ordering::Greater)
    )
}

fn parse_cursor_dated_triple(text: &str) -> Option<[u64; 3]> {
    let start = text.find(|character: char| character.is_ascii_digit())?;
    let run: String = text[start..]
        .chars()
        .take_while(|character| character.is_ascii_digit() || *character == '.')
        .collect();
    let mut parts = run.split('.');
    Some([
        parts.next()?.parse().ok()?,
        parts.next()?.parse().ok()?,
        parts.next()?.parse().ok()?,
    ])
}

/// Native-continuation decision for one cursor turn.
///
/// `Compatible` authorizes `session/load` against the stored ACP session;
/// `Incompatible` carries the stable reason the dispatcher surfaces instead
/// of silently starting fresh or resuming across engines.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum CursorContinuationDecision {
    Compatible,
    Incompatible { reason: &'static str },
}

/// Bounded input for [`check_cursor_native_continuation`].
pub(crate) struct CursorContinuationGateInput<'a> {
    /// Probed CLI version (`CursorLaunch::version` once the probe/authority
    /// packet records it; the C1 unprobed sentinel fails the floor).
    pub cli_version: &'a str,
    /// Explicit target model from the current selection; `None` fails closed.
    pub target_model: Option<&'a str>,
    /// Advertised models when a model inventory was read; `None` skips
    /// advertisement validation (live inventory is deferred) but never skips
    /// the explicit-model or CLI gates.
    pub advertised_models: Option<&'a [&'a str]>,
    /// Whether the stored binding names the same `cursor` engine. The
    /// dispatcher scopes continuation reads to `EngineId::Cursor`, so a false
    /// value fails closed instead of resuming across engines.
    pub same_engine: bool,
}

/// Gates one cursor native continuation without touching provider state.
///
/// Order is contractual: same-engine first, then the explicit target model
/// (pre-validated before resume), then the certified CLI floor, then model
/// advertisement when an inventory is supplied. Any failure is a typed
/// incompatible — never a silent fresh start and never a cross-engine resume.
pub(crate) fn check_cursor_native_continuation(
    input: &CursorContinuationGateInput<'_>,
) -> CursorContinuationDecision {
    if !input.same_engine {
        return CursorContinuationDecision::Incompatible {
            reason: "Cursor native continuation cannot resume across engines",
        };
    }
    let Some(target) = input.target_model.filter(|model| !model.is_empty()) else {
        return CursorContinuationDecision::Incompatible {
            reason: "Cursor native continuation requires an explicit target model",
        };
    };
    if !cursor_cli_meets_minimum(input.cli_version, CURSOR_CONTINUATION_MINIMUM_CLI_VERSION) {
        return CursorContinuationDecision::Incompatible {
            reason: "Cursor native continuation requires CLI 2026.08.11-e8db854 or newer",
        };
    }
    if let Some(advertised) = input.advertised_models
        && !advertised.contains(&target)
    {
        return CursorContinuationDecision::Incompatible {
            reason: "Cursor does not currently advertise the target model",
        };
    }
    CursorContinuationDecision::Compatible
}

/// Reopens the stored ACP session for one authorized continuation.
///
/// Mirrors the TypeScript resume path (`session/load` with the stored session
/// id over the same cwd a fresh `session/new` would use): the resume reopens
/// provider-owned state only and never invents checkpoints. Returns `None`
/// when the stored session id is outside its bounded route-segment grammar
/// so the caller fails closed instead of resuming a corrupt session. The
/// load then requires the agent to acknowledge exactly this session.
pub(crate) fn cursor_resume_session_id(stored_session_id: &str) -> Option<String> {
    if stored_session_id.is_empty() || stored_session_id.len() > CURSOR_MAX_SESSION_ID_BYTES {
        return None;
    }
    Some(stored_session_id.to_owned())
}
