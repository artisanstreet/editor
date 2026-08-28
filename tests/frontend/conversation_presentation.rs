//! Exhaustive truth-table coverage for conversation disclosure presentation.
//!
//! The cases mirror the pure TypeScript contract and its Svelte call site:
//! initial-open inputs include deliberately unused details/failure facts, and
//! disclosure inputs distinguish defined details, visible details, working
//! state, caller-owned open state, data attributes, hidden state, and mounting.

use artisan_frontend::conversation_presentation::{
    WorkSessionDisclosureInput, WorkSessionDisclosureOutput, WorkSessionInitialOpenInput,
    work_session_disclosure, work_session_initially_open,
};

#[test]
fn initially_open_matches_working_for_every_input_combination() {
    struct Case {
        input: WorkSessionInitialOpenInput,
        expected: bool,
    }

    // All 2^3 combinations are listed explicitly. In particular, details and
    // unsuccessful vary independently of working to guard their intentional
    // non-effect on the result.
    let cases = [
        Case {
            input: WorkSessionInitialOpenInput {
                has_details: false,
                unsuccessful: false,
                working: false,
            },
            expected: false,
        },
        Case {
            input: WorkSessionInitialOpenInput {
                has_details: false,
                unsuccessful: false,
                working: true,
            },
            expected: true,
        },
        Case {
            input: WorkSessionInitialOpenInput {
                has_details: false,
                unsuccessful: true,
                working: false,
            },
            expected: false,
        },
        Case {
            input: WorkSessionInitialOpenInput {
                has_details: false,
                unsuccessful: true,
                working: true,
            },
            expected: true,
        },
        Case {
            input: WorkSessionInitialOpenInput {
                has_details: true,
                unsuccessful: false,
                working: false,
            },
            expected: false,
        },
        Case {
            input: WorkSessionInitialOpenInput {
                has_details: true,
                unsuccessful: false,
                working: true,
            },
            expected: true,
        },
        Case {
            input: WorkSessionInitialOpenInput {
                has_details: true,
                unsuccessful: true,
                working: false,
            },
            expected: false,
        },
        Case {
            input: WorkSessionInitialOpenInput {
                has_details: true,
                unsuccessful: true,
                working: true,
            },
            expected: true,
        },
    ];

    for case in cases {
        assert_eq!(
            work_session_initially_open(case.input),
            case.expected,
            "initial-open mismatch for {:?}",
            case.input
        );
    }
}

#[test]
fn disclosure_matches_every_four_boolean_truth_table_case() {
    struct Case {
        input: WorkSessionDisclosureInput,
        expected: WorkSessionDisclosureOutput,
    }

    // Every combination of (details_defined, has_visible_details, open,
    // working) is represented. These expected values are intentionally
    // explicit so each output remains independently covered rather than being
    // reconstructed by a test-side copy of the implementation.
    let cases = [
        Case {
            input: WorkSessionDisclosureInput {
                details_defined: false,
                has_visible_details: false,
                open: false,
                working: false,
            },
            expected: WorkSessionDisclosureOutput {
                can_collapse: false,
                data_open: None,
                data_state: None,
                details_hidden: true,
                details_mounted: false,
            },
        },
        Case {
            input: WorkSessionDisclosureInput {
                details_defined: false,
                has_visible_details: false,
                open: false,
                working: true,
            },
            expected: WorkSessionDisclosureOutput {
                can_collapse: false,
                data_open: Some(false),
                data_state: Some("closed"),
                details_hidden: true,
                details_mounted: false,
            },
        },
        Case {
            input: WorkSessionDisclosureInput {
                details_defined: false,
                has_visible_details: false,
                open: true,
                working: false,
            },
            expected: WorkSessionDisclosureOutput {
                can_collapse: false,
                data_open: None,
                data_state: None,
                details_hidden: false,
                details_mounted: false,
            },
        },
        Case {
            input: WorkSessionDisclosureInput {
                details_defined: false,
                has_visible_details: false,
                open: true,
                working: true,
            },
            expected: WorkSessionDisclosureOutput {
                can_collapse: false,
                data_open: Some(true),
                data_state: Some("open"),
                details_hidden: false,
                details_mounted: false,
            },
        },
        Case {
            input: WorkSessionDisclosureInput {
                details_defined: false,
                has_visible_details: true,
                open: false,
                working: false,
            },
            expected: WorkSessionDisclosureOutput {
                can_collapse: true,
                data_open: Some(false),
                data_state: Some("closed"),
                details_hidden: true,
                details_mounted: false,
            },
        },
        Case {
            input: WorkSessionDisclosureInput {
                details_defined: false,
                has_visible_details: true,
                open: false,
                working: true,
            },
            expected: WorkSessionDisclosureOutput {
                can_collapse: true,
                data_open: Some(false),
                data_state: Some("closed"),
                details_hidden: true,
                details_mounted: false,
            },
        },
        Case {
            input: WorkSessionDisclosureInput {
                details_defined: false,
                has_visible_details: true,
                open: true,
                working: false,
            },
            expected: WorkSessionDisclosureOutput {
                can_collapse: true,
                data_open: Some(true),
                data_state: Some("open"),
                details_hidden: false,
                details_mounted: false,
            },
        },
        Case {
            input: WorkSessionDisclosureInput {
                details_defined: false,
                has_visible_details: true,
                open: true,
                working: true,
            },
            expected: WorkSessionDisclosureOutput {
                can_collapse: true,
                data_open: Some(true),
                data_state: Some("open"),
                details_hidden: false,
                details_mounted: false,
            },
        },
        Case {
            input: WorkSessionDisclosureInput {
                details_defined: true,
                has_visible_details: false,
                open: false,
                working: false,
            },
            expected: WorkSessionDisclosureOutput {
                can_collapse: false,
                data_open: None,
                data_state: None,
                details_hidden: true,
                details_mounted: false,
            },
        },
        Case {
            input: WorkSessionDisclosureInput {
                details_defined: true,
                has_visible_details: false,
                open: false,
                working: true,
            },
            expected: WorkSessionDisclosureOutput {
                can_collapse: false,
                data_open: Some(false),
                data_state: Some("closed"),
                details_hidden: true,
                details_mounted: true,
            },
        },
        Case {
            input: WorkSessionDisclosureInput {
                details_defined: true,
                has_visible_details: false,
                open: true,
                working: false,
            },
            expected: WorkSessionDisclosureOutput {
                can_collapse: false,
                data_open: None,
                data_state: None,
                details_hidden: false,
                details_mounted: true,
            },
        },
        Case {
            input: WorkSessionDisclosureInput {
                details_defined: true,
                has_visible_details: false,
                open: true,
                working: true,
            },
            expected: WorkSessionDisclosureOutput {
                can_collapse: false,
                data_open: Some(true),
                data_state: Some("open"),
                details_hidden: false,
                details_mounted: true,
            },
        },
        Case {
            input: WorkSessionDisclosureInput {
                details_defined: true,
                has_visible_details: true,
                open: false,
                working: false,
            },
            expected: WorkSessionDisclosureOutput {
                can_collapse: true,
                data_open: Some(false),
                data_state: Some("closed"),
                details_hidden: true,
                details_mounted: false,
            },
        },
        Case {
            input: WorkSessionDisclosureInput {
                details_defined: true,
                has_visible_details: true,
                open: false,
                working: true,
            },
            expected: WorkSessionDisclosureOutput {
                can_collapse: true,
                data_open: Some(false),
                data_state: Some("closed"),
                details_hidden: true,
                details_mounted: true,
            },
        },
        Case {
            input: WorkSessionDisclosureInput {
                details_defined: true,
                has_visible_details: true,
                open: true,
                working: false,
            },
            expected: WorkSessionDisclosureOutput {
                can_collapse: true,
                data_open: Some(true),
                data_state: Some("open"),
                details_hidden: false,
                details_mounted: true,
            },
        },
        Case {
            input: WorkSessionDisclosureInput {
                details_defined: true,
                has_visible_details: true,
                open: true,
                working: true,
            },
            expected: WorkSessionDisclosureOutput {
                can_collapse: true,
                data_open: Some(true),
                data_state: Some("open"),
                details_hidden: false,
                details_mounted: true,
            },
        },
    ];

    assert_eq!(cases.len(), 16);
    for case in cases {
        assert_eq!(
            work_session_disclosure(case.input),
            case.expected,
            "disclosure mismatch for {:?}",
            case.input
        );
    }
}
