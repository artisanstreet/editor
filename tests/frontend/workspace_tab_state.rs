//! Direct tests for the dependency-free workspace tab state boundary.
//!
//! The source is loaded directly so these focused tests do not require shared
//! module registration, Cargo, Bazel, or host/runtime dependencies.

#[path = "../../modules/frontend/src/workspace_tab_state.rs"]
mod workspace_tab_state;

use workspace_tab_state::{
    AgentChangeBadge, ChatViewState, EditorViewState, OrchestratorViewState, TabContent,
    TabEditState, TabOwnership, WorkspaceFileReference, WorkspaceMode, WorkspaceState,
    create_workspace_state, diff_tab_id, file_tab_id, make_file_tab, switch_mode, update_chat_view,
    update_editor_view, update_orchestrator_view,
};

fn file(id: &str, name: &str) -> WorkspaceFileReference {
    WorkspaceFileReference::new(id, name, "rust", format!("src/{name}"))
}

#[test]
fn empty_initialization_preserves_all_exact_defaults() {
    let state = create_workspace_state([]);

    assert_eq!(state.mode, WorkspaceMode::Editor);
    assert!(state.tabs.is_empty());
    assert_eq!(state.active_tab_id, None);
    assert!(state.recent_files.is_empty());
    assert!(state.changed_files.is_empty());
    assert_eq!(state.editor, EditorViewState::default());
    assert!(state.editor.scroll_top.abs() <= f64::EPSILON);
    assert_eq!(state.editor.cursor_line, 1);
    assert_eq!(state.editor.cursor_column, 1);
    assert_eq!(state.chat, ChatViewState::default());
    assert_eq!(state.chat.draft, "");
    assert!(state.chat.transcript_scroll_top.abs() <= f64::EPSILON);
    assert_eq!(state.orchestrator, OrchestratorViewState::default());
    assert_eq!(state.orchestrator.selected_node_id, None);
    assert!(state.orchestrator.graph_scroll_top.abs() <= f64::EPSILON);
    assert_eq!(state.next_tab_generation, 0);
}

#[test]
fn populated_initialization_preserves_order_recency_ids_and_generations() {
    let first = file("one", "one.rs");
    let second = file("two", "two.rs");
    let third = file("three", "three.rs");
    let initial_files = vec![first.clone(), second.clone(), third.clone()];

    let state = WorkspaceState::new(&initial_files);

    assert_eq!(
        state
            .tabs
            .iter()
            .map(|tab| tab.file.id.as_str())
            .collect::<Vec<_>>(),
        vec!["one", "two", "three"]
    );
    assert_eq!(
        state
            .tabs
            .iter()
            .map(|tab| tab.id.as_str())
            .collect::<Vec<_>>(),
        vec!["file:one", "file:two", "file:three"]
    );
    assert_eq!(
        state
            .tabs
            .iter()
            .map(|tab| tab.generation)
            .collect::<Vec<_>>(),
        vec![0, 1, 2]
    );
    assert_eq!(state.active_tab_id.as_deref(), Some("file:one"));
    assert_eq!(state.recent_files, vec![third, second, first]);
    assert_eq!(state.next_tab_generation, 3);
    assert!(state.tabs.iter().all(|tab| {
        tab.ownership == TabOwnership::Open
            && tab.content == TabContent::File
            && tab.edit_state == TabEditState::Clean
            && tab.agent_change.is_none()
    }));
}

#[test]
fn file_and_diff_ids_and_clean_construction_are_deterministic() {
    assert_eq!(file_tab_id("src/main.rs"), "file:src/main.rs");
    assert_eq!(
        diff_tab_id("src/main.rs", "change-7"),
        r#"["diff","src/main.rs","change-7"]"#
    );
    assert_eq!(
        diff_tab_id("file\"id", "change\\id\n"),
        r#"["diff","file\"id","change\\id\n"]"#
    );

    let file = file("source", "source.rs");
    let tab = make_file_tab(
        &file,
        TabOwnership::Pinned,
        TabContent::DiffPreview {
            change_id: String::from("change-7"),
        },
        41,
    );

    assert_eq!(tab.id, r#"["diff","source","change-7"]"#);
    assert_eq!(tab.generation, 41);
    assert_eq!(tab.file, file);
    assert_eq!(tab.ownership, TabOwnership::Pinned);
    assert_eq!(
        tab.content,
        TabContent::DiffPreview {
            change_id: String::from("change-7")
        }
    );
    assert_eq!(tab.edit_state, TabEditState::Clean);
    assert_eq!(tab.agent_change, None);

    let preview = make_file_tab(&file, TabOwnership::Preview, TabContent::File, 42);
    assert_eq!(preview.ownership, TabOwnership::Preview);
    let mut dirty_preview = preview;
    dirty_preview.edit_state = TabEditState::Dirty { revision: 2 };
    assert_eq!(
        dirty_preview.edit_state,
        TabEditState::Dirty { revision: 2 }
    );
}

#[test]
fn mode_and_each_view_transition_isolated_from_other_state() {
    let file = file("one", "one.rs");
    let initial = WorkspaceState::new(vec![file]);
    let badge = AgentChangeBadge {
        agent_name: String::from("Ada"),
        added: 8,
        removed: 3,
    };
    let mut baseline = initial.clone();
    baseline
        .changed_files
        .push(workspace_tab_state::ChangedFile {
            file: baseline.tabs[0].file.clone(),
            change: badge,
        });

    let editor = EditorViewState {
        scroll_top: 12.5,
        cursor_line: 9,
        cursor_column: 4,
    };
    let after_editor = update_editor_view(&baseline, editor.clone());
    assert_eq!(after_editor.editor, editor);
    assert_eq!(after_editor.chat, baseline.chat);
    assert_eq!(after_editor.orchestrator, baseline.orchestrator);
    assert_eq!(after_editor.tabs, baseline.tabs);
    assert_eq!(after_editor.changed_files, baseline.changed_files);
    assert_eq!(
        after_editor.next_tab_generation,
        baseline.next_tab_generation
    );

    let chat = ChatViewState {
        draft: String::from("hello"),
        transcript_scroll_top: 38.25,
    };
    let after_chat = update_chat_view(&after_editor, chat.clone());
    assert_eq!(after_chat.chat, chat);
    assert_eq!(after_chat.editor, after_editor.editor);
    assert_eq!(after_chat.orchestrator, after_editor.orchestrator);

    let orchestrator = OrchestratorViewState {
        selected_node_id: Some(String::from("node-2")),
        graph_scroll_top: 7.75,
    };
    let after_orchestrator = update_orchestrator_view(&after_chat, orchestrator.clone());
    assert_eq!(after_orchestrator.orchestrator, orchestrator);
    assert_eq!(after_orchestrator.editor, after_chat.editor);
    assert_eq!(after_orchestrator.chat, after_chat.chat);

    let after_chat_mode = switch_mode(&after_orchestrator, WorkspaceMode::Chat);
    assert_eq!(after_chat_mode.mode, WorkspaceMode::Chat);
    assert_eq!(
        after_chat_mode.orchestrator,
        after_orchestrator.orchestrator
    );

    let after_mode = switch_mode(&after_orchestrator, WorkspaceMode::Orchestrator);
    assert_eq!(after_mode.mode, WorkspaceMode::Orchestrator);
    assert_eq!(after_mode.tabs, after_orchestrator.tabs);
    assert_eq!(after_mode.active_tab_id, after_orchestrator.active_tab_id);
    assert_eq!(after_mode.recent_files, after_orchestrator.recent_files);
    assert_eq!(after_mode.changed_files, after_orchestrator.changed_files);
    assert_eq!(after_mode.editor, after_orchestrator.editor);
    assert_eq!(after_mode.chat, after_orchestrator.chat);
    assert_eq!(after_mode.orchestrator, after_orchestrator.orchestrator);
    assert_eq!(
        after_mode.next_tab_generation,
        after_orchestrator.next_tab_generation
    );

    assert_eq!(baseline.mode, WorkspaceMode::Editor);
    assert_eq!(baseline.editor, EditorViewState::default());
    assert_eq!(initial.tabs[0].agent_change, None);
}
