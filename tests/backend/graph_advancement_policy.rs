//! Focused table tests for dependency-free graph advancement convergence.

#![allow(dead_code)]

#[path = "../../modules/backend/src/graph_advancement_policy.rs"]
mod graph_advancement_policy;

use std::cell::RefCell;
use std::rc::Rc;

use graph_advancement_policy::{
    GraphAdvancementError, GraphAdvancementPolicy, GraphTransitionInput, advance_graph,
};

fn input() -> GraphTransitionInput {
    GraphTransitionInput::new("cause-1", "correlation-1", "group-1", "thread-1")
}

fn queued_waves(
    waves: Vec<Vec<&'static str>>,
) -> impl FnMut(&GraphTransitionInput) -> Vec<&'static str> {
    let mut waves = waves.into_iter();
    move |_| waves.next().unwrap_or_default()
}

#[test]
fn immediate_convergence_runs_one_empty_wave_then_group_update() {
    let dependency_calls = Rc::new(RefCell::new(0));
    let join_calls = Rc::new(RefCell::new(0));
    let group_calls = Rc::new(RefCell::new(0));
    let dependency_calls_seen = Rc::clone(&dependency_calls);
    let join_calls_seen = Rc::clone(&join_calls);
    let group_calls_seen = Rc::clone(&group_calls);

    let result = advance_graph(
        input(),
        1,
        move |_| {
            *dependency_calls_seen.borrow_mut() += 1;
            Vec::<&str>::new()
        },
        move |_| {
            *join_calls_seen.borrow_mut() += 1;
            Vec::<&str>::new()
        },
        move |_| {
            *group_calls_seen.borrow_mut() += 1;
            vec!["group"]
        },
    )
    .expect("empty first wave converges");

    assert_eq!(result.events, vec!["group"]);
    assert_eq!(result.iterations, 1);
    assert_eq!(*dependency_calls.borrow(), 1);
    assert_eq!(*join_calls.borrow(), 1);
    assert_eq!(*group_calls.borrow(), 1);
}

#[test]
fn dependency_only_waves_retain_dependency_order() {
    let result = advance_graph(
        input(),
        3,
        queued_waves(vec![vec!["dependency-1"], vec!["dependency-2"], vec![]]),
        queued_waves(vec![vec![], vec![], vec![]]),
        |_| vec!["group"],
    )
    .expect("third empty wave converges");

    assert_eq!(result.events, vec!["dependency-1", "dependency-2", "group"]);
    assert_eq!(result.iterations, 3);
}

#[test]
fn join_only_waves_retain_join_order() {
    let result = advance_graph(
        input(),
        3,
        queued_waves(vec![vec![], vec![], vec![]]),
        queued_waves(vec![vec!["join-1"], vec!["join-2"], vec![]]),
        |_| vec!["group"],
    )
    .expect("third empty wave converges");

    assert_eq!(result.events, vec!["join-1", "join-2", "group"]);
    assert_eq!(result.iterations, 3);
}

#[test]
fn mixed_multiwave_order_is_dependency_then_join_for_each_wave() {
    let result = advance_graph(
        input(),
        3,
        queued_waves(vec![
            vec!["dependency-1", "dependency-2"],
            vec!["dependency-3"],
            vec![],
        ]),
        queued_waves(vec![vec!["join-1"], vec!["join-2", "join-3"], vec![]]),
        |_| vec!["group"],
    )
    .expect("third empty wave converges");

    assert_eq!(
        result.events,
        vec![
            "dependency-1",
            "dependency-2",
            "join-1",
            "dependency-3",
            "join-2",
            "join-3",
            "group",
        ]
    );
}

#[test]
fn empty_terminal_wave_is_evaluated_and_not_emitted() {
    let call_order = Rc::new(RefCell::new(Vec::new()));
    let dependency_order = Rc::clone(&call_order);
    let join_order = Rc::clone(&call_order);
    let group_order = Rc::clone(&call_order);
    let dependency_waves = Rc::new(RefCell::new(vec![vec!["dependency"], vec![]]));
    let join_waves = Rc::new(RefCell::new(vec![vec!["join"], vec![]]));
    let dependency_waves_seen = Rc::clone(&dependency_waves);
    let join_waves_seen = Rc::clone(&join_waves);

    let result = advance_graph(
        input(),
        2,
        move |_| {
            dependency_order.borrow_mut().push("dependency");
            dependency_waves_seen.borrow_mut().remove(0)
        },
        move |_| {
            join_order.borrow_mut().push("join");
            join_waves_seen.borrow_mut().remove(0)
        },
        move |_| {
            group_order.borrow_mut().push("group");
            vec!["group"]
        },
    )
    .expect("second empty wave converges");

    assert_eq!(result.events, vec!["dependency", "join", "group"]);
    assert_eq!(
        *call_order.borrow(),
        vec!["dependency", "join", "dependency", "join", "group"]
    );
    assert_eq!(result.iterations, 2);
}

#[test]
fn group_state_events_are_appended_once_after_all_waves() {
    let group_calls = Rc::new(RefCell::new(0));
    let group_calls_seen = Rc::clone(&group_calls);
    let result = advance_graph(
        input(),
        2,
        queued_waves(vec![vec!["dependency"], vec![]]),
        queued_waves(vec![vec!["join"], vec![]]),
        move |_| {
            *group_calls_seen.borrow_mut() += 1;
            vec!["group-1", "group-2"]
        },
    )
    .expect("second wave converges");

    assert_eq!(
        result.events,
        vec!["dependency", "join", "group-1", "group-2"]
    );
    assert_eq!(*group_calls.borrow(), 1);
}

#[test]
fn transition_identity_is_preserved_for_every_callback_and_result() {
    let expected = input();
    let observed = Rc::new(RefCell::new(Vec::new()));
    let dependency_observed = Rc::clone(&observed);
    let join_observed = Rc::clone(&observed);
    let group_observed = Rc::clone(&observed);

    let result = GraphAdvancementPolicy::advance(
        &expected,
        2,
        move |transition| {
            dependency_observed.borrow_mut().push(transition.clone());
            if dependency_observed.borrow().len() == 1 {
                vec!["dependency"]
            } else {
                vec![]
            }
        },
        move |transition| {
            join_observed.borrow_mut().push(transition.clone());
            if join_observed.borrow().len() == 2 {
                vec!["join"]
            } else {
                vec![]
            }
        },
        move |transition| {
            group_observed.borrow_mut().push(transition.clone());
            vec![]
        },
    )
    .expect("second wave converges");

    assert_eq!(result.input(), &expected);
    assert_eq!(result.iterations(), 2);
    assert_eq!(
        observed.borrow().as_slice(),
        &[
            expected.clone(),
            expected.clone(),
            expected.clone(),
            expected.clone(),
            expected,
        ]
    );
}

#[test]
fn finite_bound_failure_retains_partial_events_and_is_explicit() {
    let dependency_calls = Rc::new(RefCell::new(0));
    let dependency_calls_seen = Rc::clone(&dependency_calls);
    let error = advance_graph(
        input(),
        2,
        move |_| {
            *dependency_calls_seen.borrow_mut() += 1;
            vec!["still-changing"]
        },
        |_| vec![],
        |_| panic!("group update must not run on non-convergence"),
    )
    .expect_err("finite bound must reject non-convergence");

    assert!(matches!(
        error,
        GraphAdvancementError::IterationLimitExceeded { .. }
    ));
    assert_eq!(error.events(), &["still-changing", "still-changing"]);
    assert_eq!(error.iterations(), 2);
    assert_eq!(error.max_iterations(), 2);
    assert_eq!(*dependency_calls.borrow(), 2);
    assert!(error.to_string().contains("did not converge"));
}

#[test]
fn no_group_update_occurs_when_the_bound_expires() {
    let group_calls = Rc::new(RefCell::new(0));
    let group_calls_seen = Rc::clone(&group_calls);
    let result = advance_graph(
        input(),
        1,
        |_| vec!["dependency"],
        |_| vec!["join"],
        move |_| {
            *group_calls_seen.borrow_mut() += 1;
            vec!["group"]
        },
    );

    assert!(result.is_err());
    assert_eq!(*group_calls.borrow(), 0);
}
