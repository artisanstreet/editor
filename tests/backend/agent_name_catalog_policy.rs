//! Focused parity tests for the dependency-free agent-name catalog policy.

#![allow(dead_code)]

#[path = "../../modules/backend/src/agent_name_catalog_policy.rs"]
mod agent_name_catalog_policy;

use agent_name_catalog_policy::{
    AgentNameCatalogBanks, AgentNameCatalogDefaultsRead, AgentNameCatalogError,
    AgentNameCatalogInput, AgentNameCatalogPolicy, BRITISH_DATASET_ID,
    DEFAULT_AGENT_NAME_DATASET_ID, DEFAULT_DATASET_ID, NORWEGIAN_DATASET_ID,
    resolve_agent_name_catalog,
};

fn input<'banks, 'name, 'selection>(
    norwegian: &'banks [&'name str],
    british: &'banks [&'name str],
    selection: Option<&'selection str>,
) -> AgentNameCatalogInput<'banks, 'name, 'selection> {
    AgentNameCatalogInput::new(
        norwegian,
        british,
        AgentNameCatalogDefaultsRead::new(selection),
    )
}

#[test]
fn protocol_dataset_ids_and_default_are_exact() {
    assert_eq!(NORWEGIAN_DATASET_ID, "norwegian");
    assert_eq!(BRITISH_DATASET_ID, "british");
    assert_eq!(DEFAULT_AGENT_NAME_DATASET_ID, "norwegian");
    assert_eq!(DEFAULT_DATASET_ID, DEFAULT_AGENT_NAME_DATASET_ID);
}

#[test]
fn completed_defaults_read_preserves_missing_and_explicit_values() {
    assert_eq!(
        AgentNameCatalogDefaultsRead::missing(),
        AgentNameCatalogDefaultsRead::new(None)
    );
    assert_eq!(
        AgentNameCatalogDefaultsRead::new(Some(" future ")).agent_name_dataset,
        Some(" future ")
    );
}

#[test]
fn empty_norwegian_bank_is_rejected_before_any_selection() {
    let british = ["Ada"];

    for selection in [
        None,
        Some(NORWEGIAN_DATASET_ID),
        Some(BRITISH_DATASET_ID),
        Some("future"),
    ] {
        assert_eq!(
            resolve_agent_name_catalog(input(&[], &british, selection)),
            Err(AgentNameCatalogError::EmptyBank {
                dataset_id: NORWEGIAN_DATASET_ID,
            })
        );
    }
}

#[test]
fn empty_british_bank_is_rejected_even_when_norwegian_is_selected() {
    let norwegian = ["Nora"];

    for selection in [
        None,
        Some(NORWEGIAN_DATASET_ID),
        Some(BRITISH_DATASET_ID),
        Some("future"),
    ] {
        assert_eq!(
            resolve_agent_name_catalog(input(&norwegian, &[], selection)),
            Err(AgentNameCatalogError::EmptyBank {
                dataset_id: BRITISH_DATASET_ID,
            })
        );
    }
}

#[test]
fn empty_entries_are_rejected_in_each_bank_with_their_exact_index() {
    let norwegian = ["Nora", "", "Ingrid"];
    let british = ["Ada", "", "Grace"];

    assert_eq!(
        AgentNameCatalogBanks::new(&norwegian, &british).validate(),
        Err(AgentNameCatalogError::EmptyName {
            dataset_id: NORWEGIAN_DATASET_ID,
            index: 1,
        })
    );

    let norwegian = ["Nora", "Ingrid"];
    assert_eq!(
        AgentNameCatalogBanks::new(&norwegian, &british).validate(),
        Err(AgentNameCatalogError::EmptyName {
            dataset_id: BRITISH_DATASET_ID,
            index: 1,
        })
    );
}

#[test]
fn both_invalid_banks_report_the_first_canonical_failure() {
    let norwegian = [""];
    let british = [""];

    assert_eq!(
        AgentNameCatalogPolicy::resolve(input(&norwegian, &british, Some(BRITISH_DATASET_ID),)),
        Err(AgentNameCatalogError::EmptyName {
            dataset_id: NORWEGIAN_DATASET_ID,
            index: 0,
        })
    );
}

#[test]
fn whitespace_only_names_are_non_empty_and_remain_exact() {
    let norwegian = ["  ", "\u{2003}"];
    let british = ["Ada"];

    let selected =
        resolve_agent_name_catalog(input(&norwegian, &british, Some(NORWEGIAN_DATASET_ID)))
            .expect("whitespace-only strings are non-empty names");

    assert_eq!(selected, &norwegian);
}

#[test]
fn missing_durable_selection_uses_the_protocol_default_norwegian_bank() {
    let norwegian = ["first", "second"];
    let british = ["other"];

    let selected = resolve_agent_name_catalog(input(&norwegian, &british, None))
        .expect("both bundled banks are valid");

    assert_eq!(selected, &norwegian);
    assert!(std::ptr::eq(selected, &norwegian));
}

#[test]
fn known_dataset_ids_return_the_exact_corresponding_ordered_bank() {
    let norwegian = ["Nora", "Ada", "Nora", "Solveig"];
    let british = ["Zoe", "Ada", "Grace", "Zoe"];

    let selected_norwegian =
        resolve_agent_name_catalog(input(&norwegian, &british, Some(NORWEGIAN_DATASET_ID)))
            .expect("known Norwegian selection succeeds");
    let selected_british =
        resolve_agent_name_catalog(input(&norwegian, &british, Some(BRITISH_DATASET_ID)))
            .expect("known British selection succeeds");

    assert_eq!(selected_norwegian, &norwegian);
    assert_eq!(selected_british, &british);
    assert!(std::ptr::eq(selected_norwegian, &norwegian));
    assert!(std::ptr::eq(selected_british, &british));
}

#[test]
fn unknown_and_future_dataset_ids_fall_back_to_norwegian_exactly() {
    let norwegian = ["Nørå", "Åse", "🦀井"];
    let british = ["Élodie", "Zoë"];
    let unknown = [
        "future", "playful", "", "British", " british", "british ", "🧭",
    ];

    for selection in unknown {
        let selected = resolve_agent_name_catalog(input(&norwegian, &british, Some(selection)))
            .expect("unknown selection falls back after validation");
        assert_eq!(selected, &norwegian, "selection {selection:?}");
        assert!(std::ptr::eq(selected, &norwegian));
    }
}

#[test]
fn unicode_name_spelling_order_and_duplicates_are_not_rewritten() {
    let norwegian = ["Åsa", "🦀", "Åsa", "e\u{301}", "é", "  No trim  "];
    let british = ["Zoë", "Élodie", "Zoë"];

    let selected =
        AgentNameCatalogPolicy::select(input(&norwegian, &british, Some(NORWEGIAN_DATASET_ID)))
            .expect("Unicode banks are valid");

    assert_eq!(
        selected,
        &["Åsa", "🦀", "Åsa", "e\u{301}", "é", "  No trim  "]
    );
    assert_eq!(selected[1], "🦀");
    assert_eq!(selected[0], selected[2], "duplicate entries are retained");
    assert_ne!(selected[3], selected[4], "no Unicode normalization occurs");
}

#[test]
fn repeated_selection_is_pure_and_keeps_each_bank_identity() {
    let norwegian = ["First", "Second", "First"];
    let british = ["Third", "Fourth"];
    let banks = AgentNameCatalogBanks::new(&norwegian, &british);
    let defaults = AgentNameCatalogDefaultsRead::new(Some(BRITISH_DATASET_ID));

    let validated = banks.validate().expect("both banks are valid");
    let first = validated.select(defaults.agent_name_dataset);
    let second = validated.select(defaults.agent_name_dataset);
    let third = AgentNameCatalogPolicy::resolve(AgentNameCatalogInput {
        banks,
        durable_defaults: defaults,
    })
    .expect("repeated resolution remains valid");

    assert_eq!(first, &british);
    assert_eq!(second, first);
    assert_eq!(third, first);
    assert!(std::ptr::eq(first, &british));
    assert!(std::ptr::eq(second, &british));
    assert!(std::ptr::eq(third, &british));
}

#[test]
fn error_accessors_and_display_retain_the_validation_fact() {
    let bank_error = AgentNameCatalogError::EmptyBank {
        dataset_id: BRITISH_DATASET_ID,
    };
    assert_eq!(bank_error.dataset_id(), BRITISH_DATASET_ID);
    assert_eq!(bank_error.name_index(), None);
    assert_eq!(
        bank_error.to_string(),
        "british agent-name bank must not be empty"
    );

    let name_error = AgentNameCatalogError::EmptyName {
        dataset_id: NORWEGIAN_DATASET_ID,
        index: 3,
    };
    assert_eq!(name_error.dataset_id(), NORWEGIAN_DATASET_ID);
    assert_eq!(name_error.name_index(), Some(3));
    assert_eq!(
        name_error.to_string(),
        "norwegian agent-name bank entry at index 3 must not be empty"
    );
}
