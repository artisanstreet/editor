//! Recent-threads and project-catalog delivery: a connection that read the
//! recent threads, or listed the projects, receives them again whenever they
//! change.

use std::time::Duration;

use artisan_database::RecentThreadsFingerprint;
use artisan_domain::{ConversationRequest, Event, ProjectListing, RecentThreadListing, ThreadId};
use artisan_protocol::{ClientRequest, ProtocolFailure, ResponsePayload, ServerResponse};
use artisan_transport::{CancelHandle, DeadlineError, OperationKind, run_with_deadline};

use crate::connection::{DeliveryStageError, RequestStageError, ServerFrameStamp};

use super::{ConversationDeliveryDriver, map_deadline};

/// What a handled request asks of delivery once its response is out.
#[derive(Debug, Default)]
pub(crate) struct RequestFollowUp {
    /// The thread an unsubscribe stopped.
    pub(crate) stopped_thread: Option<ThreadId>,
    /// The recent threads a read served: the connection now receives their
    /// changes.
    pub(crate) recent_threads: Option<RecentThreadListing>,
    /// The project catalog a listing served: the connection now receives
    /// its changes.
    pub(crate) project_catalog: Option<ProjectListing>,
}

impl RequestFollowUp {
    /// Derives the follow-up of one request from the request and its answer.
    pub(crate) fn after(
        request: &ClientRequest,
        answered: &Result<ServerResponse, ProtocolFailure>,
    ) -> Self {
        let stopped_thread = match request {
            ClientRequest::Conversation(ConversationRequest::Unsubscribe(unsubscribe)) => {
                Some(unsubscribe.thread_id.clone())
            }
            _ => None,
        };
        let payload = answered.as_ref().ok().map(|response| &response.payload);
        let recent_threads = match payload {
            Some(ResponsePayload::RecentThreads(listing)) => Some(listing.clone()),
            _ => None,
        };
        let project_catalog = match payload {
            Some(ResponsePayload::ProjectListing(listing)) => Some(listing.clone()),
            _ => None,
        };
        Self {
            stopped_thread,
            recent_threads,
            project_catalog,
        }
    }
}

/// The recent threads this connection last received and the inputs they
/// were read at; `None` inputs force the next look to read them again.
#[derive(Debug)]
pub(super) struct DeliveredRecentThreads {
    pub(super) fingerprint: Option<RecentThreadsFingerprint>,
    pub(super) subtitle_generation: u64,
    pub(super) listing: RecentThreadListing,
}

impl ConversationDeliveryDriver {
    /// Pushes the recent threads when they changed since this connection
    /// last received them; nothing before the connection read them. A cheap
    /// fingerprint (and the subtitle cache's generation) skips the listing
    /// read while nothing it shows moved.
    pub(super) async fn deliver_recent_threads<F>(
        &mut self,
        stamp: &mut F,
        limit: Duration,
        cancel: &CancelHandle,
    ) -> Result<(), DeadlineError<RequestStageError>>
    where
        F: FnMut() -> Result<ServerFrameStamp, RequestStageError>,
    {
        let Some(delivered) = self.recent.as_ref() else {
            return Ok(());
        };
        let repository = self.context.repository().clone();
        let fingerprint = run_with_deadline(
            OperationKind::Receive,
            limit,
            cancel,
            repository.recent_threads_fingerprint(),
        )
        .await
        .map_err(|error| map_deadline(&error, DeliveryStageError::Replay))?;
        let subtitle_generation = self.context.subtitle_generation();
        if delivered.fingerprint == Some(fingerprint)
            && delivered.subtitle_generation == subtitle_generation
        {
            return Ok(());
        }
        let listing = run_with_deadline(
            OperationKind::Receive,
            limit,
            cancel,
            self.context.recent_threads(),
        )
        .await
        .map_err(|error| map_deadline(&error, DeliveryStageError::Replay))?;
        if self
            .recent
            .as_ref()
            .is_none_or(|delivered| delivered.listing != listing)
        {
            let event = Event::RecentThreads(listing.clone());
            self.send_state_event(event, stamp, limit, cancel).await?;
        }
        self.recent = Some(DeliveredRecentThreads {
            fingerprint: Some(fingerprint),
            subtitle_generation,
            listing,
        });
        Ok(())
    }

    /// Pushes the project catalog when it changed since this connection
    /// last received it; nothing before the connection listed the projects.
    /// The catalog is a handful of rows, so it is read and compared whole.
    pub(super) async fn deliver_project_catalog<F>(
        &mut self,
        stamp: &mut F,
        limit: Duration,
        cancel: &CancelHandle,
    ) -> Result<(), DeadlineError<RequestStageError>>
    where
        F: FnMut() -> Result<ServerFrameStamp, RequestStageError>,
    {
        if self.projects.is_none() {
            return Ok(());
        }
        let repository = self.context.repository().clone();
        let listing = run_with_deadline(
            OperationKind::Receive,
            limit,
            cancel,
            repository.list_projects(),
        )
        .await
        .map_err(|error| map_deadline(&error, DeliveryStageError::Replay))?;
        if self.projects.as_ref() == Some(&listing) {
            return Ok(());
        }
        let event = Event::ProjectCatalog(listing.clone());
        self.send_state_event(event, stamp, limit, cancel).await?;
        self.projects = Some(listing);
        Ok(())
    }
}
