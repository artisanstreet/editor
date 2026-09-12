//! Private orchestration for the engine owner: the single owner task,
//! generation allocation, per-operation execution, and the quarantine tail.
//!
//! One task owns at most one active engine child at a time — its exact
//! [`tokio::process::Child`], the taken sole stdin lifeline writer, the
//! stderr counting state, the burned generation, and every cleanup decision.
//! Work arrives through a bounded channel and is processed strictly
//! sequentially; there is no per-job task, no parallel owner, and no
//! replacement child before an observed reap.
//!
//! Fixed precedence, re-checked at the top of every scheduling cycle:
//! owner shutdown or terminal state, abandonment or explicit cancellation,
//! the operation deadline, and only then completion sources. Once a cleanup
//! sequence starts it runs to completion regardless of those signals.
//!
//! P3 adds bounded child readiness parsing and a bounded authenticated
//! HTTP/1 health handshake after spawn. Readiness is exactly one
//! newline-terminated `{"url": "..."}` record capped via `cap + 1`; health
//! is one `GET /api/health` with `Basic base64(opencode:<secret>)` over a
//! Hyper `TokioIo<TcpStream>` connection configured with caller-supplied
//! `max_headers` and `max_buf_bytes` and body-bounded via `Limited`.

mod bootstrap;
mod claude;
mod codex;
mod core;
mod cursor;
mod failures;
mod grok;
mod hermes;
mod lifecycle;
mod opencode;
mod owner;
mod turn_common;

// Vocabulary and handoff types re-exported unchanged for the rest of the
// engine owner and the `#[path]` suites.
#[allow(unused_imports)]
pub(crate) use self::core::{
    AcceptedCatalog, AcceptedLaunch, AcceptedPreflight, AcceptedTurn, CatalogOperationResult,
    EngineOperationError, EngineTurnResult, Execution, GenerationAllocator, HealthState, Job,
    LaunchAdmissionError, LaunchOutcome, LaunchResult, PreflightReap, PreflightReceipt,
    PreflightResult, PreparedSession, STEER_CHANNEL_CAPACITY, SteerDelivery, SteerError,
    TurnResult,
};

// Steer helpers re-exported so the `engine_owner::operation::*` paths keep
// resolving for the `#[path]` engine-owner suites.
#[allow(unused_imports)]
pub(crate) use self::claude::service_claude_steer_delivery;
#[allow(unused_imports)]
pub(crate) use self::codex::{
    ack_codex_steer_response, codex_response_id_matches, codex_resumed_thread_id, codex_thread_id,
    codex_turn_id, is_codex_result_for, service_codex_steer_delivery,
};
#[allow(unused_imports)]
pub(crate) use self::hermes::service_hermes_steer_delivery;

// Owner entry points re-exported for `engine_owner::mod` and the seeded owner
// tests.
#[allow(unused_imports)]
pub(crate) use self::owner::{run_configured_owner, run_owner, run_owner_with_allocator};

// Intake helpers used only by the tests below.
#[allow(unused_imports)]
use self::hermes::map_hermes_turn_error;
#[allow(unused_imports)]
use self::lifecycle::check_turn_attachment_applicability;

#[cfg(test)]
mod intake_attachment_tests {
    use artisan_domain::{AuthoredText, EngineId, ImageAttachment, QueueMessagePayload};

    use super::*;

    fn text_prompt() -> QueueMessagePayload {
        QueueMessagePayload::text_only("hello").expect("text prompt builds")
    }

    fn image_prompt() -> QueueMessagePayload {
        let attachment = ImageAttachment::new("image/png", vec![1, 2, 3, 4], "shot.png")
            .expect("image attachment builds");
        QueueMessagePayload::new(
            Some(AuthoredText::parse("see this").expect("authored text parses")),
            vec![attachment],
        )
        .expect("image prompt builds")
    }

    #[test]
    fn text_prompts_pass_intake_for_every_engine() {
        for engine in EngineId::ALL {
            assert!(
                check_turn_attachment_applicability(engine, &text_prompt()).is_ok(),
                "{engine:?} admits a text-only turn"
            );
        }
    }

    #[test]
    fn image_prompts_pass_except_hermes() {
        for engine in [
            EngineId::OpenCode2,
            EngineId::Codex,
            EngineId::Claude,
            EngineId::Grok,
            EngineId::Cursor,
        ] {
            assert!(
                check_turn_attachment_applicability(engine, &image_prompt()).is_ok(),
                "{engine:?} supports provider images at intake"
            );
        }
    }

    #[test]
    fn hermes_images_fail_closed_with_the_typed_reject() {
        assert_eq!(
            check_turn_attachment_applicability(EngineId::Hermes, &image_prompt()),
            Err(EngineOperationError::Configuration)
        );
        assert_eq!(
            map_hermes_turn_error(&crate::engine_owner::hermes::HermesTurnError::ImagesUnsupported),
            EngineOperationError::Configuration
        );
        assert!(check_turn_attachment_applicability(EngineId::Hermes, &text_prompt()).is_ok());
    }
}
