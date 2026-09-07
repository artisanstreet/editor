#[path = "../../modules/frontend/src/project_identity_policy.rs"]
mod project_identity_policy;

use std::time::Duration;

use project_identity_policy::{
    COLD_START_RETRY_SCHEDULE_DURATION, ColdStartRetryState, MAX_RETAINED_PROJECT_IDENTITIES,
    ProjectIdentity, ProjectIdentityPolicy, ProjectIdentityState, RefreshAdmission, RetryIntent,
};

fn identity(project_id: &str, metadata: &str) -> ProjectIdentity<String> {
    ProjectIdentity::new(project_id, metadata.to_owned())
}

fn response(project_ids: &[&str]) -> Vec<ProjectIdentity<String>> {
    project_ids
        .iter()
        .map(|project_id| identity(project_id, project_id))
        .collect()
}

fn state_ids(state: &ProjectIdentityState<String>) -> Vec<&str> {
    state
        .identities()
        .iter()
        .map(|identity| identity.project_id.as_str())
        .collect()
}

#[test]
fn refresh_admission_table_preserves_one_batch_and_empty_noop() {
    let empty: Vec<String> = Vec::new();
    let requested = vec![
        "project-a".to_owned(),
        "project-a".to_owned(),
        "project-b".to_owned(),
    ];
    let returned = response(&["project-a"]);

    assert_eq!(
        ProjectIdentityPolicy::<String>::admit_refresh(&empty),
        RefreshAdmission::NoOp
    );
    assert_eq!(
        ProjectIdentityPolicy::<String>::admit_refresh(&requested),
        RefreshAdmission::BatchLookup {
            project_ids: requested,
        }
    );
    assert_eq!(returned.len(), 1);
}

#[test]
fn retry_sequence_is_exponential_and_bounded_by_elapsed_schedule_duration() {
    let mut schedule = ColdStartRetryState::new();
    let mut intents = Vec::new();

    while let RetryIntent::Retry {
        attempt,
        delay,
        elapsed,
    } = schedule.next_intent()
    {
        intents.push((attempt, delay, elapsed));
    }

    assert_eq!(
        intents,
        vec![
            (1, Duration::from_millis(100), Duration::ZERO),
            (2, Duration::from_millis(200), Duration::from_millis(100)),
            (3, Duration::from_millis(400), Duration::from_millis(300)),
            (4, Duration::from_millis(800), Duration::from_millis(700)),
            (
                5,
                Duration::from_millis(1_600),
                Duration::from_millis(1_500)
            ),
            (
                6,
                Duration::from_millis(3_200),
                Duration::from_millis(3_100)
            ),
        ]
    );
    assert!(
        intents
            .last()
            .is_some_and(|(_, _, elapsed)| *elapsed <= COLD_START_RETRY_SCHEDULE_DURATION)
    );
    let (_, final_delay, final_elapsed) = intents[5];
    assert!(final_elapsed + final_delay > COLD_START_RETRY_SCHEDULE_DURATION);
    assert!(schedule.is_exhausted());
    assert_eq!(schedule.next_intent(), RetryIntent::Exhausted);
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct ClientFailure;

#[test]
fn final_client_failure_escapes_without_mutating_state() {
    let mut policy = ProjectIdentityPolicy::new();
    policy
        .apply_client_result::<ClientFailure>(Ok(response(&["existing"])))
        .unwrap();
    let before = policy.clone();

    let result = policy.apply_client_result::<ClientFailure>(Err(ClientFailure));

    assert_eq!(result, Err(ClientFailure));
    assert_eq!(policy, before);
}

#[test]
fn partial_response_updates_only_identities_returned() {
    let mut policy = ProjectIdentityPolicy::new();
    policy
        .apply_client_result::<ClientFailure>(Ok(vec![
            identity("one", "one-old"),
            identity("two", "two-old"),
            identity("three", "three-old"),
        ]))
        .unwrap();

    policy
        .apply_client_result::<ClientFailure>(Ok(vec![identity("two", "two-new")]))
        .unwrap();

    assert_eq!(state_ids(policy.state()), vec!["one", "three", "two"]);
    assert_eq!(policy.state().identities()[0].metadata, "one-old");
    assert_eq!(policy.state().identities()[1].metadata, "three-old");
    assert_eq!(policy.state().identities()[2].metadata, "two-new");
}

#[test]
fn replacement_uses_delete_then_set_recency() {
    let mut state = ProjectIdentityState::new();
    state.apply_response(response(&["a", "b", "c"]));

    let action = state.apply_response(vec![identity("b", "b-replaced")]);

    assert_eq!(action.returned_project_ids, vec!["b"]);
    assert!(action.evicted_project_ids.is_empty());
    assert_eq!(state_ids(&state), vec!["a", "c", "b"]);
    assert_eq!(state.identities()[2].metadata, "b-replaced");
}

#[test]
fn multiple_returned_identities_follow_response_order_while_untouched_order_is_relative() {
    let mut state = ProjectIdentityState::new();
    state.apply_response(response(&["a", "b", "c", "d"]));

    state.apply_response(vec![
        identity("c", "c-new"),
        identity("a", "a-new"),
        identity("e", "e-new"),
    ]);

    assert_eq!(state_ids(&state), vec!["b", "d", "c", "a", "e"]);
    assert_eq!(state.identities()[2].metadata, "c-new");
    assert_eq!(state.identities()[3].metadata, "a-new");
}

#[test]
fn capacity_edge_accepts_exactly_128_identities() {
    let mut state = ProjectIdentityState::new();
    assert!(state.is_empty());
    let identities = (0..MAX_RETAINED_PROJECT_IDENTITIES)
        .map(|index| identity(&format!("project-{index}"), "metadata"))
        .collect();

    let action = state.apply_response(identities);

    assert_eq!(state.len(), MAX_RETAINED_PROJECT_IDENTITIES);
    assert!(action.evicted_project_ids.is_empty());
    assert_eq!(state.identities()[0].project_id, "project-0");
    assert_eq!(state.identities()[127].project_id, "project-127");
}

#[test]
fn eviction_is_oldest_first_after_complete_response() {
    let mut state = ProjectIdentityState::new();
    state.apply_response(response(&["old-0", "old-1", "old-2", "old-3"]));

    let mut returned = Vec::new();
    for index in 4..(MAX_RETAINED_PROJECT_IDENTITIES + 3) {
        returned.push(identity(&format!("new-{index}"), "metadata"));
    }
    let action = state.apply_response(returned);

    assert_eq!(state.len(), MAX_RETAINED_PROJECT_IDENTITIES);
    assert_eq!(action.evicted_project_ids, vec!["old-0", "old-1", "old-2"]);
    assert_eq!(state.identities()[0].project_id, "old-3");
    assert_eq!(state.identities()[127].project_id, "new-130");
}
