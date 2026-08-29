//! Exhaustive coverage for the pure conversation-checklist policy.
//!
//! The module is loaded directly so this focused harness stays dependency
//! free and does not require registration through the VP-owned `lib.rs` or
//! build files.

#[path = "../../modules/frontend/src/conversation_checklist.rs"]
mod conversation_checklist;

use conversation_checklist::{
    ConversationItem, ConversationLifecycle, ConversationPlan, ConversationPlanEntry,
    ConversationPlanEntryState, ConversationPlanState, ConversationTurn,
    conversation_plan_has_open_entries, latest_conversation_plan,
};

fn plan(
    id: &str,
    turn_id: &str,
    ordinal: u64,
    revision: u64,
    state: ConversationPlanState,
    entries: &[ConversationPlanEntryState],
) -> ConversationPlan {
    ConversationPlan::new(
        id,
        turn_id,
        ordinal,
        revision,
        state,
        entries
            .iter()
            .copied()
            .map(ConversationPlanEntry::new)
            .collect(),
    )
}

fn turn(id: &str, lifecycle: ConversationLifecycle) -> ConversationTurn {
    ConversationTurn::new(id, lifecycle)
}

fn item(plan: ConversationPlan) -> ConversationItem {
    ConversationItem::Plan(plan)
}

fn active_plan(id: &str, turn_id: &str, ordinal: u64, revision: u64) -> ConversationPlan {
    plan(
        id,
        turn_id,
        ordinal,
        revision,
        ConversationPlanState::Active,
        &[ConversationPlanEntryState::Active],
    )
}

#[test]
fn each_entry_state_matches_the_open_work_predicate() {
    let cases = [
        (ConversationPlanEntryState::Pending, true),
        (ConversationPlanEntryState::Active, true),
        (ConversationPlanEntryState::Completed, false),
        (ConversationPlanEntryState::Skipped, false),
    ];

    for (state, expected) in cases {
        let plan = plan(
            "plan",
            "turn",
            1,
            0,
            ConversationPlanState::Active,
            &[state],
        );
        assert_eq!(
            conversation_plan_has_open_entries(&plan),
            expected,
            "{state:?}"
        );
        assert_eq!(plan.has_open_entries(), expected, "method for {state:?}");
    }
}

#[test]
fn open_work_requires_at_least_one_pending_or_active_entry() {
    let no_entries = plan("empty", "turn", 1, 0, ConversationPlanState::Completed, &[]);
    let only_closed = plan(
        "closed",
        "turn",
        2,
        0,
        ConversationPlanState::Draft,
        &[
            ConversationPlanEntryState::Completed,
            ConversationPlanEntryState::Skipped,
        ],
    );
    let mixed = plan(
        "mixed",
        "turn",
        3,
        0,
        ConversationPlanState::Completed,
        &[
            ConversationPlanEntryState::Completed,
            ConversationPlanEntryState::Pending,
            ConversationPlanEntryState::Skipped,
        ],
    );

    assert!(!conversation_plan_has_open_entries(&no_entries));
    assert!(!conversation_plan_has_open_entries(&only_closed));
    assert!(conversation_plan_has_open_entries(&mixed));
}

#[test]
fn non_plan_items_are_ignored_during_candidate_selection() {
    let selected = active_plan("selected", "turn", 9, 0);
    let items = vec![
        ConversationItem::Other,
        item(active_plan("older", "turn", 2, 99)),
        ConversationItem::Other,
        item(selected.clone()),
    ];

    let result = latest_conversation_plan(&items, &[turn("turn", ConversationLifecycle::Active)]);

    assert_eq!(result, Some(&selected));

    let only_non_plans = [ConversationItem::Other, ConversationItem::Other];
    assert_eq!(
        latest_conversation_plan(
            &only_non_plans,
            &[turn("turn", ConversationLifecycle::Active)]
        ),
        None
    );
}

#[test]
fn latest_selection_does_not_require_open_entries() {
    let selected = plan(
        "selected",
        "turn",
        9,
        0,
        ConversationPlanState::Active,
        &[
            ConversationPlanEntryState::Completed,
            ConversationPlanEntryState::Skipped,
        ],
    );
    let items = [item(selected.clone())];

    assert!(!conversation_plan_has_open_entries(&selected));
    assert_eq!(
        latest_conversation_plan(&items, &[turn("turn", ConversationLifecycle::Active)]),
        Some(&selected)
    );
}

#[test]
fn candidate_order_uses_ordinal_then_id_then_revision() {
    let highest_ordinal = active_plan("a", "turn", 8, 0);
    let highest_id = active_plan("z", "turn", 7, 0);
    let highest_revision = active_plan("same", "turn", 6, 4);
    let lower_revision = active_plan("same", "turn", 6, 1);
    let items = vec![
        item(highest_revision),
        item(lower_revision),
        item(highest_id),
        item(highest_ordinal.clone()),
    ];

    let result = latest_conversation_plan(&items, &[turn("turn", ConversationLifecycle::Active)]);

    assert_eq!(result, Some(&highest_ordinal));

    let same_ordinal = vec![
        item(active_plan("a", "turn", 7, 90)),
        item(active_plan("z", "turn", 7, 0)),
    ];
    let result = latest_conversation_plan(
        &same_ordinal,
        &[turn("turn", ConversationLifecycle::Active)],
    );
    assert_eq!(result.map(|plan| plan.id.as_str()), Some("z"));

    let same_id_and_ordinal = vec![
        item(active_plan("same", "turn", 7, 2)),
        item(active_plan("same", "turn", 7, 3)),
    ];
    let result = latest_conversation_plan(
        &same_id_and_ordinal,
        &[turn("turn", ConversationLifecycle::Active)],
    );
    assert_eq!(result.map(|plan| plan.revision), Some(3));
}

#[test]
fn equal_keys_keep_the_first_input_plan_deterministically() {
    let first = active_plan("same", "turn", 4, 2);
    let second = plan(
        "same",
        "turn",
        4,
        2,
        ConversationPlanState::Draft,
        &[ConversationPlanEntryState::Completed],
    );
    let items = vec![item(first.clone()), item(second)];

    let result = latest_conversation_plan(&items, &[turn("turn", ConversationLifecycle::Active)]);

    assert_eq!(result, Some(&first));
}

#[test]
fn an_inactive_latest_plan_supersedes_an_older_active_plan() {
    for inactive_state in [
        ConversationPlanState::Draft,
        ConversationPlanState::Completed,
        ConversationPlanState::Abandoned,
    ] {
        let items = vec![
            item(active_plan("older", "turn", 1, 0)),
            item(plan("latest", "turn", 2, 0, inactive_state, &[])),
        ];
        let turns = [turn("turn", ConversationLifecycle::Active)];

        assert_eq!(
            latest_conversation_plan(&items, &turns),
            None,
            "inactive latest state {inactive_state:?} must clear the checklist"
        );
    }
}

#[test]
fn an_active_plan_requires_an_existing_owner_turn() {
    let items = vec![item(active_plan("plan", "owner", 1, 0))];

    assert_eq!(latest_conversation_plan(&items, &[]), None);
    assert_eq!(
        latest_conversation_plan(&items, &[turn("different", ConversationLifecycle::Active)]),
        None
    );
}

#[test]
fn every_turn_lifecycle_is_classified_for_checklist_eligibility() {
    let cases = [
        (ConversationLifecycle::Pending, true),
        (ConversationLifecycle::Streaming, true),
        (ConversationLifecycle::Active, true),
        (ConversationLifecycle::Waiting, true),
        (ConversationLifecycle::Completed, false),
        (ConversationLifecycle::Failed, false),
        (ConversationLifecycle::Interrupted, false),
        (ConversationLifecycle::Cancelled, false),
    ];
    let items = [item(active_plan("plan", "owner", 1, 0))];

    for (lifecycle, expected) in cases {
        let turns = [turn("owner", lifecycle)];
        assert_eq!(
            latest_conversation_plan(&items, &turns).is_some(),
            expected,
            "owner lifecycle {lifecycle:?}"
        );
        assert_eq!(lifecycle.is_checklist_eligible(), expected);
    }
}

#[test]
fn selection_borrows_inputs_without_mutating_them() {
    let items = vec![
        item(active_plan("older", "turn", 1, 0)),
        ConversationItem::Other,
        item(active_plan("latest", "turn", 2, 1)),
    ];
    let turns = vec![turn("turn", ConversationLifecycle::Waiting)];
    let items_before = items.clone();
    let turns_before = turns.clone();

    let result = latest_conversation_plan(&items, &turns);

    assert_eq!(result.map(|plan| plan.id.as_str()), Some("latest"));
    assert_eq!(items, items_before);
    assert_eq!(turns, turns_before);
}
