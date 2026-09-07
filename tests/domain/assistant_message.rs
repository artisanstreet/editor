//! Assistant message vocabulary coverage: run identity, bounded bodies,
//! phases independent from lifecycles, and mixed-kind snapshot validation.

use artisan_domain::{
    AssistantBody, AssistantBodyError, AssistantMessageItem, AssistantMessagePhase,
    ConversationCursor, ConversationItem, ConversationLifecycle, ConversationSnapshot,
    ConversationSnapshotError, ConversationTurn, IDENTIFIER_MAX_BYTES, IdentifierError, ItemId,
    ItemOrdinal, MESSAGE_BODY_MAX_BYTES, MessageBody, Revision, RunId, ThreadId, TurnId,
    TurnOrdinal, UnixMillis, UserMessageItem,
};

fn thread_id() -> ThreadId {
    ThreadId::parse("thread-assistant-1").expect("fixture thread id is valid")
}

fn turn(id: &str, ordinal: u64) -> ConversationTurn {
    ConversationTurn {
        turn_id: TurnId::parse(id).expect("fixture turn id is valid"),
        ordinal: TurnOrdinal::new(ordinal),
        revision: Revision::default(),
        lifecycle: ConversationLifecycle::Pending,
        created_at: UnixMillis::from_millis(-10),
        updated_at: UnixMillis::from_millis(20),
    }
}

fn user_item(id: &str, turn: &str, ordinal: u64) -> ConversationItem {
    ConversationItem::UserMessage(UserMessageItem {
        item_id: ItemId::parse(id).expect("fixture item id is valid"),
        turn_id: TurnId::parse(turn).expect("fixture turn id is valid"),
        ordinal: ItemOrdinal::new(ordinal),
        revision: Revision::default(),
        lifecycle: ConversationLifecycle::Completed,
        body: MessageBody::parse("Queued question").expect("fixture body is valid"),
        created_at: UnixMillis::from_millis(-5),
        updated_at: UnixMillis::from_millis(25),
    })
}

fn assistant_item(id: &str, turn: &str, ordinal: u64) -> ConversationItem {
    ConversationItem::AssistantMessage(AssistantMessageItem {
        item_id: ItemId::parse(id).expect("fixture item id is valid"),
        turn_id: TurnId::parse(turn).expect("fixture turn id is valid"),
        run_id: RunId::parse("run-assistant-1").expect("fixture run id is valid"),
        ordinal: ItemOrdinal::new(ordinal),
        revision: Revision::default(),
        lifecycle: ConversationLifecycle::Streaming,
        body: AssistantBody::parse("partial answer").expect("fixture body is valid"),
        phase: AssistantMessagePhase::Commentary,
        created_at: UnixMillis::from_millis(-5),
        updated_at: UnixMillis::from_millis(25),
    })
}

#[test]
fn run_id_follows_the_shared_wire_identifier_rule() {
    let parsed = RunId::parse("run-2718281828").expect("the fixture is valid");
    assert_eq!(parsed.as_str(), "run-2718281828");
    assert_eq!(parsed.to_string(), "run-2718281828");
    assert_eq!(
        "run-from-str"
            .parse::<RunId>()
            .expect("FromStr shares parse"),
        RunId::parse("run-from-str").expect("the fixture is valid")
    );

    assert_eq!(RunId::parse(""), Err(IdentifierError::Empty));
    assert_eq!(
        RunId::parse("run has a space"),
        Err(IdentifierError::ForbiddenCharacter { character: ' ' })
    );
    assert_eq!(
        RunId::parse("run\nwith-control"),
        Err(IdentifierError::ForbiddenCharacter { character: '\n' })
    );
    assert_eq!(
        RunId::parse("x".repeat(IDENTIFIER_MAX_BYTES + 1)),
        Err(IdentifierError::TooLong {
            length: IDENTIFIER_MAX_BYTES + 1,
            maximum: IDENTIFIER_MAX_BYTES,
        })
    );
}

#[test]
fn assistant_body_permits_empty_whitespace_and_unicode_and_preserves_text() {
    assert_eq!(
        AssistantBody::parse("")
            .expect("an empty opening assistant body is valid")
            .as_str(),
        ""
    );
    let blank = "  \n\t ";
    assert_eq!(
        AssistantBody::parse(blank)
            .expect("whitespace-only text stays valid for assistant output")
            .as_str(),
        blank
    );
    let padded = "  padded reply  ";
    assert_eq!(
        AssistantBody::parse(padded)
            .expect("the fixture is valid")
            .as_str(),
        padded
    );
    let unicode = "réponse 🦀 — kept verbatim";
    assert_eq!(
        AssistantBody::parse(unicode)
            .expect("the fixture is valid")
            .as_str(),
        unicode
    );
}

#[test]
fn assistant_body_enforces_the_shared_body_ceiling_in_utf8_bytes() {
    AssistantBody::parse("x".repeat(MESSAGE_BODY_MAX_BYTES))
        .expect("65,536 ASCII bytes fit the shared body ceiling");
    AssistantBody::parse("é".repeat(MESSAGE_BODY_MAX_BYTES / 2))
        .expect("32,768 two-byte characters fit the shared body ceiling");

    assert_eq!(
        AssistantBody::parse("x".repeat(MESSAGE_BODY_MAX_BYTES + 1)),
        Err(AssistantBodyError::TooLong {
            length: MESSAGE_BODY_MAX_BYTES + 1,
            maximum: MESSAGE_BODY_MAX_BYTES,
        })
    );
    // Byte units, not character counts: one extra two-byte character lands
    // two bytes over the ceiling.
    assert_eq!(
        AssistantBody::parse("é".repeat((MESSAGE_BODY_MAX_BYTES / 2) + 1)),
        Err(AssistantBodyError::TooLong {
            length: MESSAGE_BODY_MAX_BYTES + 2,
            maximum: MESSAGE_BODY_MAX_BYTES,
        })
    );
}

#[test]
fn phases_classify_renderer_text_without_touching_lifecycle() {
    let settled_but_open = AssistantMessageItem {
        item_id: ItemId::parse("item-final-open").expect("fixture item id is valid"),
        turn_id: TurnId::parse("turn-final-open").expect("fixture turn id is valid"),
        run_id: RunId::parse("run-final-open").expect("fixture run id is valid"),
        ordinal: ItemOrdinal::new(0),
        revision: Revision::default(),
        lifecycle: ConversationLifecycle::Pending,
        body: AssistantBody::parse("settled text").expect("fixture body is valid"),
        phase: AssistantMessagePhase::Final,
        created_at: UnixMillis::from_millis(-1),
        updated_at: UnixMillis::from_millis(1),
    };
    assert_eq!(settled_but_open.phase, AssistantMessagePhase::Final);
    assert_eq!(settled_but_open.lifecycle, ConversationLifecycle::Pending);
    // Final text does not imply a completed item: the lifecycle keeps its
    // own transition rules.
    assert!(
        settled_but_open
            .lifecycle
            .validate_transition(ConversationLifecycle::Completed)
            .is_ok()
    );

    let commentary_done = AssistantMessageItem {
        lifecycle: ConversationLifecycle::Completed,
        phase: AssistantMessagePhase::Commentary,
        ..settled_but_open.clone()
    };
    assert_eq!(commentary_done.phase, AssistantMessagePhase::Commentary);
    assert_eq!(commentary_done.lifecycle, ConversationLifecycle::Completed);

    let unspecified_failed = AssistantMessageItem {
        lifecycle: ConversationLifecycle::Failed,
        phase: AssistantMessagePhase::Unspecified,
        ..settled_but_open.clone()
    };
    assert_eq!(unspecified_failed.phase, AssistantMessagePhase::Unspecified);
    assert_eq!(unspecified_failed.lifecycle, ConversationLifecycle::Failed);

    // Every phase remains constructible with every lifecycle state.
    for lifecycle in [
        ConversationLifecycle::Pending,
        ConversationLifecycle::Streaming,
        ConversationLifecycle::Active,
        ConversationLifecycle::Waiting,
        ConversationLifecycle::Completed,
        ConversationLifecycle::Failed,
        ConversationLifecycle::Interrupted,
        ConversationLifecycle::Cancelled,
    ] {
        for phase in [
            AssistantMessagePhase::Unspecified,
            AssistantMessagePhase::Commentary,
            AssistantMessagePhase::Final,
        ] {
            let item = AssistantMessageItem {
                lifecycle,
                phase,
                ..settled_but_open.clone()
            };
            assert_eq!(item.phase, phase);
            assert_eq!(item.lifecycle, lifecycle);
        }
    }
}

#[test]
fn mixed_snapshots_sort_items_across_kinds_and_keep_accessors_total() {
    let user = user_item("item-user-mixed", "turn-mixed", 1);
    let assistant = assistant_item("item-assist-mixed", "turn-mixed", 2);
    let snapshot = ConversationSnapshot::new(
        thread_id(),
        ConversationCursor::new(3),
        vec![turn("turn-mixed", 0)],
        // Deliberately out of ordinal order: sorting is kind-independent.
        vec![assistant.clone(), user.clone()],
        UnixMillis::from_millis(30),
    )
    .expect("a mixed snapshot over one known turn is valid");

    let items = snapshot.items();
    assert_eq!(items.len(), 2);
    assert_eq!(
        items.first().map(ConversationItem::item_id),
        Some(user.item_id())
    );
    assert_eq!(
        items.get(1).map(ConversationItem::item_id),
        Some(assistant.item_id())
    );
    // Total accessors answer identically for both kinds.
    assert_eq!(
        items.first().map(ConversationItem::ordinal),
        Some(ItemOrdinal::new(1))
    );
    assert_eq!(
        items.get(1).map(ConversationItem::ordinal),
        Some(ItemOrdinal::new(2))
    );
    assert_eq!(
        items.get(1).map(ConversationItem::turn_id),
        Some(&TurnId::parse("turn-mixed").expect("fixture turn id is valid"))
    );

    let ConversationItem::AssistantMessage(decoded) = &items[1] else {
        panic!("the higher ordinal must stay the assistant item");
    };
    assert_eq!(decoded.run_id.as_str(), "run-assistant-1");
    assert_eq!(decoded.phase, AssistantMessagePhase::Commentary);
}

#[test]
fn cross_kind_identity_and_ordinal_duplicates_are_rejected_uniformly() {
    let duplicate_id = ConversationSnapshot::new(
        thread_id(),
        ConversationCursor::default(),
        vec![turn("turn-cross", 0)],
        vec![
            user_item("item-shared", "turn-cross", 1),
            assistant_item("item-shared", "turn-cross", 2),
        ],
        UnixMillis::from_millis(30),
    );
    assert_eq!(
        duplicate_id,
        Err(ConversationSnapshotError::DuplicateItemId {
            item_id: ItemId::parse("item-shared").expect("fixture item id is valid"),
        })
    );

    let duplicate_ordinal = ConversationSnapshot::new(
        thread_id(),
        ConversationCursor::default(),
        vec![turn("turn-cross", 0)],
        vec![
            user_item("item-user-cross", "turn-cross", 3),
            assistant_item("item-assist-cross", "turn-cross", 3),
        ],
        UnixMillis::from_millis(30),
    );
    assert_eq!(
        duplicate_ordinal,
        Err(ConversationSnapshotError::DuplicateOrdinal { ordinal: 3 })
    );

    let missing_turn = ConversationSnapshot::new(
        thread_id(),
        ConversationCursor::default(),
        vec![turn("turn-present", 0)],
        vec![assistant_item("item-orphan", "turn-absent", 1)],
        UnixMillis::from_millis(30),
    );
    assert_eq!(
        missing_turn,
        Err(ConversationSnapshotError::UnknownTurn {
            item_id: ItemId::parse("item-orphan").expect("fixture item id is valid"),
            turn_id: TurnId::parse("turn-absent").expect("fixture turn id is valid"),
        })
    );
}
