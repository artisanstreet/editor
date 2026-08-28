//! Focused, dependency-free coverage for the attachment-tray policy.

#[path = "../../modules/frontend/src/attachment_tray_policy.rs"]
mod attachment_tray_policy;

use attachment_tray_policy::{
    ATTACHMENT_TRAY_TRANSITION_FACTS, AttachmentFact, AttachmentTrayCommand,
    AttachmentTrayProjection, AttachmentTrayRow, AttachmentTrayState,
    AttachmentTrayTransitionFacts, GridRowTrack, PaddingFacts, PresentationTransition,
    RowTransformFacts, attachment_tray_state, project_attachment_tray,
};

fn attachment(id: &str, name: &str, preview_reference: &str) -> AttachmentFact {
    AttachmentFact::new(id, name, preview_reference)
}

#[test]
fn empty_projection_is_closed_and_inactive() {
    let projection = project_attachment_tray(&[]);

    assert_eq!(projection, AttachmentTrayProjection { rows: Vec::new() });
    assert!(projection.rows().is_empty());
    assert!(!projection.is_active());
    assert!(!projection.is_open());
    assert_eq!(projection.state(), AttachmentTrayState::Closed);
    assert_eq!(attachment_tray_state(&[]), AttachmentTrayState::Closed);
}

#[test]
fn nonempty_projection_is_open_without_a_separate_open_input() {
    let facts = [attachment("one", "one.png", "opaque:one")];
    let projection = project_attachment_tray(&facts);

    assert!(projection.is_active());
    assert!(projection.is_open());
    assert_eq!(projection.state(), AttachmentTrayState::Open);
    assert_eq!(attachment_tray_state(&facts), AttachmentTrayState::Open);
}

#[test]
fn rows_preserve_order_and_stable_identity() {
    let facts = [
        attachment("first", "first.png", "preview:first"),
        attachment("second", "second.png", "preview:second"),
        attachment("third", "third.png", "preview:third"),
    ];
    let projection = project_attachment_tray(&facts);

    assert_eq!(projection.rows().len(), 3);
    assert_eq!(
        projection
            .rows()
            .iter()
            .map(AttachmentTrayRow::attachment_id)
            .collect::<Vec<_>>(),
        vec!["first", "second", "third"]
    );
    assert_eq!(projection.rows()[1].name(), "second.png");
    assert_eq!(projection.rows()[1].preview_reference(), "preview:second");
}

#[test]
fn rows_expose_full_view_and_id_only_remove_commands() {
    let fact = attachment("stable-id", "screen shot.png", "blob:opaque-preview");
    let row = AttachmentTrayRow::from_attachment(&fact);

    assert_eq!(
        row.view_command(),
        AttachmentTrayCommand::View {
            attachment: fact.clone()
        }
    );
    assert_eq!(
        row.remove_command(),
        AttachmentTrayCommand::Remove {
            attachment_id: String::from("stable-id")
        }
    );
    assert_eq!(row.view_label(), "View screen shot.png");
    assert_eq!(row.remove_label(), "Remove screen shot.png");
}

#[test]
fn labels_and_alt_text_retain_empty_and_unicode_names_exactly() {
    let facts = [
        attachment("empty", "", "preview:empty-name"),
        attachment("unicode", "猫 🚀 — résumé.png", "preview:unicode"),
    ];
    let projection = project_attachment_tray(&facts);
    let empty = &projection.rows()[0];
    let unicode = &projection.rows()[1];

    assert_eq!(empty.alt_text(), "");
    assert_eq!(empty.view_label(), "View ");
    assert_eq!(empty.remove_label(), "Remove ");
    assert_eq!(unicode.name(), "猫 🚀 — résumé.png");
    assert_eq!(unicode.alt_text(), "猫 🚀 — résumé.png");
    assert_eq!(unicode.view_label(), "View 猫 🚀 — résumé.png");
    assert_eq!(unicode.remove_label(), "Remove 猫 🚀 — résumé.png");
}

#[test]
fn preview_reference_and_view_command_keep_opaque_custody() {
    let fact = attachment("image", "original.webp", "not-a-url://原样 🚀");
    let projection = project_attachment_tray(std::slice::from_ref(&fact));
    let row = &projection.rows()[0];

    assert_eq!(fact.preview_url(), "not-a-url://原样 🚀");
    assert_eq!(row.preview_reference(), "not-a-url://原样 🚀");
    assert_eq!(row.preview_url(), "not-a-url://原样 🚀");
    assert_eq!(row.alt_text(), row.name());
    assert_eq!(
        row.view_command(),
        AttachmentTrayCommand::view_by_attachment(fact)
    );
}

#[test]
fn transition_facts_match_all_fixed_source_endpoints() {
    let expected = AttachmentTrayTransitionFacts {
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

    assert_eq!(ATTACHMENT_TRAY_TRANSITION_FACTS, expected);
    assert_eq!(
        ATTACHMENT_TRAY_TRANSITION_FACTS.tray_height.closed.as_str(),
        "0fr"
    );
    assert_eq!(
        ATTACHMENT_TRAY_TRANSITION_FACTS.tray_height.open.as_str(),
        "1fr"
    );
}
