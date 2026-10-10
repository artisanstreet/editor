//! Transport-thread child for where threads live: the archive Settings
//! reads, and renaming, settling, archiving, and restoring a thread.
//!
//! Every command is answered by exactly one [`ThreadLifecycleEvent`]. The
//! sidebar's own listing follows a change as a pushed
//! [`Event::RecentThreads`](artisan_domain::Event::RecentThreads).

#![forbid(unsafe_code)]
#![allow(clippy::module_name_repetitions)]

use artisan_domain::{ArchivedThreadListing, ReadArchivedThreads, UpdateThread};

use super::*;

/// One thread lifecycle command sent from the application thread.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ThreadLifecycleCommand {
    /// Read the archived threads.
    ReadArchived,
    /// Rename, settle, archive, or restore one thread.
    Update(UpdateThread),
}

/// One result returned by the service child.
#[derive(Clone, Debug, PartialEq)]
pub enum ThreadLifecycleEvent {
    /// The archived threads as read.
    Archived(Result<Box<ArchivedThreadListing>, ServiceFailure>),
    /// The archived threads after a change, or the change's failure.
    Updated {
        /// The change this answers.
        update: UpdateThread,
        /// The archived threads after the change, or the failure.
        result: Result<Box<ArchivedThreadListing>, ServiceFailure>,
    },
}

/// Handles one command on the authenticated service runtime.
pub(super) async fn handle_thread_lifecycle_command(
    runtime: &mut ServiceRuntime,
    frames: &mut FrameFactory,
    events: &SyncSender<NativeTransportEvent>,
    command: ThreadLifecycleCommand,
) -> Result<(), ServiceFailure> {
    let event = match command {
        ThreadLifecycleCommand::ReadArchived => ThreadLifecycleEvent::Archived(
            Box::pin(runtime.request(
                frames,
                query_request(Query::ReadArchivedThreads(ReadArchivedThreads)),
                ExpectedResponse::ArchivedThreads,
            ))
            .await
            .map_err(ServiceFailure::from)
            .and_then(archived),
        ),
        ThreadLifecycleCommand::Update(update) => {
            let result = mutate(runtime, frames, &update).await.and_then(archived);
            ThreadLifecycleEvent::Updated { update, result }
        }
    };
    publish(events, NativeTransportEvent::ThreadLifecycle(event))
}

fn archived(payload: ResponsePayload) -> Result<Box<ArchivedThreadListing>, ServiceFailure> {
    match payload {
        ResponsePayload::ArchivedThreads(listing) => Ok(Box::new(listing)),
        _ => Err(ServiceFailure::invalid(ServiceFailureStage::Request)),
    }
}

/// Sends one change with its stable frame identity.
async fn mutate(
    runtime: &mut ServiceRuntime,
    frames: &mut FrameFactory,
    update: &UpdateThread,
) -> Result<ResponsePayload, ServiceFailure> {
    let frame_id = FrameId::parse(update.request_id.as_str().to_owned())
        .map_err(|_| ServiceFailure::invalid(ServiceFailureStage::Request))?;
    let sent_at =
        real_unix_millis().map_err(|_| ServiceFailure::invalid(ServiceFailureStage::Request))?;
    let mutation = StableMutation {
        frame_id,
        sent_at,
        command: Command::UpdateThread(update.clone()),
    };
    Box::pin(durable_save_request(
        runtime,
        frames,
        &mutation,
        ExpectedResponse::ArchivedThreads,
    ))
    .await
    .map_err(ServiceFailure::from)
}
