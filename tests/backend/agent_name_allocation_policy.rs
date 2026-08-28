//! Focused parity tests for dependency-free agent-name allocation.

#![allow(dead_code)]

#[path = "../../modules/backend/src/agent_name_allocation_policy.rs"]
mod agent_name_allocation_policy;

use agent_name_allocation_policy::{
    AgentNameAllocationError, AgentNameAllocationPolicy, FALLBACK_AGENT_NAME,
    RESERVED_COORDINATOR_NAME, VISIBLE_NAME_MAXIMUM, choose_available_agent_name,
};

fn choose(
    name_bank: &[&str],
    existing_display_names: &[&str],
    selector: usize,
) -> Result<String, AgentNameAllocationError> {
    choose_available_agent_name(name_bank, existing_display_names, selector)
}

#[test]
fn empty_bank_uses_agent_fallback_and_preserves_ordered_selection() {
    assert_eq!(choose(&[], &[], 0), Ok(FALLBACK_AGENT_NAME.to_owned()));
    assert_eq!(
        choose(&[], &["agent"], 0),
        Ok(format!("{FALLBACK_AGENT_NAME} 2"))
    );
}

#[test]
fn first_occurrence_deduplicates_bases_case_insensitively() {
    assert_eq!(
        choose(&["ALPHA", "alpha", "Beta"], &["beta"], 0),
        Ok("ALPHA".to_owned())
    );
    assert_eq!(
        choose(&["ALPHA", "alpha"], &[], 1),
        Err(AgentNameAllocationError::SelectorOutOfRange {
            selector: 1,
            candidate_count: 1,
        })
    );
}

#[test]
fn existing_collisions_are_case_insensitive_and_suffixes_wait_for_all_bases() {
    assert_eq!(
        choose(&["Alpha", "Beta", "Gamma"], &["ALPHA", "beta"], 0),
        Ok("Gamma".to_owned())
    );
    assert_eq!(
        choose(&["Alpha", "Beta", "Gamma"], &["ALPHA", "beta", "gamma"], 0),
        Ok("Alpha 2".to_owned())
    );
}

#[test]
fn all_available_indices_follow_base_order() {
    let bank = ["First", "Second", "Third"];
    let existing = ["second"];

    assert_eq!(choose(&bank, &existing, 0), Ok("First".to_owned()));
    assert_eq!(choose(&bank, &existing, 1), Ok("Third".to_owned()));
    assert_eq!(
        choose(&bank, &existing, 2),
        Err(AgentNameAllocationError::SelectorOutOfRange {
            selector: 2,
            candidate_count: 2,
        })
    );
}

#[test]
fn out_of_range_selector_is_rejected_without_generation_bias() {
    let error = choose(&["First", "Second"], &[], 2).unwrap_err();
    assert_eq!(
        error,
        AgentNameAllocationError::SelectorOutOfRange {
            selector: 2,
            candidate_count: 2,
        }
    );

    let error = choose(&["First", "Second"], &["first", "second"], 2).unwrap_err();
    assert_eq!(
        error,
        AgentNameAllocationError::SelectorOutOfRange {
            selector: 2,
            candidate_count: 2,
        }
    );
}

#[test]
fn coordinator_is_reserved_permanently_but_numbered_coordinator_is_available() {
    assert_eq!(
        choose(&["Coordinator", "Worker"], &[], 0),
        Ok("Worker".to_owned())
    );
    assert_eq!(
        choose(&[RESERVED_COORDINATOR_NAME], &[], 0),
        Ok("coordinator 2".to_owned())
    );
}

#[test]
fn empty_base_is_an_effective_first_occurrence_and_gets_suffixes() {
    assert_eq!(choose(&[""], &[], 0), Ok(String::new()));
    assert_eq!(choose(&[""], &[""], 0), Ok(" 2".to_owned()));
}

#[test]
fn suffix_generation_preserves_base_case_and_fits_the_visible_limit() {
    let long_base = "A".repeat(VISIBLE_NAME_MAXIMUM + 8);
    let expected_generation_two = format!(
        "{} 2",
        "A".repeat(VISIBLE_NAME_MAXIMUM - " 2".chars().count())
    );

    assert_eq!(choose(&[long_base.as_str()], &[], 0), Ok(long_base.clone()));
    assert_eq!(
        choose(&[long_base.as_str()], &[long_base.as_str()], 0),
        Ok(expected_generation_two.clone())
    );
    assert_eq!(
        expected_generation_two.chars().count(),
        VISIBLE_NAME_MAXIMUM
    );
}

#[test]
fn suffix_length_changes_the_retained_base_prefix_at_generation_boundaries() {
    let long_base = "x".repeat(VISIBLE_NAME_MAXIMUM + 10);
    let generation_ten = format!(
        "{} 10",
        "x".repeat(VISIBLE_NAME_MAXIMUM - " 10".chars().count())
    );
    let mut occupied_generations = vec![long_base.clone()];
    for generation in 2..=9 {
        occupied_generations.push(format!(
            "{} {generation}",
            "x".repeat(VISIBLE_NAME_MAXIMUM - format!(" {generation}").chars().count())
        ));
    }
    let occupied_generations = occupied_generations
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>();

    let selected = choose(&[long_base.as_str()], &occupied_generations, 0);
    assert_eq!(selected, Ok(generation_ten));
}

#[test]
fn exact_boundary_base_remains_whole_before_suffixing() {
    let base = "b".repeat(VISIBLE_NAME_MAXIMUM);
    assert_eq!(choose(&[base.as_str()], &[], 0), Ok(base.clone()));
    assert_eq!(
        choose(&[base.as_str()], &[base.as_str()], 0),
        Ok(format!(
            "{} 2",
            "b".repeat(VISIBLE_NAME_MAXIMUM - " 2".chars().count())
        ))
    );
}

#[test]
fn policy_value_and_free_function_share_the_same_deterministic_result() {
    let selected = AgentNameAllocationPolicy::choose(&["Ada", "Grace"], &[], 1);
    assert_eq!(selected, Ok("Grace".to_owned()));

    let selected =
        AgentNameAllocationPolicy::choose_available_agent_name(&["Ada", "Grace"], &[], 0);
    assert_eq!(selected, Ok("Ada".to_owned()));
}

#[test]
fn duplicate_truncated_candidates_keep_the_source_order_and_multiplicity() {
    let first = format!("{}-first", "a".repeat(VISIBLE_NAME_MAXIMUM));
    let second = format!("{}-second", "a".repeat(VISIBLE_NAME_MAXIMUM));
    let existing = [first.as_str(), second.as_str()];

    let expected = format!("{} 2", "a".repeat(VISIBLE_NAME_MAXIMUM - 2));
    assert_eq!(
        choose(&[first.as_str(), second.as_str()], &existing, 0),
        Ok(expected.clone())
    );
    assert_eq!(
        choose(&[first.as_str(), second.as_str()], &existing, 1),
        Ok(expected)
    );
}
