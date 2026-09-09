//! Finite G1 Grok runtime definition on the shared ACP core.
//!
//! This leaf owns the Grok-specific interpretation of the ACP transport core
//! (`super::acp`) plus the A2 bridges (`super::acp_bridges`): typed
//! [`GrokSettings`] derived from the durable [`GrokSelection`], the launch
//! args mapping with plan-mode forcing for read-only policies, the
//! definition-row accessor over the existing [`GROK_ACP`](super::acp::GROK_ACP)
//! row (args builder, version parser, auth classifier, embedded image mode),
//! the pending-launch capability ([`GrokLaunch`]), and the G1 command policy
//! (steer-while-active rejection, always-unsupported native continuation,
//! honest unavailability of provider usage, no provider-owned startup
//! classifier).
//!
//! Behavior mirrors `modules/engines/src/grok/engine.ts` over the shared
//! `MakeAcpEngine` core: default executable `"grok"`, `--no-auto-update`
//! first, optional `--model` / `--reasoning-effort`, plan mode when the
//! canonical policy denies writes, `auto` / `always-approve` permission
//! mapping, then `agent stdio`; `xai.api_key` when `XAI_API_KEY` is present
//! else `cached_token`; embedded `artisan://attachment` image blocks; a
//! waiting session accepts a follow-up as a new prompt while an active prompt
//! cannot be steered in place (`EngineUnsupportedCommandError` in
//! TypeScript); native continuation stays unsupported because ACP does not
//! guarantee that a loaded session may change model identity.
//!
//! Explicit non-goals for G1: catalog support (the catalog leaf stays
//! OpenCode2-only), frontend selection, provider usage surfacing (honest
//! unknown/unsupported), a verified-launch authority in `native_engine`
//! (the dispatcher probes with the existing discovery plus version parse and
//! seats the resolved path here), streaming text projection onto the S1a
//! vocabulary (a later packet), and live answer delivery into the provider
//! session (same as X1: tracked, never auto-answered).
//!
//! [`GrokSelection`]: artisan_domain::GrokSelection

#![forbid(unsafe_code)]

use std::path::{Path, PathBuf};

use artisan_domain::{EngineProfileId, FilesystemAccess, GrokSelection};
use thiserror::Error;

use super::acp::{AcpDefinition, GROK_ACP, LaunchArgs};

/// Structural ceiling for one resumed Grok session identity, mirroring the
/// owner continuation bound the ACP core already enforces.
pub(crate) const GROK_MAX_SESSION_ID_BYTES: usize = 256;

/// Ceiling for tracked permission/elicitation requests on one live Grok turn.
/// Matches the per-frame question ceiling the sibling runtimes use.
pub(crate) const GROK_MAX_PENDING_REQUESTS: usize = 32;

/// Agent method carrying one Grok permission request on the shared wire,
/// mirroring the TypeScript `session/requestPermission` evidence.
pub(crate) const GROK_PERMISSION_METHOD: &str = "session/requestPermission";

/// Agent method carrying one Grok elicitation request on the shared wire,
/// mirroring the TypeScript `elicitation/create` evidence.
pub(crate) const GROK_ELICITATION_METHOD: &str = "elicitation/create";

/// Typed Grok settings derived from the durable selection.
///
/// Mirrors `GrokAcpArgs` in `modules/engines/src/grok/engine.ts`: the model
/// and reasoning effort pass through verbatim when selected, the permission
/// mode passes through as its adapter spelling, and write access derives
/// from the canonical filesystem policy (anything but `None` may write, so
/// only `None` forces plan mode at argument-building time). The adapter
/// imposes no construction-time permission rejections beyond the typed
/// options, so construction here is infallible.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct GrokSettings {
    profile_id: String,
    model: Option<String>,
    reasoning_effort: Option<String>,
    permission_mode: Option<String>,
    write_access: bool,
}

impl GrokSettings {
    /// Derives typed ACP settings from the durable selection.
    #[must_use = "settings must reach the definition row"]
    pub(crate) fn from_selection(selection: &GrokSelection) -> Self {
        Self {
            profile_id: selection.profile_id().as_str().to_owned(),
            model: selection.model_id().map(|model| model.as_str().to_owned()),
            reasoning_effort: selection
                .reasoning_effort()
                .map(|effort| effort.as_str().to_owned()),
            permission_mode: selection
                .permission_mode()
                .map(|mode| mode.as_str().to_owned()),
            write_access: selection.permission().filesystem() != FilesystemAccess::None,
        }
    }

    /// Returns the managed profile identity.
    #[must_use]
    pub(crate) fn profile_id(&self) -> &str {
        &self.profile_id
    }

    /// Builds the explicit launch params the row's arg builder interprets.
    /// Cursor-only speed forcing stays off; the row maps the stored
    /// permission spelling and forces plan mode when writes are denied.
    #[must_use = "launch args must reach the spawn call"]
    pub(crate) fn launch_args(&self) -> LaunchArgs {
        LaunchArgs {
            model: self.model.clone(),
            reasoning_effort: self.reasoning_effort.clone(),
            speed_fast: false,
            permission: self.permission_mode.clone(),
            write_access: self.write_access,
        }
    }

    /// Returns the shared-core definition row for Grok Build: `grok`
    /// executable, embedded image blocks, the TypeScript version and auth
    /// classifiers, and the arg builder above.
    #[must_use = "definition rows must drive the dispatch arm"]
    pub(crate) fn definition(&self) -> AcpDefinition {
        GROK_ACP
    }
}

/// Pending Grok launch capability seated by the dispatcher probe.
///
/// Carries the resolved executable path, the exact selected profile, and the
/// probed CLI version. There is no verified-launch authority for Grok yet:
/// the dispatcher certifies the path as a regular file plus a parsed
/// `--version` at claim time, and the dispatch arm rechecks the profile
/// fence before spawn. Revalidation at spawn time is a later packet.
#[derive(Clone, Eq, PartialEq)]
pub(crate) struct GrokLaunch {
    executable: PathBuf,
    profile_id: EngineProfileId,
    version: String,
}

impl GrokLaunch {
    /// Seats a probe-certified launch capability.
    #[must_use = "launch capabilities must reach the owner queue"]
    pub(crate) fn new(executable: PathBuf, profile_id: EngineProfileId, version: String) -> Self {
        Self {
            executable,
            profile_id,
            version,
        }
    }

    /// Returns the exact selected profile identity.
    #[must_use]
    pub(crate) fn profile_id(&self) -> &EngineProfileId {
        &self.profile_id
    }

    /// Returns the resolved executable path.
    #[must_use]
    pub(crate) fn executable_path(&self) -> &Path {
        &self.executable
    }

    /// Returns the probed CLI version.
    #[must_use]
    pub(crate) fn version(&self) -> &str {
        &self.version
    }
}

impl std::fmt::Debug for GrokLaunch {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("GrokLaunch { <redacted> }")
    }
}

/// Returns whether ambient `XAI_API_KEY` credentials are present for the
/// row's auth classifier, mirroring the TypeScript `AuthMethod` evidence
/// reading `process.env`. The value itself is never read here.
#[must_use = "auth presence must gate the handshake"]
pub(crate) fn api_key_present() -> bool {
    std::env::var_os(artisan_native_engine::grok::XAI_API_KEY_ENV)
        .is_some_and(|value| !value.is_empty())
}

/// Typed, payload-free failure for Grok commands outside the G1 vocabulary.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
#[allow(dead_code)]
pub(crate) enum GrokCommandError {
    /// A steer arrived while a prompt was active. Waiting-only follow-up is
    /// a new prompt, never a fake in-place steer.
    #[error("grok command `{command}` is unsupported while a prompt is active")]
    UnsupportedCommand { command: &'static str },
}

/// How a waiting Grok session accepts follow-up text: exactly one new
/// prompt round, mirroring the TypeScript waiting-only branch.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[allow(dead_code)]
pub(crate) enum GrokFollowUp {
    /// Start a new prompt round on the waiting session.
    NewPrompt,
}

/// Decides whether follow-up text may be sent.
///
/// An active prompt rejects with typed [`GrokCommandError::UnsupportedCommand`];
/// a waiting session accepts exactly [`GrokFollowUp::NewPrompt`]. Live steer
/// delivery wiring is a later packet; this decision is exercised by tests.
///
/// # Errors
///
/// Returns [`GrokCommandError::UnsupportedCommand`] when `prompt_active`.
#[allow(dead_code)]
pub(crate) fn follow_up(prompt_active: bool) -> Result<GrokFollowUp, GrokCommandError> {
    if prompt_active {
        return Err(GrokCommandError::UnsupportedCommand { command: "steer" });
    }
    Ok(GrokFollowUp::NewPrompt)
}

/// Honest reason for the always-unsupported native continuation, mirroring
/// the TypeScript descriptor: ACP does not guarantee that a loaded session
/// may change model identity. Same-model resume through `session/load`
/// stays a transport capability and is unaffected.
#[allow(dead_code)]
pub(crate) const GROK_NATIVE_CONTINUATION_REASON: &str =
    "ACP does not guarantee that a loaded session may change model identity.";

/// Typed, payload-free failure for Grok native continuation checks.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
#[allow(dead_code)]
pub(crate) enum GrokContinuationError {
    /// Native continuation is unsupported for Grok in every release.
    #[error("grok native continuation unsupported: {reason}")]
    Unsupported { reason: &'static str },
}

/// Checks native continuation for Grok: always unsupported.
///
/// Same-engine model changes have no native path on the ACP transport, so
/// this fails closed instead of resuming as another engine. Model-change
/// gating for other engines is unrelated and untouched.
///
/// # Errors
///
/// Always returns [`GrokContinuationError::Unsupported`].
#[allow(dead_code)]
pub(crate) fn check_native_continuation() -> Result<(), GrokContinuationError> {
    Err(GrokContinuationError::Unsupported {
        reason: GROK_NATIVE_CONTINUATION_REASON,
    })
}

/// Honest reason for the missing Grok usage surface: prompt usage parses at
/// the transport boundary but G1 carries no attribution or emission for it.
#[allow(dead_code)]
pub(crate) const GROK_USAGE_UNAVAILABLE_REASON: &str =
    "Grok provider usage is not surfaced in this packet.";

/// Whether the Grok provider-owned usage surface is available.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[allow(dead_code)]
pub(crate) enum GrokUsageSurface {
    /// No usage surface in G1; honest unknown/unsupported.
    Unsupported,
}

impl GrokUsageSurface {
    /// Returns the truthful unavailable reason.
    #[must_use]
    #[allow(dead_code)]
    pub(crate) const fn reason(self) -> &'static str {
        match self {
            Self::Unsupported => GROK_USAGE_UNAVAILABLE_REASON,
        }
    }
}

/// Reports the Grok usage surface: always unsupported in G1.
///
/// # Panics
///
/// Never panics; the surface is a constant decision.
#[allow(dead_code)]
pub(crate) fn usage_surface() -> GrokUsageSurface {
    GrokUsageSurface::Unsupported
}

/// Classifies a bounded provider diagnostic emitted while Grok ACP is
/// becoming ready.
///
/// The TypeScript Grok definition carries no `ClassifyStartupFailure`, so
/// this always declines: the dispatch arm maps startup failures onto the
/// generic owner vocabulary instead of inventing provider-owned cases.
#[allow(dead_code)]
pub(crate) fn classify_startup_failure(
    _operation: &'static str,
    _stderr_tail: &str,
) -> Option<super::operation::EngineOperationError> {
    None
}
