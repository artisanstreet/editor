//! Exhaustive parity coverage for the dependency-free component-gallery policy.
//!
//! The production module is included directly so this focused harness can be
//! compiled with pinned Rust 1.98 without Cargo, Bazel, or frontend
//! registration changes.

#![forbid(unsafe_code)]

#[path = "../../modules/frontend/src/component_gallery_policy.rs"]
mod component_gallery_policy;

use component_gallery_policy::{
    COMPONENT_GALLERY_ENTRIES, COMPONENT_GALLERY_ENTRY_COUNT, ComponentGalleryDirection,
    component_gallery_entries, component_gallery_index_for, component_gallery_neighbor,
};

struct ExpectedEntry {
    description: &'static str,
    group: &'static str,
    id: &'static str,
    label: &'static str,
}

const EXPECTED_ENTRIES: [ExpectedEntry; COMPONENT_GALLERY_ENTRY_COUNT] = [
    ExpectedEntry {
        description: "A dense, realistic conversation inside the production thread workspace.",
        group: "Thread",
        id: "full-thread",
        label: "Full thread",
    },
    ExpectedEntry {
        description: "The authored prompt card aligned to the conversation’s right edge.",
        group: "Messages",
        id: "user-message",
        label: "User message",
    },
    ExpectedEntry {
        description: "A user prompt with resolved image thumbnails and the real image viewer interaction.",
        group: "Messages",
        id: "image-message",
        label: "Message with images",
    },
    ExpectedEntry {
        description: "A settled assistant response using the production Markdown renderer.",
        group: "Messages",
        id: "assistant-message",
        label: "Assistant response",
    },
    ExpectedEntry {
        description: "An assistant response while tokens are still arriving.",
        group: "Messages",
        id: "streaming-message",
        label: "Streaming response",
    },
    ExpectedEntry {
        description: "Provider-visible reasoning with the active shimmer treatment.",
        group: "Messages",
        id: "reasoning-summary",
        label: "Reasoning summary",
    },
    ExpectedEntry {
        description: "A live turn waiting on its provider, including elapsed-time and disclosure behavior.",
        group: "Work",
        id: "active-work",
        label: "Active work session",
    },
    ExpectedEntry {
        description: "A live turn's newest thinking paragraph, whole, replaced as the next one opens.",
        group: "Work",
        id: "thinking-summary",
        label: "Thinking summary",
    },
    ExpectedEntry {
        description: "A naturally completed turn rendered as settled history.",
        group: "Work",
        id: "completed-work",
        label: "Completed work session",
    },
    ExpectedEntry {
        description: "A failed provider attempt with its diagnostic trace available for inspection.",
        group: "Work",
        id: "failed-work",
        label: "Failed work session",
    },
    ExpectedEntry {
        description: "One provider activity row before it is grouped into a longer trace.",
        group: "Work",
        id: "activity-row",
        label: "Activity row",
    },
    ExpectedEntry {
        description: "A mixed tool chain with commands, search, reasoning, and an active operation.",
        group: "Work",
        id: "activity-trace",
        label: "Activity trace",
    },
    ExpectedEntry {
        description: "The aggregate changed-files card with paths and diff counts.",
        group: "Work",
        id: "edited-files",
        label: "Edited files",
    },
    ExpectedEntry {
        description: "A command permission request with its exact command and working directory.",
        group: "Requests",
        id: "command-approval",
        label: "Command approval",
    },
    ExpectedEntry {
        description: "A provider question waiting for a short user answer.",
        group: "Requests",
        id: "question",
        label: "Question",
    },
    ExpectedEntry {
        description: "A model-specific usage interruption with reset countdown and verified alternative.",
        group: "Recovery",
        id: "usage-limit",
        label: "Usage limit",
    },
    ExpectedEntry {
        description: "The compact historical state after a usage interruption has continued.",
        group: "Recovery",
        id: "usage-continued",
        label: "Usage continued",
    },
    ExpectedEntry {
        description: "A catalog-backed provider failure with code, explanation, and reset evidence.",
        group: "Recovery",
        id: "provider-error",
        label: "Provider error",
    },
    ExpectedEntry {
        description: "The chapter divider while context compaction is in progress.",
        group: "Boundaries",
        id: "compacting",
        label: "Compacting",
    },
    ExpectedEntry {
        description: "The same chapter divider after compaction has settled.",
        group: "Boundaries",
        id: "compacted",
        label: "Compacted",
    },
    ExpectedEntry {
        description: "A native continuation handing the thread from one model to another.",
        group: "Boundaries",
        id: "model-handoff",
        label: "Model handoff",
    },
    ExpectedEntry {
        description: "The composer’s context-window control and its interactive usage detail.",
        group: "Controls",
        id: "context-window",
        label: "Context window",
    },
    ExpectedEntry {
        description: "The response hover footer with copy action and relative settlement time.",
        group: "Controls",
        id: "turn-actions",
        label: "Turn actions",
    },
];

#[test]
fn canonical_catalog_matches_every_source_field_in_order() {
    assert_eq!(COMPONENT_GALLERY_ENTRY_COUNT, 23);
    assert_eq!(COMPONENT_GALLERY_ENTRIES.len(), EXPECTED_ENTRIES.len());
    assert!(std::ptr::eq(
        component_gallery_entries().as_ptr(),
        COMPONENT_GALLERY_ENTRIES.as_ptr()
    ));

    for (index, expected) in EXPECTED_ENTRIES.iter().enumerate() {
        let actual = &COMPONENT_GALLERY_ENTRIES[index];
        assert_eq!(
            actual.description, expected.description,
            "description at {index}"
        );
        assert_eq!(actual.group, expected.group, "group at {index}");
        assert_eq!(actual.id, expected.id, "id at {index}");
        assert_eq!(actual.label, expected.label, "label at {index}");
    }
}

#[test]
fn catalog_ids_are_unique() {
    for (index, entry) in COMPONENT_GALLERY_ENTRIES.iter().enumerate() {
        for previous in COMPONENT_GALLERY_ENTRIES.iter().take(index) {
            assert_ne!(
                entry.id, previous.id,
                "duplicate component-gallery id {:?}",
                entry.id
            );
        }
    }
}

#[test]
fn exact_lookup_returns_each_entry_index() {
    for (index, expected) in EXPECTED_ENTRIES.iter().enumerate() {
        assert_eq!(
            component_gallery_index_for(Some(expected.id)),
            index,
            "lookup for {:?}",
            expected.id
        );
    }
}

#[test]
fn absent_unknown_empty_and_case_mismatched_ids_select_the_first_entry() {
    let cases = [
        (None, "absent"),
        (Some(""), "empty"),
        (Some("does-not-exist"), "unknown"),
        (Some("FULL-THREAD"), "case mismatch"),
        (Some("full-thread "), "whitespace mismatch"),
    ];

    for (requested_id, description) in cases {
        assert_eq!(
            component_gallery_index_for(requested_id),
            0,
            "{description} requested ID must select the first entry"
        );
    }
}

#[test]
fn previous_and_next_select_the_expected_neighbor_for_every_index() {
    let total = EXPECTED_ENTRIES.len();

    for index in 0..total {
        let previous_index = if index == 0 { total - 1 } else { index - 1 };
        let next_index = (index + 1) % total;

        assert!(std::ptr::eq(
            component_gallery_neighbor(index, ComponentGalleryDirection::Previous),
            std::ptr::from_ref(&COMPONENT_GALLERY_ENTRIES[previous_index])
        ));
        assert!(std::ptr::eq(
            component_gallery_neighbor(index, ComponentGalleryDirection::Next),
            std::ptr::from_ref(&COMPONENT_GALLERY_ENTRIES[next_index])
        ));
    }
}

#[test]
fn both_catalog_boundaries_wrap_to_the_opposite_entry() {
    let last = EXPECTED_ENTRIES.len() - 1;

    assert_eq!(
        component_gallery_neighbor(0, ComponentGalleryDirection::Previous).id,
        EXPECTED_ENTRIES[last].id
    );
    assert_eq!(
        component_gallery_neighbor(last, ComponentGalleryDirection::Next).id,
        EXPECTED_ENTRIES[0].id
    );
}

#[test]
fn arbitrary_large_indices_use_modulo_without_overflow() {
    let total = EXPECTED_ENTRIES.len();
    let indices = [0, total, total * 2, usize::MAX - 1, usize::MAX];

    for index in indices {
        let normalized = index % total;
        let previous_index = if normalized == 0 {
            total - 1
        } else {
            normalized - 1
        };
        let next_index = (normalized + 1) % total;

        assert_eq!(
            component_gallery_neighbor(index, ComponentGalleryDirection::Previous).id,
            EXPECTED_ENTRIES[previous_index].id,
            "previous neighbor for index {index}"
        );
        assert_eq!(
            component_gallery_neighbor(index, ComponentGalleryDirection::Next).id,
            EXPECTED_ENTRIES[next_index].id,
            "next neighbor for index {index}"
        );
    }
}
