//! Dependency-free attachment-tray projection and command policy.
//!
//! This is the native value boundary for
//! `routes/components/composer/attachment-tray.svelte`. The legacy component
//! receives an ordered `ReadonlyMap` view, renders each image by its stable
//! ID, and derives its open state from whether that view is empty. This module
//! keeps those facts in ordinary owned Rust values without touching image
//! bytes, browser object URLs, a renderer, or a clock.

/// The ordered attachment facts needed by the tray.
///
/// `preview_reference` is deliberately opaque. It corresponds to the
/// component's `preview_url`, but this policy never treats it as a URL and
/// never loads, decodes, or revokes the referenced asset.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct AttachmentFact {
    /// Stable identity used as the rendered row key and view/remove target.
    pub id: String,
    /// Original file name, retained verbatim for image alt text and labels.
    pub name: String,
    /// Opaque preview reference retained verbatim for the image renderer.
    pub preview_reference: String,
}

impl AttachmentFact {
    /// Creates one attachment fact without inspecting or normalizing it.
    #[must_use]
    pub fn new(
        id: impl Into<String>,
        name: impl Into<String>,
        preview_reference: impl Into<String>,
    ) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            preview_reference: preview_reference.into(),
        }
    }

    /// Returns the preview reference under the source component's field name.
    ///
    /// The alias is descriptive only: the returned value is not loaded or
    /// otherwise interpreted by this policy.
    #[must_use]
    pub fn preview_url(&self) -> &str {
        &self.preview_reference
    }
}

/// One command exposed by an attachment-tray row.
///
/// Viewing carries the complete attachment fact because the source callback
/// receives the attachment itself. Removing carries only the stable ID,
/// matching the source callback's `onremove(attachment.id)` contract.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub enum AttachmentTrayCommand {
    /// View the exact attachment represented by the row.
    View {
        /// The attachment to pass to the image-viewer boundary.
        attachment: AttachmentFact,
    },
    /// Remove exactly one attachment by stable ID.
    Remove {
        /// The stable ID supplied to the remove boundary.
        attachment_id: String,
    },
}

impl AttachmentTrayCommand {
    /// Creates the source-equivalent view-by-attachment command.
    #[must_use]
    pub fn view_by_attachment(attachment: AttachmentFact) -> Self {
        Self::View { attachment }
    }

    /// Creates the source-equivalent remove-by-ID command.
    #[must_use]
    pub fn remove_by_id(attachment_id: impl Into<String>) -> Self {
        Self::Remove {
            attachment_id: attachment_id.into(),
        }
    }
}

/// An ordered row in the attachment tray.
///
/// The row owns the identity, name, and opaque preview reference needed by a
/// renderer. `name` is also the image's alt text; [`Self::alt_text`] exposes
/// that relationship without creating a second mutable copy of the name.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct AttachmentTrayRow {
    /// Stable identity used to retain row identity across projections.
    pub id: String,
    /// Original file name, also used as image alt text.
    pub name: String,
    /// Opaque preview reference; no asset operation is performed here.
    pub preview_reference: String,
    /// Exact accessible label for the view button.
    pub view_accessible_label: String,
    /// Exact accessible label for the remove button.
    pub remove_accessible_label: String,
}

impl AttachmentTrayRow {
    /// Builds a row from one attachment fact while retaining every source
    /// value byte-for-byte.
    #[must_use]
    pub fn from_attachment(attachment: &AttachmentFact) -> Self {
        Self {
            id: attachment.id.clone(),
            name: attachment.name.clone(),
            preview_reference: attachment.preview_reference.clone(),
            view_accessible_label: accessible_label("View", &attachment.name),
            remove_accessible_label: accessible_label("Remove", &attachment.name),
        }
    }

    /// Returns the stable row identity.
    #[must_use]
    pub fn attachment_id(&self) -> &str {
        &self.id
    }

    /// Returns the exact file name used by the row.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Returns the exact image alt text, which is the original file name.
    #[must_use]
    pub fn alt_text(&self) -> &str {
        &self.name
    }

    /// Returns the opaque preview reference without touching its asset.
    #[must_use]
    pub fn preview_reference(&self) -> &str {
        &self.preview_reference
    }

    /// Returns the preview reference under the source component's field name.
    #[must_use]
    pub fn preview_url(&self) -> &str {
        &self.preview_reference
    }

    /// Returns the exact `View <name>` accessible label.
    #[must_use]
    pub fn view_label(&self) -> &str {
        &self.view_accessible_label
    }

    /// Returns the exact `Remove <name>` accessible label.
    #[must_use]
    pub fn remove_label(&self) -> &str {
        &self.remove_accessible_label
    }

    /// Returns a command carrying this row's complete attachment fact.
    #[must_use]
    pub fn view_command(&self) -> AttachmentTrayCommand {
        AttachmentTrayCommand::view_by_attachment(AttachmentFact::new(
            self.id.clone(),
            self.name.clone(),
            self.preview_reference.clone(),
        ))
    }

    /// Returns a command carrying only this row's stable ID.
    #[must_use]
    pub fn remove_command(&self) -> AttachmentTrayCommand {
        AttachmentTrayCommand::remove_by_id(self.id.clone())
    }
}

/// A value that changes between the closed and open presentation endpoints.
///
/// This is a presentation description, not an animation driver: it stores no
/// duration, easing callback, timer, or renderer state.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct PresentationTransition<T> {
    /// The value used while the tray is closed.
    pub closed: T,
    /// The value used while the tray is open.
    pub open: T,
}

/// The two grid-row tracks used for the tray's height transition.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum GridRowTrack {
    /// The collapsed `0fr` track.
    ZeroFraction,
    /// The expanded `1fr` track.
    OneFraction,
}

impl GridRowTrack {
    /// Returns the exact serialized grid-track fact from the source classes.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ZeroFraction => "0fr",
            Self::OneFraction => "1fr",
        }
    }
}

/// Padding values expressed in the source utility spacing steps.
///
/// These are not measured pixels. The closed endpoint is `p-0`; the open
/// endpoint is `px-1 pt-1 pb-2`.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct PaddingFacts {
    /// Left padding spacing step.
    pub left: u8,
    /// Right padding spacing step.
    pub right: u8,
    /// Top padding spacing step.
    pub top: u8,
    /// Bottom padding spacing step.
    pub bottom: u8,
}

/// Per-row transform and opacity values from the source classes.
///
/// `translate_y` is a utility spacing step, `scale_percent` is a percentage,
/// and `opacity_percent` is the source's 0–100 opacity scale.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct RowTransformFacts {
    /// Vertical translation in utility spacing steps.
    pub translate_y: i8,
    /// Scale as a percentage.
    pub scale_percent: u8,
    /// Opacity as a percentage.
    pub opacity_percent: u8,
}

/// Fixed transition intent for the tray, its padding, and every row.
///
/// The source uses the same composer resize duration/easing for all three
/// layers. Those timing values remain renderer-owned and are intentionally
/// not represented here; these facts capture only the stable endpoints.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct AttachmentTrayTransitionFacts {
    /// Closed/open grid-row tracks for tray height.
    pub tray_height: PresentationTransition<GridRowTrack>,
    /// Closed/open tray opacity percentages.
    pub tray_opacity: PresentationTransition<u8>,
    /// Closed/open inner-content padding.
    pub padding: PresentationTransition<PaddingFacts>,
    /// Closed/open transform and opacity for each attachment row.
    pub row: PresentationTransition<RowTransformFacts>,
}

/// The exact fixed endpoints expressed by `attachment-tray.svelte`.
pub const ATTACHMENT_TRAY_TRANSITION_FACTS: AttachmentTrayTransitionFacts =
    AttachmentTrayTransitionFacts {
        tray_height: PresentationTransition {
            closed: GridRowTrack::ZeroFraction,
            open: GridRowTrack::OneFraction,
        },
        tray_opacity: PresentationTransition {
            closed: 0,
            open: 100,
        },
        padding: PresentationTransition {
            closed: PaddingFacts {
                left: 0,
                right: 0,
                top: 0,
                bottom: 0,
            },
            open: PaddingFacts {
                left: 1,
                right: 1,
                top: 1,
                bottom: 2,
            },
        },
        row: PresentationTransition {
            closed: RowTransformFacts {
                translate_y: 2,
                scale_percent: 96,
                opacity_percent: 0,
            },
            open: RowTransformFacts {
                translate_y: 0,
                scale_percent: 100,
                opacity_percent: 100,
            },
        },
    };

/// Whether the tray has no attachments or at least one attachment.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum AttachmentTrayState {
    /// No rows exist, so the tray is closed/inactive.
    Closed,
    /// At least one row exists, so the tray is open/active.
    Open,
}

/// Ordered attachment rows and their fixed presentation policy.
///
/// There is intentionally no independent `open` or `active` field. Both
/// states are derived from [`Self::rows`], so removing the final attachment
/// closes the tray and adding the first opens it automatically.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct AttachmentTrayProjection {
    /// Rows in the exact order supplied by the attachment projection.
    pub rows: Vec<AttachmentTrayRow>,
}

impl AttachmentTrayProjection {
    /// Projects ordered attachment facts into ordered tray rows.
    #[must_use]
    pub fn new(attachments: &[AttachmentFact]) -> Self {
        Self {
            rows: attachments
                .iter()
                .map(AttachmentTrayRow::from_attachment)
                .collect(),
        }
    }

    /// Returns the projected rows without changing their order.
    #[must_use]
    pub fn rows(&self) -> &[AttachmentTrayRow] {
        &self.rows
    }

    /// Returns whether at least one attachment makes the tray active.
    #[must_use]
    pub fn is_active(&self) -> bool {
        !self.rows.is_empty()
    }

    /// Returns whether the tray is open, derived solely from nonempty rows.
    #[must_use]
    pub fn is_open(&self) -> bool {
        self.is_active()
    }

    /// Returns the open/closed state derived solely from nonempty rows.
    #[must_use]
    pub fn state(&self) -> AttachmentTrayState {
        if self.is_open() {
            AttachmentTrayState::Open
        } else {
            AttachmentTrayState::Closed
        }
    }
}

/// Projects an ordered attachment slice into the native tray policy.
#[must_use]
pub fn project_attachment_tray(attachments: &[AttachmentFact]) -> AttachmentTrayProjection {
    AttachmentTrayProjection::new(attachments)
}

/// Returns the tray state for an ordered attachment slice.
#[must_use]
pub fn attachment_tray_state(attachments: &[AttachmentFact]) -> AttachmentTrayState {
    project_attachment_tray(attachments).state()
}

fn accessible_label(action: &str, name: &str) -> String {
    format!("{action} {name}")
}
