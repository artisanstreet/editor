//! Dependency-free parity tests for the native thread-panel policy.
//!
//! The cases mirror `thread-panel.svelte` while keeping every host operation
//! as an inspected typed action. No browser, Effect runtime, protocol crate,
//! controller, or route executor is required.

#[path = "../../modules/frontend/src/thread_panel_policy.rs"]
mod thread_panel_policy;

use thread_panel_policy::{
    ACTIVE_TASK_TONE_CLASS, COMPLETED_TASK_TONE_CLASS, ChecklistEntry, ChecklistEntryPresentation,
    ChecklistEntryState, ComposerDraftMoveResult, ConversationPlan, DETACHED_WORKSPACE_ROUTE_ID,
    OrphanedAttachment, PENDING_TASK_TONE_CLASS, ProjectSelection, SKIPPED_TASK_TONE_CLASS,
    ThreadPanelAdapterAction, ThreadPanelContext, ThreadPanelPolicy, encode_uri_component,
    new_thread_draft_key, plan_entries, present_checklist_entry, project_plan, project_selection,
    state_accessibility_prefix, task_tone_class, workspace_route_path,
};

#[test]
fn absent_plan_projects_an_empty_entry_list_and_present_plan_preserves_order() {
    assert!(project_plan(None).entries.is_empty());
    assert!(plan_entries(None).is_empty());

    let empty_plan = ConversationPlan::new(&[]);
    assert!(project_plan(Some(&empty_plan)).entries.is_empty());
    assert!(plan_entries(Some(&empty_plan)).is_empty());

    let entries = [
        ChecklistEntry::new("active-id", ChecklistEntryState::Active, "active text"),
        ChecklistEntry::new("done-id", ChecklistEntryState::Completed, "done text"),
        ChecklistEntry::new("pending-id", ChecklistEntryState::Pending, "pending text"),
        ChecklistEntry::new("skip-id", ChecklistEntryState::Skipped, "skipped text"),
    ];
    let plan = ConversationPlan::new(&entries);
    let projection = project_plan(Some(&plan));

    assert_eq!(projection.entries, &entries);
    assert_eq!(plan_entries(Some(&plan)), &entries);
}

#[test]
fn every_checklist_state_has_the_exact_class_and_accessibility_prefix() {
    let table = [
        (
            ChecklistEntryState::Active,
            "active",
            ACTIVE_TASK_TONE_CLASS,
            "active: ",
        ),
        (
            ChecklistEntryState::Completed,
            "completed",
            COMPLETED_TASK_TONE_CLASS,
            "completed: ",
        ),
        (
            ChecklistEntryState::Pending,
            "pending",
            PENDING_TASK_TONE_CLASS,
            "pending: ",
        ),
        (
            ChecklistEntryState::Skipped,
            "skipped",
            SKIPPED_TASK_TONE_CLASS,
            "skipped: ",
        ),
    ];

    assert_eq!(ChecklistEntryState::ALL, table.map(|row| row.0));
    for (state, state_text, class, prefix) in table {
        assert_eq!(state.as_str(), state_text);
        assert_eq!(state.task_tone_class(), class);
        assert_eq!(state.accessibility_prefix(), prefix);
        assert_eq!(task_tone_class(state), class);
        assert_eq!(state_accessibility_prefix(state), prefix);
    }
}

#[test]
fn all_state_plan_presentation_preserves_empty_and_unicode_entry_content() {
    let entries = [
        ChecklistEntry::new("", ChecklistEntryState::Active, ""),
        ChecklistEntry::new("識別子🚀", ChecklistEntryState::Completed, "完了 — текст"),
        ChecklistEntry::new("pending", ChecklistEntryState::Pending, "等待"),
        ChecklistEntry::new("skipped", ChecklistEntryState::Skipped, "skip"),
    ];
    let plan = ConversationPlan::new(&entries);
    let projection = project_plan(Some(&plan));

    for (entry, projected) in projection.entries.iter().copied().zip(
        projection
            .entries
            .iter()
            .copied()
            .map(present_checklist_entry),
    ) {
        let expected = ChecklistEntryPresentation {
            id: entry.id,
            state: entry.state,
            task_tone_class: entry.state.task_tone_class(),
            accessibility_prefix: entry.state.accessibility_prefix(),
            text: entry.text,
        };
        assert_eq!(projected, expected);
    }
}

#[test]
fn draft_keys_cover_thread_workspace_and_empty_unicode_matrices() {
    let cases = [
        (None, None, "draft:new-thread"),
        (None, Some(""), "draft:"),
        (
            None,
            Some(" workspace/é?&=+%#[]:🚀 "),
            "draft: workspace/é?&=+%#[]:🚀 ",
        ),
        (Some(""), None, ""),
        (Some(""), Some("workspace"), ""),
        (Some("thread/é?🚀"), Some("workspace"), "thread/é?🚀"),
    ];

    for (thread_id, workspace_id, expected) in cases {
        let context = ThreadPanelContext::new(thread_id, workspace_id);
        assert_eq!(context.current_draft_key(), expected);
    }

    assert_eq!(new_thread_draft_key(None), "draft:new-thread");
    assert_eq!(new_thread_draft_key(Some("")), "draft:");
    assert_eq!(
        new_thread_draft_key(Some("新しい/作業空間")),
        "draft:新しい/作業空間"
    );
}

#[test]
fn route_encoding_matches_encode_uri_component_for_empty_unicode_and_reserved_values() {
    assert_eq!(encode_uri_component(""), "");
    assert_eq!(encode_uri_component("AZaz09-_.!~*'()"), "AZaz09-_.!~*'()");
    assert_eq!(
        encode_uri_component("project /?&=+%#[]:é🚀"),
        "project%20%2F%3F%26%3D%2B%25%23%5B%5D%3A%C3%A9%F0%9F%9A%80"
    );
    assert_eq!(workspace_route_path(None), "/t/_");
    assert_eq!(workspace_route_path(Some("")), "/t/");
    assert_eq!(
        workspace_route_path(Some("project /é🚀")),
        "/t/project%20%2F%C3%A9%F0%9F%9A%80"
    );
    assert_eq!(DETACHED_WORKSPACE_ROUTE_ID, "_");
}

#[test]
fn selected_project_uses_thread_id_when_present_and_exact_new_thread_destination() {
    let cases = [
        (
            ThreadPanelContext::new(Some("thread-id"), Some("workspace-id")),
            "project-id",
            "thread-id",
            "draft:project-id",
            "/t/project-id",
        ),
        (
            ThreadPanelContext::new(None, Some("workspace-id")),
            "project-id",
            "draft:workspace-id",
            "draft:project-id",
            "/t/project-id",
        ),
        (
            ThreadPanelContext::new(None, None),
            "",
            "draft:new-thread",
            "draft:",
            "/t/",
        ),
        (
            ThreadPanelContext::new(Some(""), Some("workspace-id")),
            "selected/é🚀",
            "",
            "draft:selected/é🚀",
            "/t/selected%2F%C3%A9%F0%9F%9A%80",
        ),
    ];

    for (context, project_id, current, destination, navigation) in cases {
        let selection = ProjectSelection::new(context, project_id);
        assert_eq!(selection.project_id, project_id);
        assert_eq!(selection.current_draft_key, current);
        assert_eq!(selection.destination_draft_key, destination);
        assert_eq!(selection.navigation_path, navigation);
    }
}

#[test]
fn zero_orphans_emit_move_then_navigation_and_ignore_the_moved_flag() {
    let selection = project_selection(
        ThreadPanelContext::new(Some("thread"), Some("workspace")),
        "project",
    );
    let result = ComposerDraftMoveResult::new(false, Vec::new());

    assert_eq!(
        selection.move_action(),
        ThreadPanelAdapterAction::MoveComposerDraft {
            from_draft_key: "thread".to_owned(),
            to_draft_key: "draft:project".to_owned(),
        }
    );
    assert_eq!(
        selection.actions_after_move(&result),
        vec![ThreadPanelAdapterAction::Navigate {
            path: "/t/project".to_owned(),
        }]
    );
    assert_eq!(
        selection.adapter_actions(&result),
        vec![
            ThreadPanelAdapterAction::MoveComposerDraft {
                from_draft_key: "thread".to_owned(),
                to_draft_key: "draft:project".to_owned(),
            },
            ThreadPanelAdapterAction::Navigate {
                path: "/t/project".to_owned(),
            },
        ]
    );
}

#[test]
fn one_and_many_orphans_release_in_result_order_before_navigation() {
    let selection = ProjectSelection::new(ThreadPanelContext::new(None, None), "p/é");
    let result = ComposerDraftMoveResult::new(
        true,
        vec![
            OrphanedAttachment::new(""),
            OrphanedAttachment::new("blob:one/é"),
            OrphanedAttachment::new("data:image/png;base64,🚀"),
        ],
    );

    assert_eq!(
        selection.actions_after_move(&result),
        vec![
            ThreadPanelAdapterAction::ReleaseBrowserObjectUrl {
                preview_url: String::new(),
            },
            ThreadPanelAdapterAction::ReleaseBrowserObjectUrl {
                preview_url: "blob:one/é".to_owned(),
            },
            ThreadPanelAdapterAction::ReleaseBrowserObjectUrl {
                preview_url: "data:image/png;base64,🚀".to_owned(),
            },
            ThreadPanelAdapterAction::Navigate {
                path: "/t/p%2F%C3%A9".to_owned(),
            },
        ]
    );
}

#[test]
fn a_release_failure_does_not_remove_later_releases_or_move_navigation() {
    let selection = ProjectSelection::new(ThreadPanelContext::new(None, Some("workspace")), "p");
    let result = ComposerDraftMoveResult::new(
        true,
        vec![
            OrphanedAttachment::new("first"),
            OrphanedAttachment::new("fails"),
            OrphanedAttachment::new("last"),
        ],
    );
    let actions = selection.actions_after_move(&result);
    let mut release_attempts = Vec::new();
    let mut failed_releases = 0;
    let mut navigation_seen = false;

    for action in actions {
        match action {
            ThreadPanelAdapterAction::ReleaseBrowserObjectUrl { preview_url } => {
                release_attempts.push(preview_url);
                if release_attempts.last().is_some_and(|url| url == "fails") {
                    // The host's ignored error does not break this ordered action loop.
                    failed_releases += 1;
                }
            }
            ThreadPanelAdapterAction::Navigate { path } => {
                assert_eq!(release_attempts, ["first", "fails", "last"]);
                assert_eq!(failed_releases, 1);
                assert_eq!(path, "/t/p");
                navigation_seen = true;
            }
            ThreadPanelAdapterAction::OpenProjectPicker
            | ThreadPanelAdapterAction::MoveComposerDraft { .. } => {
                panic!("post-Move actions must contain only releases and navigation")
            }
        }
    }

    assert!(navigation_seen);
}

#[test]
fn complete_action_trace_keeps_move_release_and_navigation_boundaries() {
    let policy = ThreadPanelPolicy::from_ids(Some("thread"), Some("workspace"));
    let result = ComposerDraftMoveResult::new(true, vec![OrphanedAttachment::new("preview")]);

    assert_eq!(
        policy.select_project_actions("project", &result),
        vec![
            ThreadPanelAdapterAction::MoveComposerDraft {
                from_draft_key: "thread".to_owned(),
                to_draft_key: "draft:project".to_owned(),
            },
            ThreadPanelAdapterAction::ReleaseBrowserObjectUrl {
                preview_url: "preview".to_owned(),
            },
            ThreadPanelAdapterAction::Navigate {
                path: "/t/project".to_owned(),
            },
        ]
    );
}

#[test]
fn opening_the_project_picker_is_idempotent_and_keeps_the_same_typed_action() {
    let mut policy = ThreadPanelPolicy::from_ids(Some("thread"), Some("workspace"));
    assert!(!policy.is_project_picker_open());

    let first = policy.open_project_picker();
    assert_eq!(first, ThreadPanelAdapterAction::OpenProjectPicker);
    assert!(policy.is_project_picker_open());

    let second = policy.open_project_picker();
    assert_eq!(second, first);
    assert!(policy.is_project_picker_open());
    assert_eq!(policy.thread_id(), Some("thread"));
    assert_eq!(policy.workspace_id(), Some("workspace"));
}
