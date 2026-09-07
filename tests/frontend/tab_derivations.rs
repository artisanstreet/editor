//! Table-driven parity coverage for workspace tab and breadcrumb derivations.
//!
//! The source module is included directly so this focused harness stays
//! dependency-free and does not require shared crate registration.

#[path = "../../modules/frontend/src/tab_derivations.rs"]
mod tab_derivations;

use tab_derivations::{
    TabDerivationError, WorkspaceFileReference, WorkspaceState, WorkspaceTab, derive_breadcrumbs,
    derive_tab_overflow,
};

fn tab(id: &str) -> WorkspaceTab {
    WorkspaceTab { id: id.to_owned() }
}

fn state(ids: &[&str], active_tab_id: Option<&str>) -> WorkspaceState {
    WorkspaceState {
        tabs: ids.iter().map(|id| tab(id)).collect(),
        active_tab_id: active_tab_id.map(str::to_owned),
    }
}

fn ids(tabs: &[WorkspaceTab]) -> Vec<&str> {
    tabs.iter().map(|tab| tab.id.as_str()).collect()
}

#[test]
fn tab_overflow_table_covers_slices_limits_and_active_selection() {
    let cases = [
        ("empty", vec![], None, 4.0, vec![], vec![]),
        ("short", vec!["a", "b"], None, 4.0, vec!["a", "b"], vec![]),
        (
            "exact",
            vec!["a", "b", "c"],
            None,
            3.0,
            vec!["a", "b", "c"],
            vec![],
        ),
        (
            "overflow",
            vec!["a", "b", "c", "d"],
            None,
            2.0,
            vec!["a", "b"],
            vec!["c", "d"],
        ),
        (
            "zero uses one",
            vec!["a", "b", "c"],
            None,
            0.0,
            vec!["a"],
            vec!["b", "c"],
        ),
        (
            "negative uses one",
            vec!["a", "b", "c"],
            None,
            -7.75,
            vec!["a"],
            vec!["b", "c"],
        ),
        (
            "large finite makes all visible",
            vec!["a", "b"],
            None,
            f64::MAX,
            vec!["a", "b"],
            vec![],
        ),
        (
            "fraction truncates",
            vec!["a", "b", "c", "d"],
            None,
            2.9,
            vec!["a", "b"],
            vec!["c", "d"],
        ),
        (
            "active first stays first",
            vec!["a", "b", "c"],
            Some("a"),
            2.0,
            vec!["a", "b"],
            vec!["c"],
        ),
        (
            "active inside stays in order",
            vec!["a", "b", "c"],
            Some("b"),
            2.0,
            vec!["a", "b"],
            vec!["c"],
        ),
        (
            "active outside promotes",
            vec!["a", "b", "c"],
            Some("c"),
            2.0,
            vec!["c", "a"],
            vec!["b"],
        ),
        (
            "missing active does not promote",
            vec!["a", "b", "c"],
            Some("missing"),
            2.0,
            vec!["a", "b"],
            vec!["c"],
        ),
    ];

    for (name, input_ids, active_tab_id, max_visible, expected_visible, expected_overflow) in cases
    {
        let result = derive_tab_overflow(&state(&input_ids, active_tab_id), max_visible)
            .expect("finite max_visible must derive successfully");
        assert_eq!(ids(&result.visible), expected_visible, "visible: {name}");
        assert_eq!(ids(&result.overflow), expected_overflow, "overflow: {name}");
    }
}

#[test]
fn promotion_preserves_order_and_id_membership_without_duplicates() {
    let result = derive_tab_overflow(
        &state(&["third", "first", "second", "fourth"], Some("fourth")),
        3.0,
    )
    .expect("finite max_visible must derive successfully");

    assert_eq!(ids(&result.visible), vec!["fourth", "third", "first"]);
    assert_eq!(ids(&result.overflow), vec!["second"]);
    assert_eq!(result.visible.len(), 3);
    assert_eq!(result.overflow.len(), 1);
}

#[test]
fn overflow_uses_visible_ids_like_javascript_set_for_duplicate_input() {
    let result = derive_tab_overflow(&state(&["a", "a", "b", "c"], None), 2.0)
        .expect("finite max_visible must derive successfully");

    // The valid workspace invariant is unique IDs, but the ID-set behavior is
    // still explicit for malformed input: visible slices are not rewritten,
    // and every state tab with a visible ID is omitted from overflow.
    assert_eq!(ids(&result.visible), vec!["a", "a"]);
    assert_eq!(ids(&result.overflow), vec!["b", "c"]);
}

#[test]
fn non_finite_limits_are_rejected_even_for_empty_state() {
    let empty = state(&[], None);
    for max_visible in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        assert_eq!(
            derive_tab_overflow(&empty, max_visible),
            Err(TabDerivationError::NonFiniteMaxVisible),
            "non-finite max_visible must be handled explicitly"
        );
    }
}

#[test]
fn breadcrumbs_table_splits_and_filters_slashes() {
    let cases = [
        ("", vec![]),
        ("/", vec![]),
        ("///", vec![]),
        ("one", vec!["one"]),
        ("/one/two/", vec!["one", "two"]),
        ("one//two///three", vec!["one", "two", "three"]),
        (
            "//leading///middle//trailing//",
            vec!["leading", "middle", "trailing"],
        ),
        ("folder\\name/file", vec!["folder\\name", "file"]),
    ];

    for (path, expected) in cases {
        let file = WorkspaceFileReference {
            path: path.to_owned(),
        };
        assert_eq!(
            derive_breadcrumbs(&file),
            expected,
            "breadcrumbs for {path:?}"
        );
    }
}
