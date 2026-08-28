//! Dependency-free parity coverage for the editor route gate policy.
//!
//! The source modules are included directly so this focused harness exercises
//! the leaf without the frontend crate's VP-owned registration or any browser,
//! Svelte, transport, stream, or rendering runtime.

#[path = "../../modules/frontend/src/editor_route_gate_policy.rs"]
mod editor_route_gate_policy;
#[allow(dead_code)]
#[path = "../../modules/frontend/src/route_navigation.rs"]
mod route_navigation;

use editor_route_gate_policy::{
    EDITOR_ROUTE_NAVIGATION_OPTIONS, EditorRouteGateDecision, EditorRouteGateInput,
    EditorRouteGatePresentation, EditorRouteGatePresentationInput, EditorRouteTarget,
    LOADING_THREAD_TEXT, ROOT_ROUTE_PATH, ResolvedThread, present_editor_route_gate,
    reconcile_editor_route_gate,
};
use route_navigation::{RouteNavigationOptions, RouteNavigationTarget};

fn project_thread<'a>(thread_id: &'a str, project_id: &'a str) -> ResolvedThread<'a> {
    ResolvedThread::in_project(thread_id, project_id)
}

fn detached_thread(thread_id: &str) -> ResolvedThread<'_> {
    ResolvedThread::detached(thread_id)
}

fn input<'a>(
    route_owns_target: bool,
    catalog_loaded: bool,
    resolved_thread: Option<ResolvedThread<'a>>,
    editor_target: Option<EditorRouteTarget<'a>>,
    current_url: &'a str,
    route_workspace_owned: bool,
) -> EditorRouteGateInput<'a> {
    EditorRouteGateInput::new(
        route_owns_target,
        catalog_loaded,
        resolved_thread,
        editor_target,
        current_url,
        route_workspace_owned,
    )
}

fn navigation_path<'a>(decision: &'a EditorRouteGateDecision<'_>) -> Option<&'a str> {
    decision
        .navigation()
        .map(route_navigation::RouteNavigationIntent::path)
}

#[test]
fn ownership_and_catalog_loaded_precedence_is_exhaustive_for_missing_threads() {
    for route_owns_target in [false, true] {
        for catalog_loaded in [false, true] {
            let decision = reconcile_editor_route_gate(input(
                route_owns_target,
                catalog_loaded,
                None,
                None,
                "/e/workspace/thread",
                true,
            ));

            if route_owns_target && catalog_loaded {
                assert_eq!(navigation_path(&decision), Some(ROOT_ROUTE_PATH));
                assert!(decision.clears_active_state());
                assert_eq!(decision.activated_thread(), None);
            } else {
                assert_eq!(decision, EditorRouteGateDecision::NoOp);
                assert!(decision.is_no_op());
                assert!(!decision.clears_active_state());
                assert_eq!(navigation_path(&decision), None);
            }
        }
    }
}

#[test]
fn unloaded_catalog_is_a_no_op_even_when_every_later_fact_would_redirect() {
    let cases = [
        (None, None, "", false, "missing thread"),
        (
            Some(project_thread("thread", "workspace")),
            Some(EditorRouteTarget::thread("/t/workspace/thread")),
            "/e/workspace/thread?file=main.rs",
            false,
            "non-editor target",
        ),
        (
            Some(project_thread("thread", "workspace")),
            Some(EditorRouteTarget::editor(
                "/e/workspace/thread?file=main.rs",
                "workspace",
            )),
            "/e/workspace/thread?file=other.rs",
            false,
            "URL mismatch",
        ),
    ];

    for (resolved_thread, target, current_url, route_workspace_owned, label) in cases {
        let decision = reconcile_editor_route_gate(input(
            true,
            false,
            resolved_thread,
            target,
            current_url,
            route_workspace_owned,
        ));
        assert_eq!(decision, EditorRouteGateDecision::NoOp, "{label}");
    }
}

#[test]
fn missing_thread_clears_active_state_before_an_exact_root_navigation() {
    let decision = reconcile_editor_route_gate(input(
        true,
        true,
        None,
        None,
        "/e/workspace/removed?file=main.rs",
        true,
    ));

    assert_eq!(navigation_path(&decision), Some(ROOT_ROUTE_PATH));
    assert!(decision.clears_active_state());
    let intent = decision.navigation().expect("missing thread navigates");
    assert_eq!(
        intent.target(),
        &RouteNavigationTarget::Text(String::from("/"))
    );
    assert_eq!(intent.options(), EDITOR_ROUTE_NAVIGATION_OPTIONS);
    assert_eq!(
        intent.options(),
        RouteNavigationOptions::new(Some(true), Some(true), Some(true))
    );
}

#[test]
fn target_workspace_and_url_matrix_preserves_the_later_branch_order() {
    let target_path = "/e/workspace/thread?file=main.rs";
    let thread = project_thread("thread", "workspace");

    // The non-editor branch wins over workspace and URL facts.
    for route_workspace_owned in [false, true] {
        for current_url in [target_path, "/some/other?file=main.rs"] {
            let decision = reconcile_editor_route_gate(input(
                true,
                true,
                Some(thread),
                Some(EditorRouteTarget::thread("/t/workspace/thread")),
                current_url,
                route_workspace_owned,
            ));
            assert_eq!(navigation_path(&decision), Some("/t/workspace/thread"));
            assert!(decision.clears_active_state());
            assert_eq!(decision.activated_thread(), None);
        }
    }

    // An editor target still redirects when the adapter says the route does
    // not own the thread's workspace, irrespective of exact URL equality.
    for current_url in [target_path, "/e/workspace/thread?file=other.rs"] {
        let decision = reconcile_editor_route_gate(input(
            true,
            true,
            Some(thread),
            Some(EditorRouteTarget::editor(target_path, "workspace")),
            current_url,
            false,
        ));
        assert_eq!(navigation_path(&decision), Some(target_path));
        assert!(decision.clears_active_state());
    }

    // Workspace ownership is necessary but not sufficient: the exact URL is
    // the final gate before activation.
    let mismatch = reconcile_editor_route_gate(input(
        true,
        true,
        Some(thread),
        Some(EditorRouteTarget::editor(target_path, "workspace")),
        "/e/workspace/thread?file=other.rs",
        true,
    ));
    assert_eq!(navigation_path(&mismatch), Some(target_path));
    assert!(mismatch.clears_active_state());

    let match_decision = reconcile_editor_route_gate(input(
        true,
        true,
        Some(thread),
        Some(EditorRouteTarget::editor(target_path, "workspace")),
        target_path,
        true,
    ));
    assert_eq!(
        match_decision.activated_thread(),
        Some(project_thread("thread", "workspace"))
    );
    assert!(!match_decision.clears_active_state());
    assert_eq!(match_decision.navigation(), None);
    assert_eq!(
        EditorRouteTarget::editor(target_path, "workspace").workspace_id(),
        Some("workspace")
    );
}

#[test]
fn exact_path_and_query_comparison_does_not_parse_or_normalize_url_text() {
    let target_path = "/e/%E4%B8%96/thread?file=a%2Fb&line=1";
    let thread = project_thread("thread", "世界");
    let exact_and_near_misses = [
        (target_path, true),
        ("/e/%E4%B8%96/thread?file=a/b&line=1", false),
        ("/e/%E4%B8%96/thread?line=1&file=a%2Fb", false),
        ("/e/%E4%B8%96/thread?file=a%2Fb&line=01", false),
        ("/e/%E4%B8%96/thread?file=a%2Fb&line=1&", false),
        ("/e/%E4%B8%96/thread?file=a%2Fb&line=1#panel", false),
        (" /e/%E4%B8%96/thread?file=a%2Fb&line=1", false),
    ];

    for (current_url, activates) in exact_and_near_misses {
        let decision = reconcile_editor_route_gate(input(
            true,
            true,
            Some(thread),
            Some(EditorRouteTarget::editor(target_path, "世界")),
            current_url,
            true,
        ));

        if activates {
            assert_eq!(decision.activated_thread(), Some(thread));
        } else {
            assert_eq!(navigation_path(&decision), Some(target_path));
            assert!(decision.clears_active_state());
        }
    }
}

#[test]
fn empty_and_unicode_ids_and_paths_are_preserved_in_activation_and_redirects() {
    let thread = project_thread("", "  工作区 🚀  ");
    let target_path = "/e/%20%20%E5%B7%A5%E4%BD%9C%E5%8C%BA%20%F0%9F%9A%80%20%20/?file=";
    let exact = reconcile_editor_route_gate(input(
        true,
        true,
        Some(thread),
        Some(EditorRouteTarget::editor(target_path, "  工作区 🚀  ")),
        target_path,
        true,
    ));
    assert_eq!(exact.activated_thread(), Some(thread));

    let target = EditorRouteTarget::thread("/t/_/%E7%A9%BA%F0%9F%9A%80?x=%2F");
    let redirect = reconcile_editor_route_gate(input(
        true,
        true,
        Some(detached_thread("空🚀")),
        Some(target),
        "",
        true,
    ));
    assert_eq!(navigation_path(&redirect), Some(target.path()));
    assert!(redirect.clears_active_state());
}

#[test]
fn missing_adapter_target_fails_closed_without_inventing_navigation() {
    let decision = reconcile_editor_route_gate(input(
        true,
        true,
        Some(project_thread("thread", "workspace")),
        None,
        "/e/workspace/thread",
        true,
    ));

    assert_eq!(decision, EditorRouteGateDecision::NoOp);
    assert!(!decision.clears_active_state());
    assert_eq!(decision.navigation(), None);
    assert_eq!(decision.activated_thread(), None);
}

#[test]
fn every_redirect_uses_exact_true_navigation_flags_and_a_path_target() {
    let decisions = [
        reconcile_editor_route_gate(input(true, true, None, None, "/e/workspace/missing", true)),
        reconcile_editor_route_gate(input(
            true,
            true,
            Some(project_thread("thread", "workspace")),
            Some(EditorRouteTarget::thread("/t/workspace/thread")),
            "/e/workspace/thread",
            true,
        )),
        reconcile_editor_route_gate(input(
            true,
            true,
            Some(project_thread("thread", "workspace")),
            Some(EditorRouteTarget::editor(
                "/e/workspace/thread?file=main.rs",
                "workspace",
            )),
            "/e/workspace/thread?file=other.rs",
            true,
        )),
    ];

    for decision in decisions {
        let intent = decision.navigation().expect("case must navigate");
        assert_eq!(intent.options(), EDITOR_ROUTE_NAVIGATION_OPTIONS);
        assert_eq!(
            intent.options(),
            RouteNavigationOptions::new(Some(true), Some(true), Some(true))
        );
        assert!(!intent.target().is_url());
    }
}

#[test]
fn presentation_precedence_covers_active_project_detached_and_absent_states() {
    let project = project_thread("thread", "workspace");
    let detached = detached_thread("detached");

    let cases = [
        (
            None,
            false,
            EditorRouteGatePresentation::Loading {
                message: LOADING_THREAD_TEXT,
            },
        ),
        (None, true, EditorRouteGatePresentation::NoSurface),
        (
            Some(detached),
            false,
            EditorRouteGatePresentation::Loading {
                message: LOADING_THREAD_TEXT,
            },
        ),
        (Some(detached), true, EditorRouteGatePresentation::NoSurface),
        (
            Some(project),
            false,
            EditorRouteGatePresentation::Editor {
                thread_id: "thread",
                workspace_id: "workspace",
            },
        ),
        (
            Some(project),
            true,
            EditorRouteGatePresentation::Editor {
                thread_id: "thread",
                workspace_id: "workspace",
            },
        ),
    ];

    for (active_thread, catalog_loaded, expected) in cases {
        assert_eq!(
            present_editor_route_gate(EditorRouteGatePresentationInput::new(
                active_thread,
                catalog_loaded,
            )),
            expected,
        );
    }
}

#[test]
fn presentation_treats_empty_and_unicode_project_ids_as_present() {
    let thread = project_thread("空🧵", "");
    assert_eq!(
        present_editor_route_gate(EditorRouteGatePresentationInput::new(Some(thread), true,)),
        EditorRouteGatePresentation::Editor {
            thread_id: "空🧵",
            workspace_id: "",
        }
    );

    let unicode = project_thread("会話", "ワークスペース🚀");
    assert_eq!(
        present_editor_route_gate(EditorRouteGatePresentationInput::new(Some(unicode), false,)),
        EditorRouteGatePresentation::Editor {
            thread_id: "会話",
            workspace_id: "ワークスペース🚀",
        }
    );
}
