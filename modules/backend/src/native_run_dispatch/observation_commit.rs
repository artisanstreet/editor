//! Durable subagent and rich-activity observation commits.
//!
//! Rebuilds source-local provider rows onto dispatcher-assigned identities and
//! run-local base-plus-one sequences, encodes them under the run bind, and
//! commits them through the shared S1b checkpoint batch path with a
//! content-neutral assistant projection. Any failure reports `false` so the
//! caller marks the turn interrupted with uncertain progress.

use artisan_database::{AssistantChange, Repository, RunBatchScope};
use artisan_domain::{
    AssistantBody, AssistantMessagePhase, EngineId, FileObservation, ItemId, Observation,
    ObservationId, ObservationSequence, PlanEntry, PlanObservation,
    ReasoningSummaryCompletedObservation, ReasoningSummaryDeltaObservation, Revision,
    SearchObservation, SubagentInput, SubagentObservation, SubagentTranscriptObservation,
    TerminalActivityInput, TerminalActivityObservation, ToolObservation,
};

use crate::{CommandOrigin, SystemCommandOrigin};

use super::{
    CommitBatchRequest, NativeRunDispatcherConfig, at_or_after, commit_batch_with_retry,
    mint_item_id, mint_patch_id,
};

/// Mutable S1b cursor for subagent observation commits.
///
/// Mirrors the resolution commit shape without disturbing root text: the
/// run-scoped batch fence plus the content-neutral assistant projection.
/// Only the commit core mutates the cursor; the dispatch arm copies the
/// settled cursor back onto the turn state.
pub(crate) struct SubagentCommitCursor<'a> {
    pub scope: RunBatchScope<'a>,
    pub engine: EngineId,
    pub batch_sequence: i64,
    pub assistant_item: Option<ItemId>,
    pub assistant_revision: Revision,
    pub assistant_body: String,
}

/// Commits one re-sequenced subagent observation through the S1b batch path.
///
/// Reads the durable base fresh for this batch and assigns base-plus-one, so
/// rows stay strictly increasing across batches regardless of owner stream
/// numbering. Returns whether the batch committed; the dispatch arm maps
/// failure onto run custody. Fixture coverage drives this same commit against
/// a real repository.
#[expect(
    clippy::too_many_lines,
    reason = "linear sequence of fallible commit steps sharing one mutable state; extraction would thread every binding"
)]
pub(crate) async fn commit_subagent_observation(
    repository: &Repository,
    config: &NativeRunDispatcherConfig,
    origin: &SystemCommandOrigin,
    cursor: &mut SubagentCommitCursor<'_>,
    observation: Observation,
) -> bool {
    let Ok(base) = repository
        .last_committed_observation_sequence(&cursor.scope.launched.run_id)
        .await
    else {
        return false;
    };
    let sequence_value = match base {
        None => 1,
        Some(maximum) => maximum.saturating_add(1),
    };
    let Ok(sequence) = ObservationSequence::new(sequence_value) else {
        return false;
    };
    let Ok(identity) = origin.mint_identity() else {
        return false;
    };
    let Ok(observation_id) = ObservationId::parse(identity) else {
        return false;
    };
    let Some(resequenced) =
        resequence_subagent_observation(&observation, &observation_id, sequence)
    else {
        return false;
    };
    let Ok(checkpoint) = artisan_database::encode_observation_checkpoint(
        cursor.engine,
        cursor.scope.bound.binding_version,
        base,
        &[resequenced],
    ) else {
        return false;
    };
    if artisan_database::validate_observation_bind(
        cursor.scope.bound.binding_version,
        cursor.scope.bound,
    )
    .is_err()
    {
        return false;
    }
    let Ok(body) = AssistantBody::parse(cursor.assistant_body.clone()) else {
        return false;
    };
    let Some(patch_id) = mint_patch_id(origin) else {
        return false;
    };
    let Some(operated_at) = at_or_after(origin, cursor.scope.expected_updated_at) else {
        return false;
    };
    if let Some(item_id) = cursor.assistant_item.clone() {
        let changes = [AssistantChange::Replace {
            item_id: &item_id,
            expected_revision: cursor.assistant_revision,
            body: &body,
            phase: AssistantMessagePhase::Unspecified,
            patch_id: &patch_id,
        }];
        if commit_batch_with_retry(CommitBatchRequest {
            repository,
            notifier: &config.notifier,
            scope: &cursor.scope,
            batch_sequence: cursor.batch_sequence,
            operated_at,
            activate_turn_patch_id: None,
            changes: &changes,
            checkpoint: artisan_database::CheckpointUpdate::Replace(&checkpoint),
            retries: config.max_command_retries,
        })
        .await
        .is_err()
        {
            return false;
        }
        let Ok(next_revision) = cursor.assistant_revision.checked_next() else {
            return false;
        };
        cursor.assistant_revision = next_revision;
    } else {
        let Some(item_id) = mint_item_id(origin) else {
            return false;
        };
        let Some(activation_patch_id) = mint_patch_id(origin) else {
            return false;
        };
        let changes = [AssistantChange::Start {
            item_id: &item_id,
            phase: AssistantMessagePhase::Unspecified,
            body: &body,
            patch_id: &patch_id,
        }];
        if commit_batch_with_retry(CommitBatchRequest {
            repository,
            notifier: &config.notifier,
            scope: &cursor.scope,
            batch_sequence: cursor.batch_sequence,
            operated_at,
            activate_turn_patch_id: Some(&activation_patch_id),
            changes: &changes,
            checkpoint: artisan_database::CheckpointUpdate::Replace(&checkpoint),
            retries: config.max_command_retries,
        })
        .await
        .is_err()
        {
            return false;
        }
        cursor.assistant_item = Some(item_id);
        cursor.assistant_revision = Revision::new(0);
    }
    let Some(next_sequence) = cursor.batch_sequence.checked_add(1) else {
        return false;
    };
    cursor.batch_sequence = next_sequence;
    cursor.scope.expected_updated_at = operated_at;
    true
}

/// Rebuilds one subagent row onto a dispatcher-assigned identity.
///
/// Provider stream numbering never crosses into durable history. Only
/// lifecycle and transcript rows rebuild here; any other row rejects.
fn resequence_subagent_observation(
    observation: &Observation,
    observation_id: &ObservationId,
    sequence: ObservationSequence,
) -> Option<Observation> {
    match observation {
        Observation::Subagent(row) => SubagentObservation::new(
            observation_id.clone(),
            sequence,
            SubagentInput {
                agent_native_thread_id: row.agent_native_thread_id().clone(),
                parent_native_thread_id: row.parent_native_thread_id().clone(),
                state: row.state(),
                activity: row.activity().map(str::to_owned),
                agent_path: row.agent_path().map(str::to_owned),
                turn_id: row.turn_id().cloned(),
            },
        )
        .ok()
        .map(Observation::Subagent),
        Observation::SubagentTranscript(row) => Some(Observation::SubagentTranscript(
            SubagentTranscriptObservation::new(
                observation_id.clone(),
                sequence,
                row.agent_native_thread_id().clone(),
                row.parent_native_thread_id().clone(),
                row.content().clone(),
            ),
        )),
        _ => None,
    }
}

/// Commits one re-sequenced rich activity observation through the S1b batch
/// path.
///
/// Mirrors [`commit_subagent_observation`] exactly (fresh run-local
/// base-plus-one, dispatcher-minted identity, checkpoint encode under the run
/// bind, content-neutral assistant change, `commit_batch_with_retry` with the
/// existing fencing/notifier): only the resequence vocabulary differs. The
/// thread-scoped `delivery_sequence` is assigned atomically by the database
/// from the launch receipt plus `operated_at`; the dispatcher never stamps it.
#[expect(
    clippy::too_many_lines,
    reason = "mirrors commit_subagent_observation: same linear fallible sequence over shared mutable state"
)]
pub(crate) async fn commit_activity_observation(
    repository: &Repository,
    config: &NativeRunDispatcherConfig,
    origin: &SystemCommandOrigin,
    cursor: &mut SubagentCommitCursor<'_>,
    observation: Observation,
) -> bool {
    let Ok(base) = repository
        .last_committed_observation_sequence(&cursor.scope.launched.run_id)
        .await
    else {
        return false;
    };
    let sequence_value = match base {
        None => 1,
        Some(maximum) => maximum.saturating_add(1),
    };
    let Ok(sequence) = ObservationSequence::new(sequence_value) else {
        return false;
    };
    let Ok(identity) = origin.mint_identity() else {
        return false;
    };
    let Ok(observation_id) = ObservationId::parse(identity) else {
        return false;
    };
    let Some(resequenced) =
        resequence_activity_observation(&observation, &observation_id, sequence)
    else {
        return false;
    };
    let Ok(checkpoint) = artisan_database::encode_observation_checkpoint(
        cursor.engine,
        cursor.scope.bound.binding_version,
        base,
        &[resequenced],
    ) else {
        return false;
    };
    if artisan_database::validate_observation_bind(
        cursor.scope.bound.binding_version,
        cursor.scope.bound,
    )
    .is_err()
    {
        return false;
    }
    let Ok(body) = AssistantBody::parse(cursor.assistant_body.clone()) else {
        return false;
    };
    let Some(patch_id) = mint_patch_id(origin) else {
        return false;
    };
    let Some(operated_at) = at_or_after(origin, cursor.scope.expected_updated_at) else {
        return false;
    };
    if let Some(item_id) = cursor.assistant_item.clone() {
        let changes = [AssistantChange::Replace {
            item_id: &item_id,
            expected_revision: cursor.assistant_revision,
            body: &body,
            phase: AssistantMessagePhase::Unspecified,
            patch_id: &patch_id,
        }];
        if commit_batch_with_retry(CommitBatchRequest {
            repository,
            notifier: &config.notifier,
            scope: &cursor.scope,
            batch_sequence: cursor.batch_sequence,
            operated_at,
            activate_turn_patch_id: None,
            changes: &changes,
            checkpoint: artisan_database::CheckpointUpdate::Replace(&checkpoint),
            retries: config.max_command_retries,
        })
        .await
        .is_err()
        {
            return false;
        }
        let Ok(next_revision) = cursor.assistant_revision.checked_next() else {
            return false;
        };
        cursor.assistant_revision = next_revision;
    } else {
        let Some(item_id) = mint_item_id(origin) else {
            return false;
        };
        let Some(activation_patch_id) = mint_patch_id(origin) else {
            return false;
        };
        let changes = [AssistantChange::Start {
            item_id: &item_id,
            phase: AssistantMessagePhase::Unspecified,
            body: &body,
            patch_id: &patch_id,
        }];
        if commit_batch_with_retry(CommitBatchRequest {
            repository,
            notifier: &config.notifier,
            scope: &cursor.scope,
            batch_sequence: cursor.batch_sequence,
            operated_at,
            activate_turn_patch_id: Some(&activation_patch_id),
            changes: &changes,
            checkpoint: artisan_database::CheckpointUpdate::Replace(&checkpoint),
            retries: config.max_command_retries,
        })
        .await
        .is_err()
        {
            return false;
        }
        cursor.assistant_item = Some(item_id);
        cursor.assistant_revision = Revision::new(0);
    }
    let Some(next_sequence) = cursor.batch_sequence.checked_add(1) else {
        return false;
    };
    cursor.batch_sequence = next_sequence;
    cursor.scope.expected_updated_at = operated_at;
    true
}

/// Rebuilds one rich activity row onto a dispatcher-assigned identity.
///
/// Source-local identity/sequence never cross into durable history: every
/// supported activity variant (reasoning-summary delta/completed, tool,
/// terminal activity, file, search, plan) is rebuilt with the dispatcher
/// base-plus-one run-local sequence and a fresh identity, preserving all
/// provider payload fields verbatim. Subagent rows stay on the existing
/// subagent path; any other row rejects so no discard or no-op commit can be
/// emitted.
fn resequence_activity_observation(
    observation: &Observation,
    observation_id: &ObservationId,
    sequence: ObservationSequence,
) -> Option<Observation> {
    match observation {
        Observation::ReasoningSummaryDelta(row) => ReasoningSummaryDeltaObservation::new(
            observation_id.clone(),
            sequence,
            row.item_id().clone(),
            row.summary_index(),
            row.delta().to_owned(),
            row.thinking_tokens(),
            row.turn_id().clone(),
        )
        .ok()
        .map(Observation::ReasoningSummaryDelta),
        Observation::ReasoningSummaryCompleted(row) => ReasoningSummaryCompletedObservation::new(
            observation_id.clone(),
            sequence,
            row.item_id().clone(),
            row.text().map(str::to_owned),
            row.turn_id().clone(),
        )
        .ok()
        .map(Observation::ReasoningSummaryCompleted),
        Observation::Tool(row) => ToolObservation::new(
            observation_id.clone(),
            sequence,
            row.tool_id().clone(),
            row.tool_name().to_owned(),
            row.action(),
            row.detail().map(str::to_owned),
        )
        .ok()
        .map(Observation::Tool),
        Observation::TerminalActivity(row) => TerminalActivityObservation::new(
            observation_id.clone(),
            sequence,
            TerminalActivityInput {
                activity_id: row.activity_id().clone(),
                channel: row.channel(),
                command: row.command().map(str::to_owned),
                shell: row.shell().map(str::to_owned),
                output: row.output().map(str::to_owned),
                exit_code: row.exit_code(),
                state: row.state(),
            },
        )
        .ok()
        .map(Observation::TerminalActivity),
        Observation::File(row) => FileObservation::new(
            observation_id.clone(),
            sequence,
            row.path().to_owned(),
            row.action(),
            row.lines_added(),
            row.lines_deleted(),
        )
        .ok()
        .map(Observation::File),
        Observation::Search(row) => SearchObservation::new(
            observation_id.clone(),
            sequence,
            row.query().to_owned(),
            row.scope(),
            row.search_id().cloned(),
            row.state(),
            row.result_count(),
        )
        .ok()
        .map(Observation::Search),
        Observation::Plan(row) => {
            let mut entries = Vec::with_capacity(row.entries().len());
            for entry in row.entries() {
                entries.push(
                    PlanEntry::new(entry.id().clone(), entry.status(), entry.text().to_owned())
                        .ok()?,
                );
            }
            // Rebuilding plan entries keeps provider entry ids: they are
            // renderer-scoped within the plan payload, while the observation
            // identity itself is freshly minted above.
            PlanObservation::new(
                observation_id.clone(),
                sequence,
                entries,
                row.turn_id().cloned(),
            )
            .ok()
            .map(Observation::Plan)
        }
        _ => None,
    }
}

#[cfg(test)]
mod activity_resequence_tests {
    use super::resequence_activity_observation;
    use artisan_domain::{
        FileAction, FileObservation, MessagePhase, Observation, ObservationId, ObservationSequence,
        PlanEntry, PlanEntryStatus, PlanObservation, ReasoningSummaryCompletedObservation,
        ReasoningSummaryDeltaObservation, SearchObservation, SearchState, TerminalActivityInput,
        TerminalActivityObservation, TerminalActivityState, ToolAction, ToolObservation,
    };

    fn observation_id(value: &str) -> ObservationId {
        ObservationId::parse(value).expect("fixture observation id is valid")
    }

    fn sequence(value: u64) -> ObservationSequence {
        ObservationSequence::new(value).expect("fixture sequence is valid")
    }

    #[test]
    fn rich_activity_rows_remint_identity_and_run_local_sequence() {
        let fresh_id = observation_id("dispatcher-minted-activity");
        let fresh_sequence = sequence(7);
        let source = Observation::Tool(
            ToolObservation::new(
                observation_id("source-tool-row"),
                sequence(41),
                observation_id("tool-source-1"),
                String::from("read"),
                ToolAction::Completed,
                Some(String::from("read 42 lines")),
            )
            .expect("fixture tool row is valid"),
        );
        let resequenced = resequence_activity_observation(&source, &fresh_id, fresh_sequence)
            .expect("tool activity must resequence");
        let Observation::Tool(row) = resequenced else {
            panic!("tool activity must stay a tool row");
        };
        assert_eq!(row.id(), &fresh_id);
        assert_eq!(row.sequence(), fresh_sequence);
        assert_eq!(row.tool_id().as_str(), "tool-source-1");
        assert_eq!(row.tool_name(), "read");
        assert_eq!(row.action(), ToolAction::Completed);
        assert_eq!(row.detail(), Some("read 42 lines"));
    }

    #[test]
    fn file_search_terminal_plan_and_reasoning_rows_preserve_payload() {
        let fresh_id = observation_id("dispatcher-minted-rich");
        let fresh_sequence = sequence(8);
        let rows = vec![
            Observation::File(
                FileObservation::new(
                    observation_id("source-file"),
                    sequence(42),
                    String::from("src/main.rs"),
                    FileAction::Modified,
                    Some(10),
                    Some(2),
                )
                .expect("fixture file row is valid"),
            ),
            Observation::Search(
                SearchObservation::new(
                    observation_id("source-search"),
                    sequence(43),
                    String::from("observation delivery"),
                    None,
                    None,
                    SearchState::Completed,
                    Some(7),
                )
                .expect("fixture search row is valid"),
            ),
            Observation::TerminalActivity(
                TerminalActivityObservation::new(
                    observation_id("source-terminal"),
                    sequence(44),
                    TerminalActivityInput {
                        activity_id: observation_id("activity-1"),
                        channel: None,
                        command: Some(String::from("cargo test")),
                        shell: None,
                        output: Some(String::from("test result: ok")),
                        exit_code: Some(0),
                        state: TerminalActivityState::Completed,
                    },
                )
                .expect("fixture terminal row is valid"),
            ),
            Observation::Plan(
                PlanObservation::new(
                    observation_id("source-plan"),
                    sequence(45),
                    vec![
                        PlanEntry::new(
                            observation_id("plan-entry-1"),
                            PlanEntryStatus::Completed,
                            String::from("Define the vocabulary"),
                        )
                        .expect("fixture plan entry is valid"),
                    ],
                    None,
                )
                .expect("fixture plan is valid"),
            ),
            Observation::ReasoningSummaryDelta(
                ReasoningSummaryDeltaObservation::new(
                    observation_id("source-reasoning-delta"),
                    sequence(46),
                    observation_id("item-2"),
                    3,
                    String::from("summary fragment"),
                    None,
                    observation_id("turn-1"),
                )
                .expect("fixture reasoning delta is valid"),
            ),
            Observation::ReasoningSummaryCompleted(
                ReasoningSummaryCompletedObservation::new(
                    observation_id("source-reasoning-completed"),
                    sequence(47),
                    observation_id("item-2"),
                    Some(String::from("public summary")),
                    observation_id("turn-1"),
                )
                .expect("fixture settled reasoning is valid"),
            ),
        ];
        for source in &rows {
            let resequenced = resequence_activity_observation(source, &fresh_id, fresh_sequence)
                .unwrap_or_else(|| panic!("{} must resequence", source.tag()));
            assert_eq!(resequenced.observation_id(), &fresh_id);
            assert_eq!(resequenced.sequence(), fresh_sequence);
            assert_eq!(resequenced.tag(), source.tag());
        }
        // Plan entry identities stay renderer-scoped while the observation
        // identity itself is freshly minted.
        let Observation::Plan(plan) =
            resequence_activity_observation(&rows[3], &fresh_id, fresh_sequence)
                .expect("plan activity must resequence")
        else {
            panic!("plan activity must stay a plan row");
        };
        assert_eq!(plan.entries().len(), 1);
        assert_eq!(plan.entries()[0].id().as_str(), "plan-entry-1");
    }

    #[test]
    fn unsupported_activity_rows_reject_without_a_no_op_commit() {
        let fresh_id = observation_id("dispatcher-minted-reject");
        let fresh_sequence = sequence(9);
        let usage = Observation::Usage(
            artisan_domain::UsageObservation::new(
                observation_id("source-usage"),
                sequence(48),
                artisan_domain::UsageInput {
                    basis: artisan_domain::UsageBasis::Cumulative,
                    input_tokens: Some(10),
                    cached_input_tokens: None,
                    output_tokens: Some(5),
                    context_tokens: None,
                    context_window_tokens: None,
                    cost_usd: None,
                    provider_route_id: None,
                    turn_id: None,
                },
            )
            .expect("fixture usage is valid"),
        );
        assert!(
            resequence_activity_observation(&usage, &fresh_id, fresh_sequence).is_none(),
            "usage must stay on its own commit path, never the activity path"
        );
        let message = Observation::AgentMessageCompleted(
            artisan_domain::AgentMessageCompletedObservation::new(
                observation_id("source-message"),
                sequence(49),
                observation_id("item-1"),
                MessagePhase::Final,
                String::from("settled reply"),
                observation_id("turn-1"),
            )
            .expect("fixture completed message is valid"),
        );
        assert!(
            resequence_activity_observation(&message, &fresh_id, fresh_sequence).is_none(),
            "transcript text must never enter the activity vocabulary"
        );
    }
}
