//! Cursor executor seam: one finite Cursor turn over the shared ACP core.

use std::sync::Arc;

use artisan_transport::CancelHandle;

use super::core::EngineOperationError;
use super::core::Execution;
use super::turn_common::ConfiguredRuntime;
use super::turn_common::ConfiguredTurnRequest;

/// Executes one finite Cursor turn over the shared ACP core.
///
/// Single-owner match arm beside the Codex and Claude executors: no second
/// task, no second queue. C3 proves admission agreement (durable cursor
/// selection, typed [`CursorSettings`](super::super::cursor::CursorSettings), and
/// the cursor launch capability carry the same managed profile) and gates a
/// provider continuation through the C3 gate (same engine, explicit target
/// model, CLI >= 2026.08.11-e8db854; anything else is typed incompatible),
/// then still fails closed: the probe authority, live spawn/pump, catalog
/// merge, and frontend selection belong to later packets. The cursor-shaped
/// ACP wire itself is proven by the fixture script tests in `super::super::cursor`
/// and `tests/backend/engine_owner_cursor.rs`, which drive the exact
/// definition row (`--model` resolution, `--mode ask`, `--force`, `acp`;
/// image-block mode; permission deny-then-allow; plan-approval extensions;
/// resume; cancel/close; malformed frames; `AE-PROVIDER-206`) through the
/// shared transport core without spawning the real CLI.
pub(super) async fn execute_cursor_turn(
    request: ConfiguredTurnRequest,
    _runtime: ConfiguredRuntime,
    _shutdown: &Arc<CancelHandle>,
) -> Execution {
    use super::super::cursor as cursor_runtime;

    let artisan_domain::EngineSelection::Cursor(selection) =
        request.input.settings.config().selection()
    else {
        return request.fail(EngineOperationError::Configuration);
    };
    let settings = cursor_runtime::CursorSettings::from_selection(selection);
    if settings.profile_id() != request.input.launch.profile_id() {
        return request.fail(EngineOperationError::Configuration);
    }
    let super::super::InternalLaunch::Cursor(launch) = &request.input.launch else {
        return request.fail(EngineOperationError::Configuration);
    };
    if launch.profile_id() != settings.profile_id() {
        return request.fail(EngineOperationError::Configuration);
    }
    // C3 continuation gate: same-engine is fenced by the dispatcher (cursor
    // bindings only); the owner additionally requires an explicit target
    // model and CLI >= 2026.08.11-e8db854, plus a bounded stored session id.
    // Anything else is typed incompatible — never a silent fresh start and
    // never a cross-engine resume. The validated session id is consumed by
    // the live `session/load` resume once the runnable packet lands; C3 still
    // fails closed before spawning.
    if let Some(continuation) = request.input.continuation.as_ref() {
        let gate = cursor_runtime::check_cursor_native_continuation(
            &cursor_runtime::CursorContinuationGateInput {
                cli_version: request.input.launch.version(),
                target_model: selection.model_id().map(artisan_domain::EngineModelId::as_str),
                advertised_models: None,
                same_engine: true,
            },
        );
        if !matches!(gate, cursor_runtime::CursorContinuationDecision::Compatible) {
            return request.fail(EngineOperationError::Configuration);
        }
        if cursor_runtime::cursor_resume_session_id(continuation.provider_session_id()).is_none() {
            return request.fail(EngineOperationError::Configuration);
        }
    }
    let _definition = cursor_runtime::CursorSettings::definition();
    request.fail(EngineOperationError::Configuration)
}
