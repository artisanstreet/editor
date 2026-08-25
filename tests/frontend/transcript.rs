//! Behavioral coverage for the native transcript/message presentation model
//! (`docs/ui/INVENTORY.md` §6.5): exact audited per-role labels, verbatim
//! owned message content, and the pending-steer status fact.

use artisan_frontend::transcript::{
    AnnouncementRole, MessageRole, PendingSteer, TranscriptMessage,
};

const ALL_ROLES: [MessageRole; 3] = [
    MessageRole::User,
    MessageRole::Assistant,
    MessageRole::Reasoning,
];

#[test]
fn every_message_role_maps_to_its_exact_audited_label() {
    assert_eq!(MessageRole::User.label(), "Your message");
    assert_eq!(MessageRole::Assistant.label(), "Assistant message");
    assert_eq!(MessageRole::Reasoning.label(), "Reasoning summary");
}

#[test]
fn message_roles_are_mutually_distinct() {
    // Match without a wildcard arm: adding a role breaks this test until the
    // new label is audited here.
    for role in ALL_ROLES {
        let label = match role {
            MessageRole::User | MessageRole::Assistant | MessageRole::Reasoning => role.label(),
        };
        assert!(!label.is_empty());
    }

    for (index, first) in ALL_ROLES.iter().enumerate() {
        for second in &ALL_ROLES[index + 1..] {
            assert_ne!(first, second);
            assert_ne!(first.label(), second.label());
        }
    }
}

#[test]
fn message_content_is_preserved_verbatim() {
    let cases: [(&str, &str); 4] = [
        ("empty", ""),
        (
            "multiline",
            "first line\nsecond line\n\nfourth line\r\ntrailing",
        ),
        ("unicode", "Grüße — 日本語 🦀 שלום\né\u{301}…\ttab"),
        (
            "markdown-looking",
            "# Heading\n\n- **bold** _item_\n\n```rust\nlet x = 1;\n```\n",
        ),
    ];

    for (name, text) in cases {
        let message = TranscriptMessage::new(MessageRole::User, text);
        assert_eq!(message.content(), text, "{name} must round-trip");
        assert_eq!(
            message.content().as_bytes(),
            text.as_bytes(),
            "{name} must be preserved byte-for-byte"
        );
        assert_eq!(message.role(), MessageRole::User);
    }
}

#[test]
fn content_cannot_inject_or_mutate_the_audited_label() {
    // Content that exactly equals another role's audited label stays content.
    let injected = TranscriptMessage::new(MessageRole::Assistant, "Your message");
    assert_eq!(injected.label(), "Assistant message");
    assert_eq!(injected.content(), "Your message");

    let all_labels = format!(
        "{}\n{}\n{}",
        MessageRole::User.label(),
        MessageRole::Assistant.label(),
        MessageRole::Reasoning.label(),
    );
    let reasoning = TranscriptMessage::new(MessageRole::Reasoning, all_labels);
    assert_eq!(reasoning.label(), "Reasoning summary");

    // The same role always yields the same label regardless of content.
    let empty = TranscriptMessage::new(MessageRole::User, "");
    let self_named = TranscriptMessage::new(MessageRole::User, MessageRole::User.label());
    assert_ne!(empty.content(), self_named.content());
    assert_eq!(empty.label(), self_named.label());
}

#[test]
fn pending_steer_carries_the_exact_inventory_wording() {
    assert_eq!(PendingSteer::LABEL, "Steering");
    assert_eq!(PendingSteer::ANNOUNCEMENT_ROLE, AnnouncementRole::Status);
    assert_eq!(PendingSteer::ANNOUNCEMENT_ROLE.attribute_value(), "status");
}

#[test]
fn pending_steer_is_semantically_distinct_from_every_message_role() {
    for role in ALL_ROLES {
        assert_ne!(PendingSteer::LABEL, role.label());
    }
}
