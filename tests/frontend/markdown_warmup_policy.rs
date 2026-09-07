//! Focused dependency-free coverage for settled Markdown renderer warmup.

#[path = "../../modules/frontend/src/markdown_warmup_policy.rs"]
mod markdown_warmup_policy;

use markdown_warmup_policy::{
    CONVERSATION_RENDERER_WARMUP_CHUNKS, IdleCapability, MARKDOWN_WARMUP_CHUNKS,
    RENDERER_IDLE_CALLBACK_TIMEOUT_MS, RENDERER_IDLE_FALLBACK_DELAY_MS, RendererChunkResult,
    RendererFirstUseDisposition, RendererIdleWait, RendererWarmup, RendererWarmupAction,
    RendererWarmupChunk, RendererWarmupChunkStatus, RendererWarmupEvent, RendererWarmupFailure,
    RendererWarmupState,
};

fn expected_wait(capability: IdleCapability) -> RendererIdleWait {
    match capability {
        IdleCapability::Available => RendererIdleWait::IdleCallback {
            timeout_ms: RENDERER_IDLE_CALLBACK_TIMEOUT_MS,
        },
        IdleCapability::Absent | IdleCapability::DetectionFailed => RendererIdleWait::PacedDelay {
            duration_ms: RENDERER_IDLE_FALLBACK_DELAY_MS,
        },
    }
}

fn finish_chunk(
    warmup: &mut RendererWarmup,
    chunk: RendererWarmupChunk,
    result: RendererChunkResult,
) -> RendererWarmupAction {
    assert_eq!(
        warmup.on_idle_wait_finished(),
        Some(RendererWarmupAction::LoadChunk { chunk })
    );
    warmup
        .on_chunk_finished(result)
        .expect("a loading chunk emits its next action")
}

fn finish_all_loaded(warmup: &mut RendererWarmup) {
    let mut action = warmup.start().expect("an unstarted warmup starts");
    for &chunk in &CONVERSATION_RENDERER_WARMUP_CHUNKS {
        assert_eq!(
            action,
            RendererWarmupAction::WaitForIdle {
                chunk,
                wait: expected_wait(warmup.idle_capability()),
            }
        );
        action = finish_chunk(warmup, chunk, RendererChunkResult::Loaded);
    }
    assert_eq!(action, RendererWarmupAction::Complete);
}

#[test]
fn chunks_are_exactly_ordered_and_have_stable_labels() {
    assert_eq!(CONVERSATION_RENDERER_WARMUP_CHUNKS, MARKDOWN_WARMUP_CHUNKS);
    assert_eq!(
        CONVERSATION_RENDERER_WARMUP_CHUNKS,
        [
            RendererWarmupChunk::SettledHighlighting,
            RendererWarmupChunk::MathRenderer,
            RendererWarmupChunk::MathStylesheet,
            RendererWarmupChunk::MermaidRenderer,
        ]
    );
    assert_eq!(
        CONVERSATION_RENDERER_WARMUP_CHUNKS.map(RendererWarmupChunk::label),
        [
            "settled highlighting",
            "math renderer",
            "math stylesheet",
            "Mermaid renderer",
        ]
    );
    assert_eq!(
        CONVERSATION_RENDERER_WARMUP_CHUNKS.map(RendererWarmupChunk::index),
        [0, 1, 2, 3]
    );
}

#[test]
fn supported_idle_callbacks_wait_once_before_each_ordered_load() {
    let mut warmup = RendererWarmup::new(IdleCapability::Available);
    let wait = expected_wait(IdleCapability::Available);
    let mut action = warmup.start().expect("an unstarted warmup starts");

    for (index, &chunk) in CONVERSATION_RENDERER_WARMUP_CHUNKS.iter().enumerate() {
        assert_eq!(action, RendererWarmupAction::WaitForIdle { chunk, wait });
        assert_eq!(
            warmup.state(),
            RendererWarmupState::WaitingForIdle { chunk, wait }
        );
        action = finish_chunk(
            &mut warmup,
            chunk,
            if index == 1 {
                RendererChunkResult::Failed
            } else {
                RendererChunkResult::Loaded
            },
        );
        if index + 1 < CONVERSATION_RENDERER_WARMUP_CHUNKS.len() {
            assert!(matches!(action, RendererWarmupAction::WaitForIdle { .. }));
        }
    }

    assert_eq!(action, RendererWarmupAction::Complete);
    assert_eq!(warmup.state(), RendererWarmupState::Completed);
}

#[test]
fn absent_and_failed_capability_inputs_use_one_second_pacing() {
    for capability in [IdleCapability::Absent, IdleCapability::DetectionFailed] {
        let mut warmup = RendererWarmup::new(capability);
        let wait = expected_wait(capability);
        let mut action = warmup.start().expect("an unstarted warmup starts");

        for &chunk in &CONVERSATION_RENDERER_WARMUP_CHUNKS {
            assert_eq!(action, RendererWarmupAction::WaitForIdle { chunk, wait });
            action = finish_chunk(&mut warmup, chunk, RendererChunkResult::Loaded);
        }

        assert_eq!(action, RendererWarmupAction::Complete);
        assert!(warmup.is_completed());
    }
    assert_eq!(
        RendererWarmup::new(true).idle_capability(),
        IdleCapability::Available
    );
    assert_eq!(
        RendererWarmup::new(false).idle_capability(),
        IdleCapability::Absent
    );
}

#[test]
fn every_failure_is_diagnostic_only_and_keeps_first_use_retry_eligible() {
    let mut warmup = RendererWarmup::new(true);
    let mut action = warmup.start().expect("an unstarted warmup starts");

    for &chunk in &CONVERSATION_RENDERER_WARMUP_CHUNKS {
        assert!(matches!(action, RendererWarmupAction::WaitForIdle { .. }));
        action = finish_chunk(&mut warmup, chunk, RendererChunkResult::Failed);
    }

    assert_eq!(action, RendererWarmupAction::Complete);
    assert_eq!(
        warmup.failures().len(),
        CONVERSATION_RENDERER_WARMUP_CHUNKS.len()
    );
    assert_eq!(
        warmup.failures(),
        &[
            RendererWarmupFailure {
                chunk: RendererWarmupChunk::SettledHighlighting,
            },
            RendererWarmupFailure {
                chunk: RendererWarmupChunk::MathRenderer,
            },
            RendererWarmupFailure {
                chunk: RendererWarmupChunk::MathStylesheet,
            },
            RendererWarmupFailure {
                chunk: RendererWarmupChunk::MermaidRenderer,
            },
        ]
    );

    for &chunk in &CONVERSATION_RENDERER_WARMUP_CHUNKS {
        assert_eq!(
            warmup.chunk_status(chunk),
            RendererWarmupChunkStatus::Failed
        );
        assert_eq!(
            warmup.first_use_disposition(chunk),
            RendererFirstUseDisposition::RetryFailedLoad
        );
        assert!(warmup.is_first_use_retry_eligible(chunk));
    }
}

#[test]
fn first_use_distinguishes_loaded_initial_and_failed_chunks() {
    let mut warmup = RendererWarmup::new(true);
    assert_eq!(
        warmup.first_use_disposition(RendererWarmupChunk::SettledHighlighting),
        RendererFirstUseDisposition::InitialLoad
    );

    let action = warmup.start().expect("an unstarted warmup starts");
    assert!(matches!(action, RendererWarmupAction::WaitForIdle { .. }));
    let mut action = finish_chunk(
        &mut warmup,
        RendererWarmupChunk::SettledHighlighting,
        RendererChunkResult::Loaded,
    );
    assert_eq!(
        warmup.first_use_disposition(RendererWarmupChunk::SettledHighlighting),
        RendererFirstUseDisposition::AlreadyWarmed
    );
    assert!(!warmup.is_first_use_retry_eligible(RendererWarmupChunk::SettledHighlighting));

    assert!(matches!(action, RendererWarmupAction::WaitForIdle { .. }));
    action = finish_chunk(
        &mut warmup,
        RendererWarmupChunk::MathRenderer,
        RendererChunkResult::Failed,
    );
    assert_eq!(
        warmup.first_use_disposition(RendererWarmupChunk::MathRenderer),
        RendererFirstUseDisposition::RetryFailedLoad
    );
    assert!(warmup.is_first_use_retry_eligible(RendererWarmupChunk::MathRenderer));
    assert!(matches!(action, RendererWarmupAction::WaitForIdle { .. }));
}

#[test]
fn cancellation_at_each_boundary_stops_future_actions_and_preserves_facts() {
    let mut not_started = RendererWarmup::new(true);
    assert_eq!(not_started.cancel(), Some(RendererWarmupAction::Cancelled));
    assert_eq!(not_started.state(), RendererWarmupState::Cancelled);
    assert_eq!(not_started.cancel(), None);
    assert_eq!(not_started.start(), None);

    let mut waiting = RendererWarmup::new(true);
    assert!(waiting.start().is_some());
    assert_eq!(waiting.cancel(), Some(RendererWarmupAction::Cancelled));
    assert_eq!(waiting.on_idle_wait_finished(), None);
    assert_eq!(waiting.on_chunk_finished(RendererChunkResult::Failed), None);

    let mut loading = RendererWarmup::new(true);
    assert!(loading.start().is_some());
    assert!(loading.on_idle_wait_finished().is_some());
    assert_eq!(loading.cancel(), Some(RendererWarmupAction::Cancelled));
    assert_eq!(loading.on_chunk_finished(RendererChunkResult::Loaded), None);
    assert_eq!(loading.failures(), &[]);
    assert_eq!(
        loading.chunk_status(RendererWarmupChunk::SettledHighlighting),
        RendererWarmupChunkStatus::NotAttempted
    );

    let mut between_chunks = RendererWarmup::new(true);
    assert!(between_chunks.start().is_some());
    let next = finish_chunk(
        &mut between_chunks,
        RendererWarmupChunk::SettledHighlighting,
        RendererChunkResult::Loaded,
    );
    assert!(matches!(next, RendererWarmupAction::WaitForIdle { .. }));
    let before_cancel = between_chunks.clone();
    assert_eq!(
        between_chunks.cancel(),
        Some(RendererWarmupAction::Cancelled)
    );
    assert_eq!(
        between_chunks.chunk_status(RendererWarmupChunk::SettledHighlighting),
        RendererWarmupChunkStatus::Loaded
    );
    assert_eq!(between_chunks.failures(), before_cancel.failures());
    assert_eq!(between_chunks.on_idle_wait_finished(), None);

    let mut after_failure = RendererWarmup::new(true);
    assert!(after_failure.start().is_some());
    let next = finish_chunk(
        &mut after_failure,
        RendererWarmupChunk::SettledHighlighting,
        RendererChunkResult::Failed,
    );
    assert!(matches!(next, RendererWarmupAction::WaitForIdle { .. }));
    assert_eq!(
        after_failure.cancel(),
        Some(RendererWarmupAction::Cancelled)
    );
    assert_eq!(
        after_failure.first_use_disposition(RendererWarmupChunk::SettledHighlighting),
        RendererFirstUseDisposition::RetryFailedLoad
    );
    assert_eq!(after_failure.failures().len(), 1);
    assert_eq!(after_failure.on_idle_wait_finished(), None);
}

#[test]
fn completed_and_cancelled_terminals_are_idempotent() {
    let mut completed = RendererWarmup::new(true);
    finish_all_loaded(&mut completed);
    assert!(completed.is_terminal());
    let facts = completed.clone();

    assert_eq!(completed.start(), None);
    assert_eq!(completed.on_idle_wait_finished(), None);
    assert_eq!(
        completed.on_chunk_finished(RendererChunkResult::Failed),
        None
    );
    assert_eq!(completed.cancel(), None);
    assert_eq!(completed.dispatch(RendererWarmupEvent::Start), None);
    assert_eq!(
        completed.dispatch(RendererWarmupEvent::ChunkFinished {
            result: RendererChunkResult::Failed,
        }),
        None
    );
    assert_eq!(completed, facts);

    let mut cancelled = RendererWarmup::new(true);
    assert!(cancelled.start().is_some());
    assert!(cancelled.cancel().is_some());
    assert!(cancelled.is_terminal());
    let facts = cancelled.clone();
    assert_eq!(cancelled.cancel(), None);
    assert_eq!(cancelled.start(), None);
    assert_eq!(cancelled.on_idle_wait_finished(), None);
    assert_eq!(
        cancelled.on_chunk_finished(RendererChunkResult::Loaded),
        None
    );
    assert_eq!(cancelled.dispatch(RendererWarmupEvent::Cancel), None);
    assert_eq!(cancelled, facts);
}

#[test]
fn dispatch_is_only_a_typed_scheduler_adapter() {
    let mut warmup = RendererWarmup::new(true);
    assert_eq!(
        warmup.dispatch(RendererWarmupEvent::Start),
        Some(RendererWarmupAction::WaitForIdle {
            chunk: RendererWarmupChunk::SettledHighlighting,
            wait: RendererIdleWait::IdleCallback {
                timeout_ms: RENDERER_IDLE_CALLBACK_TIMEOUT_MS,
            },
        })
    );
    assert_eq!(
        warmup.dispatch(RendererWarmupEvent::IdleWaitFinished),
        Some(RendererWarmupAction::LoadChunk {
            chunk: RendererWarmupChunk::SettledHighlighting,
        })
    );
    assert_eq!(
        warmup.dispatch(RendererWarmupEvent::ChunkFinished {
            result: RendererChunkResult::Loaded,
        }),
        Some(RendererWarmupAction::WaitForIdle {
            chunk: RendererWarmupChunk::MathRenderer,
            wait: RendererIdleWait::IdleCallback {
                timeout_ms: RENDERER_IDLE_CALLBACK_TIMEOUT_MS,
            },
        })
    );
    assert_eq!(
        warmup.dispatch(RendererWarmupEvent::Cancel),
        Some(RendererWarmupAction::Cancelled)
    );
    assert!(warmup.is_cancelled());
}
