//! Dependency-free catalog and navigation policy for the component gallery.
//!
//! This is the native counterpart of
//! `routes/debug/components/catalog.ts`. The catalog is deliberately static:
//! a later renderer owns the debug route, component previews, and URL
//! handling, while this leaf preserves the curated entries and their
//! deterministic navigation semantics.

#![allow(clippy::module_name_repetitions)]
#![forbid(unsafe_code)]

/// One immutable specimen in the component gallery.
#[must_use = "use the immutable component-gallery entry"]
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct ComponentGalleryEntry {
    /// Exact explanatory text shown below the gallery navigation.
    pub description: &'static str,
    /// Exact presentation group shown above the specimen label.
    pub group: &'static str,
    /// Exact stable identifier used by the gallery deep link.
    pub id: &'static str,
    /// Exact presentation label shown in the gallery navigation.
    pub label: &'static str,
}

/// Number of entries in the immutable component-gallery catalog.
pub const COMPONENT_GALLERY_ENTRY_COUNT: usize = 23;

/// The complete curated component-gallery catalog in source order.
pub static COMPONENT_GALLERY_ENTRIES: [ComponentGalleryEntry; COMPONENT_GALLERY_ENTRY_COUNT] = [
    ComponentGalleryEntry {
        description: "A dense, realistic conversation inside the production thread workspace.",
        group: "Thread",
        id: "full-thread",
        label: "Full thread",
    },
    ComponentGalleryEntry {
        description: "The authored prompt card aligned to the conversation’s right edge.",
        group: "Messages",
        id: "user-message",
        label: "User message",
    },
    ComponentGalleryEntry {
        description: "A user prompt with resolved image thumbnails and the real image viewer interaction.",
        group: "Messages",
        id: "image-message",
        label: "Message with images",
    },
    ComponentGalleryEntry {
        description: "A settled assistant response using the production Markdown renderer.",
        group: "Messages",
        id: "assistant-message",
        label: "Assistant response",
    },
    ComponentGalleryEntry {
        description: "An assistant response while tokens are still arriving.",
        group: "Messages",
        id: "streaming-message",
        label: "Streaming response",
    },
    ComponentGalleryEntry {
        description: "Provider-visible reasoning with the active shimmer treatment.",
        group: "Messages",
        id: "reasoning-summary",
        label: "Reasoning summary",
    },
    ComponentGalleryEntry {
        description: "A live turn waiting on its provider, including elapsed-time and disclosure behavior.",
        group: "Work",
        id: "active-work",
        label: "Active work session",
    },
    ComponentGalleryEntry {
        description: "A live turn's newest thinking paragraph, whole, replaced as the next one opens.",
        group: "Work",
        id: "thinking-summary",
        label: "Thinking summary",
    },
    ComponentGalleryEntry {
        description: "A naturally completed turn rendered as settled history.",
        group: "Work",
        id: "completed-work",
        label: "Completed work session",
    },
    ComponentGalleryEntry {
        description: "A failed provider attempt with its diagnostic trace available for inspection.",
        group: "Work",
        id: "failed-work",
        label: "Failed work session",
    },
    ComponentGalleryEntry {
        description: "One provider activity row before it is grouped into a longer trace.",
        group: "Work",
        id: "activity-row",
        label: "Activity row",
    },
    ComponentGalleryEntry {
        description: "A mixed tool chain with commands, search, reasoning, and an active operation.",
        group: "Work",
        id: "activity-trace",
        label: "Activity trace",
    },
    ComponentGalleryEntry {
        description: "The aggregate changed-files card with paths and diff counts.",
        group: "Work",
        id: "edited-files",
        label: "Edited files",
    },
    ComponentGalleryEntry {
        description: "A command permission request with its exact command and working directory.",
        group: "Requests",
        id: "command-approval",
        label: "Command approval",
    },
    ComponentGalleryEntry {
        description: "A provider question waiting for a short user answer.",
        group: "Requests",
        id: "question",
        label: "Question",
    },
    ComponentGalleryEntry {
        description: "A model-specific usage interruption with reset countdown and verified alternative.",
        group: "Recovery",
        id: "usage-limit",
        label: "Usage limit",
    },
    ComponentGalleryEntry {
        description: "The compact historical state after a usage interruption has continued.",
        group: "Recovery",
        id: "usage-continued",
        label: "Usage continued",
    },
    ComponentGalleryEntry {
        description: "A catalog-backed provider failure with code, explanation, and reset evidence.",
        group: "Recovery",
        id: "provider-error",
        label: "Provider error",
    },
    ComponentGalleryEntry {
        description: "The chapter divider while context compaction is in progress.",
        group: "Boundaries",
        id: "compacting",
        label: "Compacting",
    },
    ComponentGalleryEntry {
        description: "The same chapter divider after compaction has settled.",
        group: "Boundaries",
        id: "compacted",
        label: "Compacted",
    },
    ComponentGalleryEntry {
        description: "A native continuation handing the thread from one model to another.",
        group: "Boundaries",
        id: "model-handoff",
        label: "Model handoff",
    },
    ComponentGalleryEntry {
        description: "The composer’s context-window control and its interactive usage detail.",
        group: "Controls",
        id: "context-window",
        label: "Context window",
    },
    ComponentGalleryEntry {
        description: "The response hover footer with copy action and relative settlement time.",
        group: "Controls",
        id: "turn-actions",
        label: "Turn actions",
    },
];

/// Returns the complete catalog as an immutable static slice.
#[must_use = "use the immutable component-gallery catalog"]
pub const fn component_gallery_entries() -> &'static [ComponentGalleryEntry] {
    &COMPONENT_GALLERY_ENTRIES
}

/// The typed direction of a component-gallery navigation step.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum ComponentGalleryDirection {
    /// Select the preceding catalog entry, wrapping at the first entry.
    Previous,
    /// Select the following catalog entry, wrapping at the last entry.
    Next,
}

/// Returns the exact-match index for an optional gallery ID.
//
// `None`, an empty string, an unknown ID, and any case or whitespace mismatch
// all use the first catalog entry, matching the TypeScript deep-link policy.
#[must_use = "use the selected component-gallery index"]
pub fn component_gallery_index_for(requested_id: Option<&str>) -> usize {
    requested_id
        .and_then(|requested_id| {
            component_gallery_entries()
                .iter()
                .position(|entry| entry.id == requested_id)
        })
        .unwrap_or(0)
}

/// Returns the typed previous or next neighbor for any nonnegative index.
//
// Normalizing before the one-step adjustment makes both directions safe for
// every `usize`, including `usize::MAX`; the catalog is non-empty by the
// fixed [`COMPONENT_GALLERY_ENTRY_COUNT`] contract.
#[must_use = "use the selected component-gallery neighbor"]
pub fn component_gallery_neighbor(
    index: usize,
    direction: ComponentGalleryDirection,
) -> &'static ComponentGalleryEntry {
    let total = COMPONENT_GALLERY_ENTRY_COUNT;
    let normalized_index = index % total;
    let neighbor_index = match direction {
        ComponentGalleryDirection::Previous => {
            if normalized_index == 0 {
                total - 1
            } else {
                normalized_index - 1
            }
        }
        ComponentGalleryDirection::Next => (normalized_index + 1) % total,
    };

    &COMPONENT_GALLERY_ENTRIES[neighbor_index]
}
