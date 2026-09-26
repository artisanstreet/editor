//! Owned protocol coverage for the cross-project recent-threads listing: the
//! read, its answer, and the pushed event.

use std::error::Error;

use artisan_domain::{
    DisplayName, Event, ProjectId, Query, RECENT_THREADS_MAX, ReadRecentThreads, RecentThread,
    RecentThreadListing, RequestId, ThreadId, ThreadListingError, ThreadSummary, ThreadTitle,
    UnixMillis,
};
use artisan_protocol::{
    ClientRequest, EventCursor, FrameId, ProtocolVersion, ResponsePayload, ServerEvent,
    ServerResponse, WireEnvelope, WireEnvelopeBody, decode_envelope, encode_envelope,
};

fn round_trip(body: WireEnvelopeBody, frame: &str) -> Result<(), Box<dyn Error>> {
    let envelope = WireEnvelope {
        protocol_version: ProtocolVersion::V1,
        frame_id: FrameId::parse(frame)?,
        sent_at: UnixMillis::from_millis(11),
        body,
    };
    assert!(decode_envelope(&encode_envelope(&envelope)?)? == envelope);
    Ok(())
}

fn row(thread: &str, project: &str, subtitle: &str, last_message: Option<i64>) -> RecentThread {
    RecentThread {
        thread: ThreadSummary {
            has_started_response: true,
            has_active_work: last_message.is_none(),
            last_message_at: last_message.map(UnixMillis::from_millis),
            thread_id: ThreadId::parse(thread).expect("thread id"),
            project_id: ProjectId::parse(project).expect("project id"),
            title: ThreadTitle::parse(format!("Title of {thread}")).expect("title"),
            created_at: UnixMillis::from_millis(10),
            updated_at: UnixMillis::from_millis(20),
        },
        subtitle: DisplayName::parse(subtitle).expect("subtitle"),
    }
}

fn listing() -> Result<RecentThreadListing, Box<dyn Error>> {
    Ok(RecentThreadListing::new(vec![
        row("thread-a", "project-a", "artisanstreet/editor", Some(900)),
        row(
            "thread-b",
            "project-b",
            "artisanstreet/editor · sidebar-recents",
            None,
        ),
        row("thread-c", "project-c", "scratch", Some(100)),
    ])?)
}

#[test]
fn recent_threads_read_and_answer_round_trip() -> Result<(), Box<dyn Error>> {
    round_trip(
        WireEnvelopeBody::Request(ClientRequest::Query(Query::ReadRecentThreads(
            ReadRecentThreads,
        ))),
        "read-recent-threads",
    )?;
    for listing in [RecentThreadListing::default(), listing()?] {
        round_trip(
            WireEnvelopeBody::Response(ServerResponse {
                request_id: RequestId::parse("read-recent-threads")?,
                payload: ResponsePayload::RecentThreads(listing),
            }),
            "recent-threads",
        )?;
    }
    Ok(())
}

#[test]
fn pushed_recent_threads_round_trip() -> Result<(), Box<dyn Error>> {
    round_trip(
        WireEnvelopeBody::Event(ServerEvent {
            cursor: EventCursor::new(4)?,
            event: Event::RecentThreads(listing()?),
        }),
        "recent-threads-push",
    )
}

#[test]
fn recent_threads_are_bounded_and_unique() {
    let rows = (0..=RECENT_THREADS_MAX)
        .map(|index| row(&format!("thread-{index}"), "project", "project", Some(1)))
        .collect::<Vec<_>>();
    assert!(matches!(
        RecentThreadListing::new(rows),
        Err(ThreadListingError::TooManyThreads { maximum, .. }) if maximum == RECENT_THREADS_MAX
    ));
    let twice = vec![
        row("thread-a", "project", "project", Some(1)),
        row("thread-a", "other", "other", Some(2)),
    ];
    assert!(matches!(
        RecentThreadListing::new(twice),
        Err(ThreadListingError::DuplicateThread { .. })
    ));
}

#[test]
fn last_activity_prefers_the_latest_message() {
    assert_eq!(
        row("thread-a", "project", "project", Some(900)).last_activity(),
        UnixMillis::from_millis(900)
    );
    assert_eq!(
        row("thread-b", "project", "project", None).last_activity(),
        UnixMillis::from_millis(20)
    );
}
