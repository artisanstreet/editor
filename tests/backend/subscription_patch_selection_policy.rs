//! Exhaustive focused coverage for subscription patch-selection policy.

#![allow(dead_code)]
#![forbid(unsafe_code)]

#[path = "../../modules/backend/src/subscription_patch_selection_policy.rs"]
mod subscription_patch_selection_policy;

use subscription_patch_selection_policy::{
    EventEnvelope, EventPayload, GRAPH_GROUP_EVENT_TYPES, PROVIDER_PROJECTION_GRAPH_ACTIONS,
    ProjectAffinityEvidenceCount, ProjectAffinityEvidenceKind, ProjectAffinityScore, ProjectRef,
    RawOrigin, SURFACE_EVENT_TYPES, SubscriptionPatchSelectionPolicy, TRANSCRIPT_EVENT_TYPES,
    ThreadListItem, ThreadListProjectionPatch, ThreadProjectRehomeSuggestion, ThreadTitleSource,
    WORKSPACE_CONFLICT_EVENT_TYPES, direct_thread_list_patch, event_affects_surface,
    event_affects_transcript, event_affects_workspace_conflicts, graph_group_id,
    is_provider_projection_event,
};

const EVENT_THREAD_ID: &str = "thread:envelope";
const EVENT_SENT_AT: &str = "2026-08-28T12:34:56.789Z";
const GROUP_ID: &str = "group:one";

fn event(payload: EventPayload) -> EventEnvelope {
    EventEnvelope::new(EVENT_THREAD_ID, EVENT_SENT_AT, payload)
}

fn provider_event(payload: EventPayload) -> EventEnvelope {
    event(payload).with_raw_origin(RawOrigin::new("provider:one", "reference:one"))
}

fn projection() -> ThreadListItem {
    let mut projection = ThreadListItem::initialized(
        "thread:projection",
        "Projection title",
        "2026-08-28T11:00:00.001Z",
    );
    projection.archived_at = Some("2026-08-28T11:01:00.002Z".to_owned());
    projection.engine_id = Some("engine:one".to_owned());
    projection.model_id = Some("model:one".to_owned());
    projection.last_assistant_message = Some("A bounded preview".to_owned());
    projection.reader_acknowledged_activity_at = Some("2026-08-28T11:02:00.003Z".to_owned());
    projection.primary_project = Some(ProjectRef {
        display_name: "Primary".to_owned(),
        project_id: "project:primary".to_owned(),
        root_path: "C:/workspace/primary".to_owned(),
    });
    projection.project_affinity_scores = vec![ProjectAffinityScore {
        evidence: vec![ProjectAffinityEvidenceCount {
            count: 2,
            kind: ProjectAffinityEvidenceKind::ThreadMetadata,
        }],
        project: ProjectRef {
            display_name: "Candidate".to_owned(),
            project_id: "project:candidate".to_owned(),
            root_path: "C:/workspace/candidate".to_owned(),
        },
        score: 87,
    }];
    projection.project_locked = true;
    projection.rename_suggestion = Some("A better title".to_owned());
    projection.rehome_suggestion = Some(ThreadProjectRehomeSuggestion {
        basis_affinity_version: 4,
        project: ProjectRef {
            display_name: "Rehome".to_owned(),
            project_id: "project:rehome".to_owned(),
            root_path: "C:/workspace/rehome".to_owned(),
        },
        score: 91,
    });
    projection.linked_projects = vec![ProjectRef {
        display_name: "Linked".to_owned(),
        project_id: "project:linked".to_owned(),
        root_path: "C:/workspace/linked".to_owned(),
    }];
    projection.summary_title = Some("Generated summary".to_owned());
    projection.title_locked = true;
    projection.title_source = ThreadTitleSource::Manual;
    projection
}

fn all_payloads() -> Vec<EventPayload> {
    vec![
        EventPayload::ThreadCreated {
            title: "Created".to_owned(),
        },
        EventPayload::ThreadContentErased,
        EventPayload::ThreadErased,
        EventPayload::ThreadAttentionAcknowledged,
        EventPayload::ThreadMetadataUpdated {
            thread: projection(),
        },
        EventPayload::ThreadRefinementIgnored,
        EventPayload::ThreadProjectAffinityUpdated {
            thread: projection(),
        },
        EventPayload::ThreadProjectAffinityIgnored,
        EventPayload::ThreadRetentionPolicyUpdated,
        EventPayload::ModelFavoritesUpdated,
        EventPayload::SessionDefaultsUpdated,
        EventPayload::UsageInterruptionUpdated,
        EventPayload::GlobalGuidanceCanonicalUpdated,
        EventPayload::GlobalGuidanceSelectionRequired,
        EventPayload::GlobalGuidanceProviderReconciled,
        EventPayload::ModelBehaviourSettingUpdated,
        EventPayload::ModelBehaviourProviderReconciled,
        EventPayload::MarketplaceLedger,
        EventPayload::WorkspaceChangeUpdated,
        EventPayload::WorkspaceConflictUpdated,
        EventPayload::ThreadMessageQueued,
        EventPayload::ThreadMessageSteering,
        EventPayload::ThreadMessageRouted,
        EventPayload::RunLifecycle,
        EventPayload::AssistantMessageCompleted,
        EventPayload::InteractionApproval,
        EventPayload::InteractionQuestion,
        EventPayload::IntakeAssessed,
        EventPayload::IntakeAssumptionRecorded,
        EventPayload::ThreadAutoSteerUpdated,
        EventPayload::ThreadSessionPolicyUpdated,
        EventPayload::FilesystemMutation,
        EventPayload::ProcessOwnership,
        EventPayload::GitWorkspaceObserved,
        EventPayload::GitWorkspaceUpdated,
        EventPayload::GitMutationUpdated,
        EventPayload::TerminalLifecycle,
        EventPayload::OrchestrationGraphLifecycle {
            group_id: GROUP_ID.to_owned(),
            action: "finished".to_owned(),
        },
        EventPayload::AssignmentHeartbeat {
            group_id: GROUP_ID.to_owned(),
        },
        EventPayload::AgentInstanceRenamed {
            group_id: GROUP_ID.to_owned(),
        },
        EventPayload::AssignmentControl {
            group_id: GROUP_ID.to_owned(),
        },
        EventPayload::ArtifactRecorded {
            group_id: GROUP_ID.to_owned(),
        },
        EventPayload::ArtisanToolInvocationUpdated,
        EventPayload::ArtisanApprovalUpdated,
        EventPayload::ArtisanAssumptionRecorded,
        EventPayload::ArtisanNativeAction,
        EventPayload::PreviewTargetUpdated,
        EventPayload::PreviewInspectionSessionUpdated,
        EventPayload::ThreadModelTransition,
        EventPayload::other("future.event"),
    ]
}

#[test]
fn event_payload_table_covers_the_durable_vocabulary_and_exact_type_spellings() {
    let expected = [
        "thread.created",
        "thread.content_erased",
        "thread.erased",
        "thread.attention.acknowledged",
        "thread.metadata.updated",
        "thread.refinement.ignored",
        "thread.project_affinity.updated",
        "thread.project_affinity.ignored",
        "thread.retention.updated",
        "model.favorites.updated",
        "session.defaults.updated",
        "usage.interruption.updated",
        "guidance.canonical.updated",
        "guidance.selection.required",
        "guidance.provider.reconciled",
        "model_behaviour.setting.updated",
        "model_behaviour.provider.reconciled",
        "marketplace.lifecycle",
        "workspace.change.updated",
        "workspace.conflict.updated",
        "thread.message_queued",
        "thread.message_steering",
        "thread.message_routed",
        "run.lifecycle",
        "assistant.message_completed",
        "interaction.approval",
        "interaction.question",
        "intake.assessed",
        "intake.assumption_recorded",
        "thread.auto_steer.updated",
        "thread.session_policy.updated",
        "filesystem.mutation",
        "process.ownership",
        "git.workspace.observed",
        "git.workspace.updated",
        "git.mutation.updated",
        "terminal.lifecycle",
        "orchestration.graph.lifecycle",
        "assignment.heartbeat",
        "agent_instance.renamed",
        "assignment.control",
        "artifact.recorded",
        "artisan.tool.invocation.updated",
        "artisan.approval.updated",
        "artisan.assumption.recorded",
        "engine.native_action",
        "preview.target.updated",
        "preview.inspection.updated",
        "thread.model_transition",
        "future.event",
    ];
    let payloads = all_payloads();
    let actual = payloads
        .iter()
        .map(EventPayload::type_name)
        .collect::<Vec<_>>();

    assert_eq!(actual, expected);
}

#[test]
fn thread_created_initializes_every_projection_field_exactly() {
    let title = "Initial 🦀 title";
    let sent_at = "2026-08-28T12:34:56.789Z";
    let created = event(EventPayload::thread_created(title));

    let expected = ThreadListItem {
        activity_version: 0,
        affinity_version: 0,
        archived_at: None,
        created_at: sent_at.to_owned(),
        current_goal: Some(title.to_owned()),
        engine_id: None,
        model_id: None,
        last_activity_at: sent_at.to_owned(),
        last_message_at: Some(sent_at.to_owned()),
        last_assistant_message: None,
        live_status: "Idle".to_owned(),
        metadata_version: 0,
        pinned: false,
        reader_activity_at: Some(sent_at.to_owned()),
        reader_acknowledged_activity_at: None,
        primary_project: None,
        project_affinity_scores: Vec::new(),
        project_locked: false,
        rename_suggestion: None,
        rehome_suggestion: None,
        linked_projects: Vec::new(),
        summary_title: None,
        thread_id: EVENT_THREAD_ID.to_owned(),
        title: title.to_owned(),
        title_locked: false,
        title_source: ThreadTitleSource::Initial,
        updated_at: sent_at.to_owned(),
    };

    assert_eq!(
        direct_thread_list_patch(&created),
        Some(ThreadListProjectionPatch::Upsert { thread: expected })
    );
}

#[test]
fn direct_thread_list_patch_has_exact_remove_and_upsert_selection() {
    let supplied_projection = projection();
    let cases = [
        (
            EventPayload::ThreadErased,
            Some(ThreadListProjectionPatch::Remove {
                thread_id: EVENT_THREAD_ID.to_owned(),
            }),
        ),
        (
            EventPayload::ThreadMetadataUpdated {
                thread: supplied_projection.clone(),
            },
            Some(ThreadListProjectionPatch::Upsert {
                thread: supplied_projection.clone(),
            }),
        ),
        (
            EventPayload::ThreadProjectAffinityUpdated {
                thread: supplied_projection.clone(),
            },
            Some(ThreadListProjectionPatch::Upsert {
                thread: supplied_projection.clone(),
            }),
        ),
    ];

    for (payload, expected) in cases {
        assert_eq!(direct_thread_list_patch(&event(payload)), expected);
    }

    for payload in all_payloads() {
        if matches!(
            &payload,
            EventPayload::ThreadCreated { .. }
                | EventPayload::ThreadErased
                | EventPayload::ThreadMetadataUpdated { .. }
                | EventPayload::ThreadProjectAffinityUpdated { .. }
        ) {
            continue;
        }
        assert_eq!(
            direct_thread_list_patch(&event(payload)),
            None,
            "unselected event must not produce a thread-list patch"
        );
    }
}

#[test]
fn supplied_projection_is_copied_without_using_the_envelope_thread_id() {
    let supplied_projection = projection();
    let patch = direct_thread_list_patch(&event(EventPayload::ThreadMetadataUpdated {
        thread: supplied_projection.clone(),
    }))
    .expect("metadata update must upsert");

    assert_eq!(
        patch,
        ThreadListProjectionPatch::Upsert {
            thread: supplied_projection.clone(),
        }
    );
    assert_eq!(patch.thread_id(), "thread:projection");
    assert!(patch.is_upsert());
    assert!(!patch.is_remove());

    let remove =
        direct_thread_list_patch(&event(EventPayload::ThreadErased)).expect("erasure must remove");
    assert_eq!(remove.thread_id(), EVENT_THREAD_ID);
    assert!(remove.is_remove());
    assert!(!remove.is_upsert());
}

#[test]
fn graph_group_id_accepts_only_the_five_exact_group_event_forms() {
    assert_eq!(GRAPH_GROUP_EVENT_TYPES.len(), 5);
    let cases = [
        EventPayload::OrchestrationGraphLifecycle {
            group_id: GROUP_ID.to_owned(),
            action: "command_side_action".to_owned(),
        },
        EventPayload::AssignmentHeartbeat {
            group_id: GROUP_ID.to_owned(),
        },
        EventPayload::AgentInstanceRenamed {
            group_id: GROUP_ID.to_owned(),
        },
        EventPayload::AssignmentControl {
            group_id: GROUP_ID.to_owned(),
        },
        EventPayload::ArtifactRecorded {
            group_id: GROUP_ID.to_owned(),
        },
    ];

    for payload in cases {
        assert_eq!(graph_group_id(&event(payload)), Some(GROUP_ID));
    }

    for payload in all_payloads() {
        if matches!(
            &payload,
            EventPayload::OrchestrationGraphLifecycle { .. }
                | EventPayload::AssignmentHeartbeat { .. }
                | EventPayload::AgentInstanceRenamed { .. }
                | EventPayload::AssignmentControl { .. }
                | EventPayload::ArtifactRecorded { .. }
        ) {
            continue;
        }
        assert_eq!(graph_group_id(&event(payload)), None);
    }
}

#[test]
fn transcript_allowlist_is_exact_for_every_modeled_event_type() {
    for payload in all_payloads() {
        let expected = TRANSCRIPT_EVENT_TYPES.contains(&payload.type_name());
        assert_eq!(
            event_affects_transcript(&event(payload.clone())),
            expected,
            "transcript selection for {}",
            payload.type_name()
        );
    }

    assert!(event_affects_transcript(&event(EventPayload::other(
        "thread.message_queued",
    ))));
    assert!(!event_affects_transcript(&event(EventPayload::other(
        "thread.message_queued ",
    ))));
}

#[test]
fn surface_allowlist_has_six_direct_forms_and_no_unattributed_provider_forms() {
    for payload in [
        EventPayload::AssistantMessageCompleted,
        EventPayload::InteractionApproval,
        EventPayload::InteractionQuestion,
        EventPayload::RunLifecycle,
        EventPayload::UsageInterruptionUpdated,
        EventPayload::ThreadErased,
    ] {
        assert!(event_affects_surface(&event(payload)));
    }

    for payload in all_payloads() {
        if matches!(
            &payload,
            EventPayload::AssistantMessageCompleted
                | EventPayload::InteractionApproval
                | EventPayload::InteractionQuestion
                | EventPayload::RunLifecycle
                | EventPayload::UsageInterruptionUpdated
                | EventPayload::ThreadErased
                | EventPayload::OrchestrationGraphLifecycle { .. }
                | EventPayload::ArtifactRecorded { .. }
        ) {
            continue;
        }
        assert!(!event_affects_surface(&event(payload)));
    }

    assert!(!event_affects_surface(&event(
        EventPayload::ArtifactRecorded {
            group_id: GROUP_ID.to_owned(),
        },
    )));
    assert!(!event_affects_surface(&event(
        EventPayload::OrchestrationGraphLifecycle {
            group_id: GROUP_ID.to_owned(),
            action: "finished".to_owned(),
        },
    )));

    assert_eq!(SURFACE_EVENT_TYPES.len(), 6);
}

#[test]
fn raw_artifacts_and_exact_provider_graph_actions_affect_surface() {
    assert!(is_provider_projection_event(&provider_event(
        EventPayload::ArtifactRecorded {
            group_id: GROUP_ID.to_owned(),
        },
    )));
    assert!(event_affects_surface(&provider_event(
        EventPayload::ArtifactRecorded {
            group_id: GROUP_ID.to_owned(),
        },
    )));

    for action in PROVIDER_PROJECTION_GRAPH_ACTIONS {
        let payload = EventPayload::OrchestrationGraphLifecycle {
            group_id: GROUP_ID.to_owned(),
            action: (*action).to_owned(),
        };
        assert!(is_provider_projection_event(&provider_event(
            payload.clone()
        )));
        assert!(event_affects_surface(&provider_event(payload)));
    }

    for action in [
        "attempt_started",
        "attempt_failed",
        "provider_state ",
        "ProviderState",
        "summary_recorded\n",
        "",
    ] {
        let payload = EventPayload::OrchestrationGraphLifecycle {
            group_id: GROUP_ID.to_owned(),
            action: action.to_owned(),
        };
        assert!(!is_provider_projection_event(&provider_event(
            payload.clone()
        )));
        assert!(!event_affects_surface(&provider_event(payload)));
    }
}

#[test]
fn any_present_raw_origin_is_the_only_origin_requirement() {
    let event = event(EventPayload::OrchestrationGraphLifecycle {
        group_id: GROUP_ID.to_owned(),
        action: "finished".to_owned(),
    })
    .with_raw_origin(RawOrigin::new("", ""));

    assert!(is_provider_projection_event(&event));
    assert!(event_affects_surface(&event));
}

#[test]
fn workspace_conflict_allowlist_is_exact_and_erasure_clears_it() {
    assert_eq!(WORKSPACE_CONFLICT_EVENT_TYPES.len(), 2);
    assert!(event_affects_workspace_conflicts(&event(
        EventPayload::WorkspaceConflictUpdated,
    )));
    assert!(event_affects_workspace_conflicts(&event(
        EventPayload::ThreadErased
    )));

    for payload in all_payloads() {
        if matches!(
            &payload,
            EventPayload::WorkspaceConflictUpdated | EventPayload::ThreadErased
        ) {
            continue;
        }
        assert!(!event_affects_workspace_conflicts(&event(payload)));
    }
}

#[test]
fn policy_facade_matches_the_free_function_surface() {
    let policy = SubscriptionPatchSelectionPolicy::new();
    assert_eq!(policy, SubscriptionPatchSelectionPolicy);

    let payload = EventPayload::ThreadCreated {
        title: "Facade".to_owned(),
    };
    let input = event(payload);

    assert_eq!(
        SubscriptionPatchSelectionPolicy::direct_thread_list_patch(&input),
        direct_thread_list_patch(&input)
    );
    assert_eq!(
        SubscriptionPatchSelectionPolicy::graph_group_id(&input),
        graph_group_id(&input)
    );
    assert_eq!(
        SubscriptionPatchSelectionPolicy::event_affects_transcript(&input),
        event_affects_transcript(&input)
    );
    assert_eq!(
        SubscriptionPatchSelectionPolicy::event_affects_surface(&input),
        event_affects_surface(&input)
    );
    assert_eq!(
        SubscriptionPatchSelectionPolicy::event_affects_workspace_conflicts(&input),
        event_affects_workspace_conflicts(&input)
    );
    assert_eq!(
        SubscriptionPatchSelectionPolicy::is_provider_projection_event(&input),
        is_provider_projection_event(&input)
    );
}
