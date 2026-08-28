//! Phase 2 conversation replay raw-wire proof.
//!
//! Round-trips every conversation arm appended to `schema/artisan.capnp`
//! through the Bazel-generated bindings (`artisan_protocol::artisan_capnp`):
//! the query, subscribe, and unsubscribe requests with bounded window/range
//! reads and fresh/resume subscriptions; the snapshot, subscription-started,
//! and subscription-stopped responses; the envelope-level patch-batch frame;
//! complete turn and user-message item values; all eight lifecycle
//! enumerators in domain order; and all five patch variants sharing their
//! patch identity and one-based sequence.
//!
//! The malformed shapes below prove raw representability only: out-of-range
//! query counts, an over-full batch, an oversized fragment, a zero
//! sequence, and structurally invalid snapshots all survive the wire
//! verbatim so owned conversion can reject them with typed errors instead of
//! the wire truncating them into validity. Nothing here
//! claims schema-level enforcement.
//!
//! Identifier vocabulary matches the schema header: opaque text ids of at
//! most 128 UTF-8 bytes, nonblank, without Unicode whitespace or control
//! characters. All string bounds are UTF-8 bytes.

use artisan_domain::{
    CONVERSATION_PATCH_BATCH_MAX_PATCHES, CONVERSATION_QUERY_MAX_TURNS,
    CONVERSATION_TEXT_FRAGMENT_MAX_BYTES,
};
use artisan_protocol::artisan_capnp::{
    ConversationLifecycle, conversation_item, conversation_patch, conversation_query_request,
    conversation_subscribe_request, conversation_subscription_started, conversation_turn, envelope,
    patch_batch, query_range, request, response, user_message_item,
};
use capnp::message::{Builder, HeapAllocator, ReaderOptions};
use capnp::serialize;

const PROTOCOL_VERSION: u32 = 1;
const CLIENT_REQUEST_ID: &str = "client-request-conversation";
const SENT_AT_MILLIS: i64 = 1_800_000_000_123;
const CREATED_AT_MILLIS: i64 = 1_800_000_100_456;
const UPDATED_AT_MILLIS: i64 = 1_800_000_200_789;
/// An instant before the Unix epoch, proving signed Int64 millisecond
/// survival across the wire.
const NEGATIVE_MILLIS: i64 = -62_167_219_600_001;

const THREAD_ID: &str = "thread_2718281828";
const TURN_ID_A: &str = "turn_alpha0";
const TURN_ID_B: &str = "turn_beta1";
/// A well-formed identifier absent from every snapshot that names it.
const TURN_ID_UNKNOWN: &str = "turn_absent9";
const ITEM_ID_A: &str = "item_alpha0";
const ITEM_ID_B: &str = "item_beta1";
const MESSAGE_BODY: &str = "Please summarize the failing module.";
const APPEND_FRAGMENT: &str = "streaming partial token";

const SNAPSHOT_CURSOR: u64 = 7;
const RESUME_CURSOR: u64 = 4;
const BATCH_FROM_CURSOR: u64 = 4;
const BATCH_TO_CURSOR: u64 = 9;

const PATCH_ID_TURN_UPSERT: &str = "patch_turn_upsert";
const PATCH_ID_ITEM_UPSERT: &str = "patch_item_upsert";
const PATCH_ID_ITEM_APPEND: &str = "patch_item_append";
const PATCH_ID_ITEM_LIFECYCLE: &str = "patch_item_lifecycle";
const PATCH_ID_TURN_LIFECYCLE: &str = "patch_turn_lifecycle";
const PATCH_ID_LIFECYCLE_WALK: &str = "patch_lifecycle_walk";

const REVISION_ZERO: u64 = 0;
const REVISION_THREE: u64 = 3;
const APPEND_REVISION: u64 = 4;
const ITEM_LIFECYCLE_REVISION: u64 = 5;
const TURN_LIFECYCLE_REVISION: u64 = 6;

const SNAPSHOT_RESPONSE_FRAME_ID: &str = "server-frame-300001";
const STARTED_FRESH_FRAME_ID: &str = "server-frame-300002";
const STARTED_RESUMED_FRAME_ID: &str = "server-frame-300003";
const STOPPED_FRAME_ID: &str = "server-frame-300004";
const PATCH_BATCH_FRAME_ID: &str = "server-frame-300005";
const LIFECYCLE_WALK_FRAME_ID: &str = "server-frame-300006";
const OVERSIZED_BATCH_FRAME_ID: &str = "server-frame-300007";
const INVALID_SNAPSHOT_ONE_FRAME_ID: &str = "server-frame-300008";
const INVALID_SNAPSHOT_TWO_FRAME_ID: &str = "server-frame-300009";

/// Every lifecycle enumerator in exact domain order.
const ALL_LIFECYCLES: [ConversationLifecycle; 8] = [
    ConversationLifecycle::Pending,
    ConversationLifecycle::Streaming,
    ConversationLifecycle::Active,
    ConversationLifecycle::Waiting,
    ConversationLifecycle::Completed,
    ConversationLifecycle::Failed,
    ConversationLifecycle::Interrupted,
    ConversationLifecycle::Cancelled,
];

/// Patch count of the lifecycle walk batch: one per enumerator.
const LIFECYCLE_WALK_LENGTH: u32 = 8;

/// Raw decoding stays explicitly finite: both limits are stated outright so
/// an upstream default change can never silently loosen test posture. The
/// traversal ceiling sits far above any legal frame; the nesting ceiling far
/// below stack-exhaustion depth.
const DECODE_TRAVERSAL_LIMIT_WORDS: usize = 16 * 1024 * 1024;
const DECODE_NESTING_LIMIT: i32 = 32;

fn frame() -> Builder<HeapAllocator> {
    Builder::new(HeapAllocator::new())
}

/// Stamps the shared header fields every frame carries.
fn init_envelope<'a>(
    message: &'a mut Builder<HeapAllocator>,
    message_id: &'a str,
) -> envelope::Builder<'a> {
    let mut root = message.init_root::<envelope::Builder>();
    root.set_protocol_version(PROTOCOL_VERSION);
    root.set_message_id(message_id);
    root.set_sent_at_millis(SENT_AT_MILLIS);
    root
}

/// Serializes deterministically so wire comparisons stay stable.
fn encode(message: &Builder<HeapAllocator>) -> Vec<u8> {
    serialize::write_message_to_words(message)
}

/// Reads one encoded frame back from its canonical byte form.
fn decode(
    bytes: &[u8],
) -> capnp::Result<capnp::message::Reader<capnp::serialize::BufferSegments<&[u8]>>> {
    let mut options = ReaderOptions::new();
    options.traversal_limit_in_words(Some(DECODE_TRAVERSAL_LIMIT_WORDS));
    options.nesting_limit(DECODE_NESTING_LIMIT);
    let mut encoded = bytes;
    serialize::read_message_from_flat_slice(&mut encoded, options)
}

/// Asserts the header fields survive a round trip unchanged.
fn assert_envelope_header(envelope: envelope::Reader<'_>, message_id: &str) -> capnp::Result<()> {
    assert_eq!(envelope.get_protocol_version(), PROTOCOL_VERSION);
    assert_eq!(envelope.get_message_id()?, message_id);
    assert_eq!(envelope.get_sent_at_millis(), SENT_AT_MILLIS);
    Ok(())
}

/// Stamps one canonical turn row; a complete value, never a projection.
fn set_turn(
    mut turn: conversation_turn::Builder<'_>,
    turn_id: &str,
    ordinal: u64,
    revision: u64,
    lifecycle: ConversationLifecycle,
    created_at_millis: i64,
    updated_at_millis: i64,
) {
    turn.set_turn_id(turn_id);
    turn.set_ordinal(ordinal);
    turn.set_revision(revision);
    turn.set_lifecycle(lifecycle);
    turn.set_created_at_millis(created_at_millis);
    turn.set_updated_at_millis(updated_at_millis);
}

/// Asserts one canonical turn row survived the round trip field for field.
fn assert_turn(
    turn: conversation_turn::Reader<'_>,
    turn_id: &str,
    ordinal: u64,
    revision: u64,
    lifecycle: ConversationLifecycle,
    created_at_millis: i64,
    updated_at_millis: i64,
) -> capnp::Result<()> {
    assert_eq!(turn.get_turn_id()?, turn_id);
    assert_eq!(turn.get_ordinal(), ordinal);
    assert_eq!(turn.get_revision(), revision);
    assert_eq!(turn.get_lifecycle()?, lifecycle);
    assert_eq!(turn.get_created_at_millis(), created_at_millis);
    assert_eq!(turn.get_updated_at_millis(), updated_at_millis);
    Ok(())
}

/// Stamps one complete durably queued user-message item.
fn set_user_message(
    mut item: user_message_item::Builder<'_>,
    item_id: &str,
    turn_id: &str,
    ordinal: u64,
    revision: u64,
    lifecycle: ConversationLifecycle,
) {
    item.set_item_id(item_id);
    item.set_turn_id(turn_id);
    item.set_ordinal(ordinal);
    item.set_revision(revision);
    item.set_lifecycle(lifecycle);
    item.set_body(MESSAGE_BODY);
    item.set_created_at_millis(CREATED_AT_MILLIS);
    item.set_updated_at_millis(UPDATED_AT_MILLIS);
}

/// Asserts one complete user-message item survived field for field.
fn assert_user_message(
    item: user_message_item::Reader<'_>,
    item_id: &str,
    turn_id: &str,
    ordinal: u64,
    revision: u64,
    lifecycle: ConversationLifecycle,
) -> capnp::Result<()> {
    assert_eq!(item.get_item_id()?, item_id);
    assert_eq!(item.get_turn_id()?, turn_id);
    assert_eq!(item.get_ordinal(), ordinal);
    assert_eq!(item.get_revision(), revision);
    assert_eq!(item.get_lifecycle()?, lifecycle);
    assert_eq!(item.get_body()?, MESSAGE_BODY);
    assert_eq!(item.get_created_at_millis(), CREATED_AT_MILLIS);
    assert_eq!(item.get_updated_at_millis(), UPDATED_AT_MILLIS);
    Ok(())
}

#[test]
fn round_trips_conversation_query_window_request() -> capnp::Result<()> {
    // Request: newest-N read at the documented inclusive ceiling. The
    // 16-bit wire count mirrors the domain's QueryTurnCount exactly while
    // still representing out-of-range values such as 513 (see the malformed
    // representability test below).
    let encoded = {
        let mut message = frame();
        let mut query = init_envelope(&mut message, CLIENT_REQUEST_ID)
            .init_body()
            .init_request()
            .init_conversation_query();
        query.set_thread_id(THREAD_ID);
        query
            .init_bounds()
            .init_window()
            .set_maximum_turn_count(CONVERSATION_QUERY_MAX_TURNS);
        encode(&message)
    };

    let decoded = decode(&encoded)?;
    let root: envelope::Reader = decoded.get_root()?;
    assert_envelope_header(root, CLIENT_REQUEST_ID)?;
    match root.get_body().which()? {
        envelope::body::Which::Request(requested) => match requested?.which()? {
            request::Which::ConversationQuery(query) => {
                let query = query?;
                assert_eq!(query.get_thread_id()?, THREAD_ID);
                match query.get_bounds().which()? {
                    conversation_query_request::bounds::Which::Window(window) => {
                        assert_eq!(
                            window?.get_maximum_turn_count(),
                            CONVERSATION_QUERY_MAX_TURNS,
                        );
                    }
                    conversation_query_request::bounds::Which::Range(_) => {
                        panic!("expected window bounds")
                    }
                }
            }
            _ => panic!("expected conversationQuery request"),
        },
        _ => panic!("expected request body"),
    }

    Ok(())
}

#[test]
fn round_trips_conversation_query_range_with_floor() -> capnp::Result<()> {
    // Request: older-history read with an inclusive floor. Ordinals are
    // zero-based, so "absent floor" rides a union rather than a sentinel --
    // zero must stay expressible AS a floor.
    let encoded = {
        let mut message = frame();
        {
            let mut query = init_envelope(&mut message, CLIENT_REQUEST_ID)
                .init_body()
                .init_request()
                .init_conversation_query();
            query.set_thread_id(THREAD_ID);
            let mut range = query.init_bounds().init_range();
            range.set_before_turn_ordinal(12);
            range.reborrow().init_minimum_turn_ordinal().set_minimum(2);
            range.set_maximum_turn_count(16);
        }
        encode(&message)
    };

    assert_range_frame(&encoded, Some(2), 16)?;
    Ok(())
}

#[test]
fn round_trips_conversation_query_range_without_floor() -> capnp::Result<()> {
    // Request: open-ended paging toward older history, no floor, at the
    // documented inclusive turn ceiling.
    let encoded = {
        let mut message = frame();
        {
            let mut query = init_envelope(&mut message, CLIENT_REQUEST_ID)
                .init_body()
                .init_request()
                .init_conversation_query();
            query.set_thread_id(THREAD_ID);
            let mut range = query.init_bounds().init_range();
            range.set_before_turn_ordinal(12);
            range
                .reborrow()
                .init_minimum_turn_ordinal()
                .set_no_minimum(());
            range.set_maximum_turn_count(CONVERSATION_QUERY_MAX_TURNS);
        }
        encode(&message)
    };

    assert_range_frame(&encoded, None, CONVERSATION_QUERY_MAX_TURNS)?;
    Ok(())
}

/// Asserts one encoded range-query frame decodes with the expected floor and
/// turn count.
fn assert_range_frame(
    bytes: &[u8],
    expected_floor: Option<u64>,
    expected_count: u16,
) -> capnp::Result<()> {
    let decoded = decode(bytes)?;
    let root: envelope::Reader = decoded.get_root()?;
    assert_envelope_header(root, CLIENT_REQUEST_ID)?;
    match root.get_body().which()? {
        envelope::body::Which::Request(requested) => match requested?.which()? {
            request::Which::ConversationQuery(query) => {
                let query = query?;
                assert_eq!(query.get_thread_id()?, THREAD_ID);
                match query.get_bounds().which()? {
                    conversation_query_request::bounds::Which::Range(range) => {
                        let range = range?;
                        assert_eq!(range.get_before_turn_ordinal(), 12);
                        match (range.get_minimum_turn_ordinal().which()?, expected_floor) {
                            (
                                query_range::minimum_turn_ordinal::Which::Minimum(floor),
                                Some(expected),
                            ) => {
                                assert_eq!(floor, expected);
                            }
                            (query_range::minimum_turn_ordinal::Which::NoMinimum(()), None) => {}
                            _ => panic!("floor presence mismatch"),
                        }
                        assert_eq!(range.get_maximum_turn_count(), expected_count);
                    }
                    conversation_query_request::bounds::Which::Window(_) => {
                        panic!("expected range bounds")
                    }
                }
            }
            _ => panic!("expected conversationQuery request"),
        },
        _ => panic!("expected request body"),
    }

    Ok(())
}

#[test]
fn round_trips_conversation_subscribe_fresh() -> capnp::Result<()> {
    // Request: fresh subscription whose first delivered server value must be
    // a full snapshot.
    let encoded = {
        let mut message = frame();
        let mut subscribe = init_envelope(&mut message, CLIENT_REQUEST_ID)
            .init_body()
            .init_request()
            .init_conversation_subscribe();
        subscribe.set_thread_id(THREAD_ID);
        subscribe.init_start().set_fresh(());
        encode(&message)
    };

    let decoded = decode(&encoded)?;
    let root: envelope::Reader = decoded.get_root()?;
    assert_envelope_header(root, CLIENT_REQUEST_ID)?;
    match root.get_body().which()? {
        envelope::body::Which::Request(requested) => match requested?.which()? {
            request::Which::ConversationSubscribe(subscribe) => {
                let subscribe = subscribe?;
                assert_eq!(subscribe.get_thread_id()?, THREAD_ID);
                match subscribe.get_start().which()? {
                    conversation_subscribe_request::start::Which::Fresh(()) => {}
                    conversation_subscribe_request::start::Which::ResumeAfter(_) => {
                        panic!("expected fresh start")
                    }
                }
            }
            _ => panic!("expected conversationSubscribe request"),
        },
        _ => panic!("expected request body"),
    }

    Ok(())
}

#[test]
fn round_trips_conversation_subscribe_resume_after_cursor() -> capnp::Result<()> {
    // Request: resume strictly after a previously applied cursor. Zero is a
    // legitimate cursor meaning nothing was applied yet, so the union carries
    // it explicitly rather than conflating absence with zero.
    for after in [0, RESUME_CURSOR] {
        let encoded = {
            let mut message = frame();
            let mut subscribe = init_envelope(&mut message, CLIENT_REQUEST_ID)
                .init_body()
                .init_request()
                .init_conversation_subscribe();
            subscribe.set_thread_id(THREAD_ID);
            subscribe.init_start().set_resume_after(after);
            encode(&message)
        };

        let decoded = decode(&encoded)?;
        let root: envelope::Reader = decoded.get_root()?;
        assert_envelope_header(root, CLIENT_REQUEST_ID)?;
        match root.get_body().which()? {
            envelope::body::Which::Request(requested) => match requested?.which()? {
                request::Which::ConversationSubscribe(subscribe) => {
                    let subscribe = subscribe?;
                    assert_eq!(subscribe.get_thread_id()?, THREAD_ID);
                    match subscribe.get_start().which()? {
                        conversation_subscribe_request::start::Which::ResumeAfter(cursor) => {
                            assert_eq!(cursor, after);
                        }
                        conversation_subscribe_request::start::Which::Fresh(()) => {
                            panic!("expected resume start")
                        }
                    }
                }
                _ => panic!("expected conversationSubscribe request"),
            },
            _ => panic!("expected request body"),
        }
    }

    Ok(())
}

#[test]
fn round_trips_conversation_unsubscribe_request() -> capnp::Result<()> {
    let encoded = {
        let mut message = frame();
        init_envelope(&mut message, CLIENT_REQUEST_ID)
            .init_body()
            .init_request()
            .reborrow()
            .init_conversation_unsubscribe()
            .set_thread_id(THREAD_ID);
        encode(&message)
    };

    let decoded = decode(&encoded)?;
    let root: envelope::Reader = decoded.get_root()?;
    assert_envelope_header(root, CLIENT_REQUEST_ID)?;
    match root.get_body().which()? {
        envelope::body::Which::Request(requested) => match requested?.which()? {
            request::Which::ConversationUnsubscribe(unsubscribe) => {
                assert_eq!(unsubscribe?.get_thread_id()?, THREAD_ID);
            }
            _ => panic!("expected conversationUnsubscribe request"),
        },
        _ => panic!("expected request body"),
    }

    Ok(())
}

/// Builds one response frame carrying the complete canonical snapshot
/// fixture: two turns (one stamped before the Unix epoch) and one complete
/// user-message item, all at a positive replay cursor.
fn full_snapshot_frame(frame_id: &str) -> Vec<u8> {
    let mut message = frame();
    let mut res = init_envelope(&mut message, frame_id)
        .init_body()
        .init_response();
    res.set_request_id(CLIENT_REQUEST_ID);
    let mut snapshot = res.init_conversation_snapshot();
    snapshot.set_thread_id(THREAD_ID);
    snapshot.set_cursor(SNAPSHOT_CURSOR);

    let mut turns = snapshot.reborrow().init_turns(2);
    set_turn(
        turns.reborrow().get(0),
        TURN_ID_A,
        0,
        REVISION_ZERO,
        ConversationLifecycle::Pending,
        CREATED_AT_MILLIS,
        UPDATED_AT_MILLIS,
    );
    set_turn(
        turns.get(1),
        TURN_ID_B,
        1,
        REVISION_THREE,
        ConversationLifecycle::Active,
        NEGATIVE_MILLIS,
        UPDATED_AT_MILLIS,
    );

    let items = snapshot.reborrow().init_items(1);
    set_user_message(
        items.get(0).init_user_message(),
        ITEM_ID_A,
        TURN_ID_A,
        2,
        REVISION_ZERO,
        ConversationLifecycle::Completed,
    );

    snapshot.set_updated_at_millis(UPDATED_AT_MILLIS);
    encode(&message)
}

/// Asserts a decoded response frame is the complete snapshot fixture.
fn assert_full_snapshot_response(bytes: &[u8], message_id: &str) -> capnp::Result<()> {
    let decoded = decode(bytes)?;
    let root: envelope::Reader = decoded.get_root()?;
    assert_envelope_header(root, message_id)?;
    match root.get_body().which()? {
        envelope::body::Which::Response(res) => {
            let res = res?;
            assert_eq!(res.get_request_id()?, CLIENT_REQUEST_ID);
            match res.which()? {
                response::Which::ConversationSnapshot(snapshot) => {
                    let snapshot = snapshot?;
                    assert_eq!(snapshot.get_thread_id()?, THREAD_ID);
                    assert_eq!(snapshot.get_cursor(), SNAPSHOT_CURSOR);

                    let turns = snapshot.get_turns()?;
                    assert_eq!(turns.len(), 2);
                    assert_turn(
                        turns.get(0),
                        TURN_ID_A,
                        0,
                        REVISION_ZERO,
                        ConversationLifecycle::Pending,
                        CREATED_AT_MILLIS,
                        UPDATED_AT_MILLIS,
                    )?;
                    assert_turn(
                        turns.get(1),
                        TURN_ID_B,
                        1,
                        REVISION_THREE,
                        ConversationLifecycle::Active,
                        NEGATIVE_MILLIS,
                        UPDATED_AT_MILLIS,
                    )?;

                    let items = snapshot.get_items()?;
                    assert_eq!(items.len(), 1);
                    match items.get(0).which()? {
                        conversation_item::Which::UserMessage(item) => {
                            assert_user_message(
                                item?,
                                ITEM_ID_A,
                                TURN_ID_A,
                                2,
                                REVISION_ZERO,
                                ConversationLifecycle::Completed,
                            )?;
                        }
                        conversation_item::Which::Unmodeled(()) => {
                            panic!("expected userMessage item")
                        }
                    }

                    assert_eq!(snapshot.get_updated_at_millis(), UPDATED_AT_MILLIS);
                }
                _ => panic!("expected conversationSnapshot response"),
            }
        }
        _ => panic!("expected response body"),
    }

    Ok(())
}

#[test]
fn round_trips_full_conversation_snapshot_response() -> capnp::Result<()> {
    // Response arm @7: bounded canonical snapshot round-trips completely,
    // including the negative creation timestamp of the second turn.
    assert_full_snapshot_response(
        &full_snapshot_frame(SNAPSHOT_RESPONSE_FRAME_ID),
        SNAPSHOT_RESPONSE_FRAME_ID,
    )?;
    Ok(())
}

#[test]
fn round_trips_subscription_started_fresh() -> capnp::Result<()> {
    // Response arm @8, fresh member: acknowledgement opens with a complete
    // snapshot establishing the projection.
    let encoded = {
        let mut message = frame();
        let mut res = init_envelope(&mut message, STARTED_FRESH_FRAME_ID)
            .init_body()
            .init_response();
        res.set_request_id(CLIENT_REQUEST_ID);
        let mut started = res.init_conversation_subscription_started();
        let mut snapshot = started.reborrow().init_fresh();
        snapshot.set_thread_id(THREAD_ID);
        snapshot.set_cursor(2);

        let turns = snapshot.reborrow().init_turns(1);
        set_turn(
            turns.get(0),
            TURN_ID_A,
            0,
            REVISION_ZERO,
            ConversationLifecycle::Waiting,
            CREATED_AT_MILLIS,
            UPDATED_AT_MILLIS,
        );
        snapshot.reborrow().init_items(0);
        snapshot.set_updated_at_millis(UPDATED_AT_MILLIS);
        encode(&message)
    };

    let decoded = decode(&encoded)?;
    let root: envelope::Reader = decoded.get_root()?;
    assert_envelope_header(root, STARTED_FRESH_FRAME_ID)?;
    match root.get_body().which()? {
        envelope::body::Which::Response(res) => {
            let res = res?;
            assert_eq!(res.get_request_id()?, CLIENT_REQUEST_ID);
            match res.which()? {
                response::Which::ConversationSubscriptionStarted(started) => {
                    let started = started?;
                    match started.which()? {
                        conversation_subscription_started::Which::Fresh(snapshot) => {
                            let snapshot = snapshot?;
                            assert_eq!(snapshot.get_thread_id()?, THREAD_ID);
                            assert_eq!(snapshot.get_cursor(), 2);
                            assert_eq!(snapshot.get_turns()?.len(), 1);
                            assert_eq!(snapshot.get_items()?.len(), 0);
                        }
                        conversation_subscription_started::Which::Resumed(_) => {
                            panic!("expected fresh start")
                        }
                    }
                }
                _ => panic!("expected conversationSubscriptionStarted response"),
            }
        }
        _ => panic!("expected response body"),
    }

    Ok(())
}

#[test]
fn round_trips_subscription_started_resumed() -> capnp::Result<()> {
    // Response arm @8, resumed member: acknowledgement carries the
    // continuation point only -- the last already-applied cursor, with
    // delivery restarting at cursor + 1.
    let encoded = {
        let mut message = frame();
        let mut res = init_envelope(&mut message, STARTED_RESUMED_FRAME_ID)
            .init_body()
            .init_response();
        res.set_request_id(CLIENT_REQUEST_ID);
        let started = res.init_conversation_subscription_started();
        let mut point = started.init_resumed();
        point.set_thread_id(THREAD_ID);
        point.set_cursor(RESUME_CURSOR);
        encode(&message)
    };

    let decoded = decode(&encoded)?;
    let root: envelope::Reader = decoded.get_root()?;
    assert_envelope_header(root, STARTED_RESUMED_FRAME_ID)?;
    match root.get_body().which()? {
        envelope::body::Which::Response(res) => {
            let res = res?;
            assert_eq!(res.get_request_id()?, CLIENT_REQUEST_ID);
            match res.which()? {
                response::Which::ConversationSubscriptionStarted(started) => {
                    let started = started?;
                    match started.which()? {
                        conversation_subscription_started::Which::Resumed(point) => {
                            let point = point?;
                            assert_eq!(point.get_thread_id()?, THREAD_ID);
                            assert_eq!(point.get_cursor(), RESUME_CURSOR);
                        }
                        conversation_subscription_started::Which::Fresh(_) => {
                            panic!("expected resumed start")
                        }
                    }
                }
                _ => panic!("expected conversationSubscriptionStarted response"),
            }
        }
        _ => panic!("expected response body"),
    }

    Ok(())
}

#[test]
fn round_trips_subscription_stopped_response() -> capnp::Result<()> {
    // Response arm @9: clean stop names only the thread released.
    let encoded = {
        let mut message = frame();
        let mut res = init_envelope(&mut message, STOPPED_FRAME_ID)
            .init_body()
            .init_response();
        res.set_request_id(CLIENT_REQUEST_ID);
        res.init_conversation_subscription_stopped()
            .set_thread_id(THREAD_ID);
        encode(&message)
    };

    let decoded = decode(&encoded)?;
    let root: envelope::Reader = decoded.get_root()?;
    assert_envelope_header(root, STOPPED_FRAME_ID)?;
    match root.get_body().which()? {
        envelope::body::Which::Response(res) => {
            let res = res?;
            assert_eq!(res.get_request_id()?, CLIENT_REQUEST_ID);
            match res.which()? {
                response::Which::ConversationSubscriptionStopped(stopped) => {
                    assert_eq!(stopped?.get_thread_id()?, THREAD_ID);
                }
                _ => panic!("expected conversationSubscriptionStopped response"),
            }
        }
        _ => panic!("expected response body"),
    }

    Ok(())
}

/// Builds one envelope-level patch batch advancing a subscriber from cursor
/// 4 to cursor 9 across five contiguous sequences -- one per patch variant,
/// each carrying its complete value.
fn five_variant_batch_frame(frame_id: &str) -> Vec<u8> {
    let mut message = frame();
    let mut batch = init_envelope(&mut message, frame_id)
        .init_body()
        .init_patch_batch();
    batch.set_thread_id(THREAD_ID);
    batch.set_from_cursor(BATCH_FROM_CURSOR);
    batch.set_to_cursor(BATCH_TO_CURSOR);
    let mut patches = batch.init_patches(5);

    let mut upsert = patches.reborrow().get(0);
    upsert.set_patch_id(PATCH_ID_TURN_UPSERT);
    upsert.set_sequence(5);
    set_turn(
        upsert.init_turn_upsert(),
        TURN_ID_B,
        1,
        REVISION_THREE,
        ConversationLifecycle::Active,
        NEGATIVE_MILLIS,
        UPDATED_AT_MILLIS,
    );

    let mut upsert = patches.reborrow().get(1);
    upsert.set_patch_id(PATCH_ID_ITEM_UPSERT);
    upsert.set_sequence(6);
    set_user_message(
        upsert.init_item_upsert().init_user_message(),
        ITEM_ID_A,
        TURN_ID_A,
        2,
        REVISION_ZERO,
        ConversationLifecycle::Completed,
    );

    let mut append = patches.reborrow().get(2);
    append.set_patch_id(PATCH_ID_ITEM_APPEND);
    append.set_sequence(7);
    {
        let mut payload = append.init_item_append();
        payload.set_item_id(ITEM_ID_A);
        payload.set_revision(APPEND_REVISION);
        payload.set_text(APPEND_FRAGMENT);
    }

    let mut transition = patches.reborrow().get(3);
    transition.set_patch_id(PATCH_ID_ITEM_LIFECYCLE);
    transition.set_sequence(8);
    {
        let mut payload = transition.init_item_lifecycle();
        payload.set_item_id(ITEM_ID_A);
        payload.set_revision(ITEM_LIFECYCLE_REVISION);
        payload.set_lifecycle(ConversationLifecycle::Failed);
    }

    let mut transition = patches.get(4);
    transition.set_patch_id(PATCH_ID_TURN_LIFECYCLE);
    transition.set_sequence(9);
    {
        let mut payload = transition.init_turn_lifecycle();
        payload.set_turn_id(TURN_ID_B);
        payload.set_revision(TURN_LIFECYCLE_REVISION);
        payload.set_lifecycle(ConversationLifecycle::Cancelled);
    }

    encode(&message)
}

/// Asserts the shared header of the five-variant batch fixture.
fn assert_five_variant_batch_header(batch: patch_batch::Reader<'_>) -> capnp::Result<()> {
    assert_eq!(batch.get_thread_id()?, THREAD_ID);
    assert_eq!(batch.get_from_cursor(), BATCH_FROM_CURSOR);
    assert_eq!(batch.get_to_cursor(), BATCH_TO_CURSOR);
    Ok(())
}

#[test]
fn round_trips_upsert_patch_variants_in_one_batch() -> capnp::Result<()> {
    // Envelope arm: the first two variants insert or replace complete
    // values -- a canonical turn and a renderer-visible item.
    let encoded = five_variant_batch_frame(PATCH_BATCH_FRAME_ID);
    let decoded = decode(&encoded)?;
    let root: envelope::Reader = decoded.get_root()?;
    assert_envelope_header(root, PATCH_BATCH_FRAME_ID)?;
    match root.get_body().which()? {
        envelope::body::Which::PatchBatch(batch) => {
            let batch = batch?;
            assert_five_variant_batch_header(batch)?;

            let patches = batch.get_patches()?;
            assert_eq!(patches.len(), 5);

            match patches.get(0).which()? {
                conversation_patch::Which::TurnUpsert(turn) => {
                    let patch = patches.get(0);
                    assert_eq!(patch.get_patch_id()?, PATCH_ID_TURN_UPSERT);
                    assert_eq!(patch.get_sequence(), 5);
                    assert_turn(
                        turn?,
                        TURN_ID_B,
                        1,
                        REVISION_THREE,
                        ConversationLifecycle::Active,
                        NEGATIVE_MILLIS,
                        UPDATED_AT_MILLIS,
                    )?;
                }
                _ => panic!("expected turnUpsert patch"),
            }

            match patches.get(1).which()? {
                conversation_patch::Which::ItemUpsert(item) => {
                    let patch = patches.get(1);
                    assert_eq!(patch.get_patch_id()?, PATCH_ID_ITEM_UPSERT);
                    assert_eq!(patch.get_sequence(), 6);
                    match item?.which()? {
                        conversation_item::Which::UserMessage(message) => {
                            assert_user_message(
                                message?,
                                ITEM_ID_A,
                                TURN_ID_A,
                                2,
                                REVISION_ZERO,
                                ConversationLifecycle::Completed,
                            )?;
                        }
                        conversation_item::Which::Unmodeled(()) => {
                            panic!("expected userMessage item")
                        }
                    }
                }
                _ => panic!("expected itemUpsert patch"),
            }
        }
        _ => panic!("expected patchBatch body"),
    }

    Ok(())
}

#[test]
fn round_trips_append_and_lifecycle_patch_variants_in_one_batch() -> capnp::Result<()> {
    // Envelope arm: the remaining three variants mutate in place -- a
    // bounded text fragment append plus item- and turn-scoped lifecycle
    // transitions.
    let encoded = five_variant_batch_frame(PATCH_BATCH_FRAME_ID);
    let decoded = decode(&encoded)?;
    let root: envelope::Reader = decoded.get_root()?;
    assert_envelope_header(root, PATCH_BATCH_FRAME_ID)?;
    match root.get_body().which()? {
        envelope::body::Which::PatchBatch(batch) => {
            let batch = batch?;
            assert_five_variant_batch_header(batch)?;

            let patches = batch.get_patches()?;
            assert_eq!(patches.len(), 5);

            match patches.get(2).which()? {
                conversation_patch::Which::ItemAppend(payload) => {
                    let patch = patches.get(2);
                    assert_eq!(patch.get_patch_id()?, PATCH_ID_ITEM_APPEND);
                    assert_eq!(patch.get_sequence(), 7);
                    let payload = payload?;
                    assert_eq!(payload.get_item_id()?, ITEM_ID_A);
                    assert_eq!(payload.get_revision(), APPEND_REVISION);
                    assert_eq!(payload.get_text()?, APPEND_FRAGMENT);
                }
                _ => panic!("expected itemAppend patch"),
            }

            match patches.get(3).which()? {
                conversation_patch::Which::ItemLifecycle(payload) => {
                    let patch = patches.get(3);
                    assert_eq!(patch.get_patch_id()?, PATCH_ID_ITEM_LIFECYCLE);
                    assert_eq!(patch.get_sequence(), 8);
                    let payload = payload?;
                    assert_eq!(payload.get_item_id()?, ITEM_ID_A);
                    assert_eq!(payload.get_revision(), ITEM_LIFECYCLE_REVISION);
                    assert_eq!(payload.get_lifecycle()?, ConversationLifecycle::Failed,);
                }
                _ => panic!("expected itemLifecycle patch"),
            }

            match patches.get(4).which()? {
                conversation_patch::Which::TurnLifecycle(payload) => {
                    let patch = patches.get(4);
                    assert_eq!(patch.get_patch_id()?, PATCH_ID_TURN_LIFECYCLE);
                    assert_eq!(patch.get_sequence(), 9);
                    let payload = payload?;
                    assert_eq!(payload.get_turn_id()?, TURN_ID_B);
                    assert_eq!(payload.get_revision(), TURN_LIFECYCLE_REVISION);
                    assert_eq!(payload.get_lifecycle()?, ConversationLifecycle::Cancelled,);
                }
                _ => panic!("expected turnLifecycle patch"),
            }
        }
        _ => panic!("expected patchBatch body"),
    }

    Ok(())
}

#[test]
fn round_trips_every_lifecycle_enumerator_in_domain_order() -> capnp::Result<()> {
    // Batch walk: one item-lifecycle patch per enumerator, in exact domain
    // order, verifying the numeric wire encoding preserves the sequence so
    // appended enumerators can never silently reorder existing ones.
    let encoded = {
        let mut message = frame();
        let mut batch = init_envelope(&mut message, LIFECYCLE_WALK_FRAME_ID)
            .init_body()
            .init_patch_batch();
        batch.set_thread_id(THREAD_ID);
        batch.set_from_cursor(0);
        batch.set_to_cursor(u64::from(LIFECYCLE_WALK_LENGTH));
        let mut patches = batch.init_patches(LIFECYCLE_WALK_LENGTH);

        for (slot, expected) in (0u32..).zip(ALL_LIFECYCLES) {
            let mut patch = patches.reborrow().get(slot);
            patch.set_patch_id(PATCH_ID_LIFECYCLE_WALK);
            patch.set_sequence(u64::from(slot) + 1);
            let mut payload = patch.init_item_lifecycle();
            payload.set_item_id(ITEM_ID_A);
            payload.set_revision(u64::from(slot));
            payload.set_lifecycle(expected);
        }

        encode(&message)
    };

    let decoded = decode(&encoded)?;
    let root: envelope::Reader = decoded.get_root()?;
    assert_envelope_header(root, LIFECYCLE_WALK_FRAME_ID)?;
    match root.get_body().which()? {
        envelope::body::Which::PatchBatch(batch) => {
            let batch = batch?;
            let patches = batch.get_patches()?;
            assert_eq!(patches.len(), LIFECYCLE_WALK_LENGTH);

            let mut expected_sequence: u64 = 1;
            for (patch, expected) in patches.iter().zip(ALL_LIFECYCLES.iter()) {
                assert_eq!(patch.get_patch_id()?, PATCH_ID_LIFECYCLE_WALK);
                assert_eq!(patch.get_sequence(), expected_sequence);
                match patch.which()? {
                    conversation_patch::Which::ItemLifecycle(payload) => {
                        let payload = payload?;
                        assert_eq!(payload.get_item_id()?, ITEM_ID_A);
                        assert_eq!(payload.get_revision(), expected_sequence - 1);
                        assert_eq!(payload.get_lifecycle()?, *expected);
                    }
                    _ => panic!("expected itemLifecycle patch"),
                }
                expected_sequence += 1;
            }
            assert_eq!(expected_sequence, u64::from(LIFECYCLE_WALK_LENGTH) + 1,);
        }
        _ => panic!("expected patchBatch body"),
    }

    Ok(())
}

#[test]
fn malformed_query_turn_counts_stay_representable() -> capnp::Result<()> {
    // Negative coverage available at the schema layer: the 16-bit wire count
    // carries out-of-range values verbatim -- 513 fits comfortably. Owned
    // conversion accepts exactly 1..=512 and must reject zero (empty query)
    // and ceiling+1 (513) with typed errors; the wire mirrors QueryTurnCount's
    // width while still representing the invalid values, so hostile peers
    // stay visible instead of truncating into valid-looking requests. Only
    // the turn count is malformed: every frame still names its valid thread
    // id, so an owned rejection stays attributable to the count alone.
    let oversized = CONVERSATION_QUERY_MAX_TURNS + 1;
    for count in [0, oversized] {
        let encoded = {
            let mut message = frame();
            let mut query = init_envelope(&mut message, CLIENT_REQUEST_ID)
                .init_body()
                .init_request()
                .init_conversation_query();
            query.set_thread_id(THREAD_ID);
            query
                .init_bounds()
                .init_window()
                .set_maximum_turn_count(count);
            encode(&message)
        };

        let decoded = decode(&encoded)?;
        let root: envelope::Reader = decoded.get_root()?;
        assert_envelope_header(root, CLIENT_REQUEST_ID)?;
        match root.get_body().which()? {
            envelope::body::Which::Request(requested) => match requested?.which()? {
                request::Which::ConversationQuery(query) => {
                    let query = query?;
                    assert_eq!(query.get_thread_id()?, THREAD_ID);
                    match query.get_bounds().which()? {
                        conversation_query_request::bounds::Which::Window(window) => {
                            assert_eq!(window?.get_maximum_turn_count(), count);
                        }
                        conversation_query_request::bounds::Which::Range(_) => {
                            panic!("expected window bounds")
                        }
                    }
                }
                _ => panic!("expected conversationQuery request"),
            },
            _ => panic!("expected request body"),
        }
    }

    Ok(())
}

#[test]
fn oversized_patch_batch_stays_representable() -> capnp::Result<()> {
    // Negative coverage available at the schema layer: a batch holding one
    // more than the documented 64-patch ceiling still encodes and decodes
    // intact. Owned conversion enforces 1..=64 and must reject this batch
    // size with a typed error rather than truncate it.
    let oversized_count =
        u32::try_from(CONVERSATION_PATCH_BATCH_MAX_PATCHES + 1).expect("fixture count fits u32");
    let encoded = {
        let mut message = frame();
        let mut batch = init_envelope(&mut message, OVERSIZED_BATCH_FRAME_ID)
            .init_body()
            .init_patch_batch();
        batch.set_thread_id(THREAD_ID);
        batch.set_from_cursor(0);
        batch.set_to_cursor(u64::from(oversized_count));
        let mut patches = batch.init_patches(oversized_count);

        let mut slot: u32 = 0;
        while slot < oversized_count {
            let mut patch = patches.reborrow().get(slot);
            patch.set_patch_id(PATCH_ID_LIFECYCLE_WALK);
            patch.set_sequence(u64::from(slot) + 1);
            let mut payload = patch.init_item_lifecycle();
            payload.set_item_id(ITEM_ID_B);
            payload.set_revision(REVISION_ZERO);
            payload.set_lifecycle(ConversationLifecycle::Streaming);
            slot += 1;
        }

        encode(&message)
    };

    let decoded = decode(&encoded)?;
    let root: envelope::Reader = decoded.get_root()?;
    assert_envelope_header(root, OVERSIZED_BATCH_FRAME_ID)?;
    match root.get_body().which()? {
        envelope::body::Which::PatchBatch(batch) => {
            let batch = batch?;
            let patches = batch.get_patches()?;
            assert_eq!(patches.len(), oversized_count);
            assert_eq!(patches.get(0).get_sequence(), 1);
            let last = patches.get(oversized_count - 1);
            assert_eq!(last.get_sequence(), u64::from(oversized_count));
            match last.which()? {
                conversation_patch::Which::ItemLifecycle(payload) => {
                    let payload = payload?;
                    assert_eq!(payload.get_item_id()?, ITEM_ID_B);
                    assert_eq!(payload.get_lifecycle()?, ConversationLifecycle::Streaming,);
                }
                _ => panic!("expected itemLifecycle patch"),
            }
        }
        _ => panic!("expected patchBatch body"),
    }

    Ok(())
}

#[test]
fn oversized_append_fragment_stays_representable() -> capnp::Result<()> {
    // Negative coverage available at the schema layer: one byte beyond the
    // 4096-byte fragment ceiling still travels the wire untouched. Owned
    // conversion enforces the ceiling (empty stays legal; oversize rejects);
    // the wire shape deliberately represents the violation.
    let fragment = "x".repeat(CONVERSATION_TEXT_FRAGMENT_MAX_BYTES + 1);
    let encoded = {
        let mut message = frame();
        let mut batch = init_envelope(&mut message, PATCH_BATCH_FRAME_ID)
            .init_body()
            .init_patch_batch();
        batch.set_thread_id(THREAD_ID);
        batch.set_from_cursor(0);
        batch.set_to_cursor(1);

        let mut patch = batch.init_patches(1).get(0);
        patch.set_patch_id(PATCH_ID_ITEM_APPEND);
        patch.set_sequence(1);
        let mut payload = patch.init_item_append();
        payload.set_item_id(ITEM_ID_B);
        payload.set_revision(APPEND_REVISION);
        payload.set_text(&fragment);

        encode(&message)
    };

    let decoded = decode(&encoded)?;
    let root: envelope::Reader = decoded.get_root()?;
    assert_envelope_header(root, PATCH_BATCH_FRAME_ID)?;
    match root.get_body().which()? {
        envelope::body::Which::PatchBatch(batch) => match batch?.get_patches()?.get(0).which()? {
            conversation_patch::Which::ItemAppend(payload) => {
                let received = payload?.get_text()?;
                assert_eq!(received.len(), CONVERSATION_TEXT_FRAGMENT_MAX_BYTES + 1);
                assert_eq!(received, fragment);
            }
            _ => panic!("expected itemAppend patch"),
        },
        _ => panic!("expected patchBatch body"),
    }

    Ok(())
}

#[test]
fn zero_patch_sequence_stays_representable() -> capnp::Result<()> {
    // Negative coverage available at the schema layer: patch sequences are
    // one-based, yet the UInt64 wire type carries zero verbatim. Zero is
    // reserved for cursors ("before the first patch"); owned conversion must
    // reject a zero sequence with a typed error rather than misplace the
    // patch in replay order.
    let encoded = {
        let mut message = frame();
        let mut batch = init_envelope(&mut message, PATCH_BATCH_FRAME_ID)
            .init_body()
            .init_patch_batch();
        batch.set_thread_id(THREAD_ID);
        batch.set_from_cursor(0);
        batch.set_to_cursor(0);

        let mut patch = batch.init_patches(1).get(0);
        patch.set_patch_id(PATCH_ID_ITEM_APPEND);
        patch.set_sequence(0);
        let mut payload = patch.init_item_append();
        payload.set_item_id(ITEM_ID_B);
        payload.set_revision(REVISION_ZERO);
        payload.set_text("");

        encode(&message)
    };

    let decoded = decode(&encoded)?;
    let root: envelope::Reader = decoded.get_root()?;
    assert_envelope_header(root, PATCH_BATCH_FRAME_ID)?;
    match root.get_body().which()? {
        envelope::body::Which::PatchBatch(batch) => {
            let batch = batch?;
            assert_eq!(batch.get_from_cursor(), 0);
            assert_eq!(batch.get_to_cursor(), 0);
            match batch.get_patches()?.get(0).which()? {
                conversation_patch::Which::ItemAppend(payload) => {
                    let patch = batch.get_patches()?.get(0);
                    assert_eq!(patch.get_sequence(), 0);
                    let payload = payload?;
                    assert_eq!(payload.get_item_id()?, ITEM_ID_B);
                    assert_eq!(payload.get_text()?, "");
                }
                _ => panic!("expected itemAppend patch"),
            }
        }
        _ => panic!("expected patchBatch body"),
    }

    Ok(())
}

#[test]
fn duplicate_identity_snapshots_stay_representable() -> capnp::Result<()> {
    // Negative coverage available at the schema layer: nothing in the wire
    // shape stops two turns from sharing one Forge identity or two items
    // from sharing one item identity. This invalid snapshot decodes
    // verbatim; owned conversion owns the structural rejections (duplicate
    // turn id / duplicate item id).
    let encoded = {
        let mut message = frame();
        let mut res = init_envelope(&mut message, INVALID_SNAPSHOT_ONE_FRAME_ID)
            .init_body()
            .init_response();
        res.set_request_id(CLIENT_REQUEST_ID);
        let mut snapshot = res.init_conversation_snapshot();
        snapshot.set_thread_id(THREAD_ID);
        snapshot.set_cursor(SNAPSHOT_CURSOR);

        // Same Forge turn identity twice; ordinals differ.
        let mut turns = snapshot.reborrow().init_turns(2);
        set_turn(
            turns.reborrow().get(0),
            TURN_ID_A,
            0,
            REVISION_ZERO,
            ConversationLifecycle::Pending,
            CREATED_AT_MILLIS,
            UPDATED_AT_MILLIS,
        );
        set_turn(
            turns.get(1),
            TURN_ID_A,
            1,
            REVISION_ZERO,
            ConversationLifecycle::Pending,
            CREATED_AT_MILLIS,
            UPDATED_AT_MILLIS,
        );

        // Same Forge item identity twice.
        let mut items = snapshot.reborrow().init_items(2);
        set_user_message(
            items.reborrow().get(0).init_user_message(),
            ITEM_ID_A,
            TURN_ID_A,
            2,
            REVISION_ZERO,
            ConversationLifecycle::Pending,
        );
        set_user_message(
            items.get(1).init_user_message(),
            ITEM_ID_A,
            TURN_ID_A,
            3,
            REVISION_ZERO,
            ConversationLifecycle::Pending,
        );

        snapshot.set_updated_at_millis(UPDATED_AT_MILLIS);
        encode(&message)
    };

    let decoded = decode(&encoded)?;
    let root: envelope::Reader = decoded.get_root()?;
    assert_envelope_header(root, INVALID_SNAPSHOT_ONE_FRAME_ID)?;
    match root.get_body().which()? {
        envelope::body::Which::Response(res) => match res?.which()? {
            response::Which::ConversationSnapshot(snapshot) => {
                let snapshot = snapshot?;
                let turns = snapshot.get_turns()?;
                assert_eq!(turns.len(), 2);
                assert_eq!(turns.get(0).get_turn_id()?, TURN_ID_A);
                assert_eq!(turns.get(1).get_turn_id()?, TURN_ID_A);

                let items = snapshot.get_items()?;
                assert_eq!(items.len(), 2);
                for item in items {
                    match item.which()? {
                        conversation_item::Which::UserMessage(message) => {
                            assert_eq!(message?.get_item_id()?, ITEM_ID_A);
                        }
                        conversation_item::Which::Unmodeled(()) => {
                            panic!("expected userMessage item")
                        }
                    }
                }
            }
            _ => panic!("expected conversationSnapshot response"),
        },
        _ => panic!("expected response body"),
    }

    Ok(())
}

#[test]
fn unknown_turn_reference_snapshot_stays_representable() -> capnp::Result<()> {
    // Negative coverage available at the schema layer: an item may name a
    // turn the snapshot never carried, and may claim an ordinal another
    // entity already holds globally. Owned conversion owns both rejections
    // (unknown turn reference / duplicate ordinal).
    let encoded = {
        let mut message = frame();
        let mut res = init_envelope(&mut message, INVALID_SNAPSHOT_TWO_FRAME_ID)
            .init_body()
            .init_response();
        res.set_request_id(CLIENT_REQUEST_ID);
        let mut snapshot = res.init_conversation_snapshot();
        snapshot.set_thread_id(THREAD_ID);
        snapshot.set_cursor(SNAPSHOT_CURSOR);

        let turns = snapshot.reborrow().init_turns(1);
        set_turn(
            turns.get(0),
            TURN_ID_A,
            0,
            REVISION_ZERO,
            ConversationLifecycle::Pending,
            CREATED_AT_MILLIS,
            UPDATED_AT_MILLIS,
        );

        let items = snapshot.reborrow().init_items(1);
        set_user_message(
            items.get(0).init_user_message(),
            ITEM_ID_B,
            TURN_ID_UNKNOWN,
            0,
            REVISION_ZERO,
            ConversationLifecycle::Pending,
        );
        snapshot.set_updated_at_millis(UPDATED_AT_MILLIS);
        encode(&message)
    };

    let decoded = decode(&encoded)?;
    let root: envelope::Reader = decoded.get_root()?;
    assert_envelope_header(root, INVALID_SNAPSHOT_TWO_FRAME_ID)?;
    match root.get_body().which()? {
        envelope::body::Which::Response(res) => match res?.which()? {
            response::Which::ConversationSnapshot(snapshot) => {
                let snapshot = snapshot?;
                assert_eq!(snapshot.get_turns()?.len(), 1);
                let items = snapshot.get_items()?;
                assert_eq!(items.len(), 1);
                match items.get(0).which()? {
                    conversation_item::Which::UserMessage(message) => {
                        let message = message?;
                        assert_eq!(message.get_item_id()?, ITEM_ID_B);
                        assert_eq!(message.get_turn_id()?, TURN_ID_UNKNOWN);
                        assert_eq!(message.get_ordinal(), 0);
                    }
                    conversation_item::Which::Unmodeled(()) => {
                        panic!("expected userMessage item")
                    }
                }
            }
            _ => panic!("expected conversationSnapshot response"),
        },
        _ => panic!("expected response body"),
    }

    Ok(())
}
